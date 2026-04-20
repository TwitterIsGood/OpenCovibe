//! Run index — scans events.jsonl + meta.json to build a searchable
//! summary of every run (tools, files, cost, errors, etc.).
//!
//! Index file:    `~/.opencovibe/run-index.jsonl`
//! Manifest file: `~/.opencovibe/run-index-manifest.json`
//!
//! Uses in-memory cache with 120s TTL (same pattern as `prompt_index.rs`).

use super::floor_char_boundary;
use crate::models::RunStatus;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

const MANIFEST_VERSION: u32 = 1;
const CACHE_TTL_SECS: u64 = 120;
const LITE_CACHE_TTL_SECS: u64 = 30;
const LITE_READ_BYTES: u64 = 65_536; // 64KB, same as cli_sessions.rs

// ── Types ──

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub enum EntryTier {
    Lite,
    Full,
}

fn default_entry_tier() -> EntryTier {
    EntryTier::Full
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RunIndexEntry {
    pub run_id: String,
    pub cwd: String,
    pub agent: String,
    pub model: Option<String>,
    pub status: RunStatus,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub name: Option<String>,
    pub prompt_preview: String,
    pub tools_used: Vec<String>,
    pub tool_call_count: u32,
    pub files_touched: Vec<String>,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    pub num_turns: u64,
    pub error_summary: Option<String>,
    pub has_errors: bool,
    pub permission_denied_count: u32,
    #[serde(default = "default_entry_tier")]
    pub entry_tier: EntryTier,
}

/// Manifest: tracks fingerprints per run to enable incremental updates.
/// Each run has dual fingerprints: (events_mtime_ns, events_size, meta_mtime_ns, meta_size).
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    runs: HashMap<String, (u128, u64, u128, u64)>,
}

// ── Cache ──

struct CachedIndex {
    computed_at: Instant,
    entries: Vec<RunIndexEntry>,
}

static CACHE: std::sync::LazyLock<Mutex<Option<CachedIndex>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

static COMPUTE_LOCK: std::sync::LazyLock<Mutex<()>> = std::sync::LazyLock::new(|| Mutex::new(()));

static LITE_CACHE: std::sync::LazyLock<Mutex<Option<CachedIndex>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

// ── File paths ──

fn index_path() -> PathBuf {
    super::data_dir().join("run-index.jsonl")
}

fn manifest_path() -> PathBuf {
    super::data_dir().join("run-index-manifest.json")
}

/// Atomically write content to `path` (write .tmp -> set 0o600 -> rename).
fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content).map_err(|e| format!("write tmp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

fn file_fingerprint(path: &Path) -> Option<(u128, u64)> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((mtime, meta.len()))
}

// ── Scanning ──

/// Max prompt preview length.
const MAX_PREVIEW_LEN: usize = 100;
/// Max error summary length.
const MAX_ERROR_LEN: usize = 200;

/// Read the head (first N bytes) and tail (last N bytes) of a file.
/// Returns `(head_string, tail_string)`. The tail's first line is pre-trimmed
/// (may be truncated by seek).
fn read_head_tail(path: &Path, size: u64, chunk_bytes: u64) -> Option<(String, String)> {
    let mut file = fs::File::open(path).ok()?;

    // Head: read from start
    let head_size = (size as usize).min(chunk_bytes as usize);
    let mut head_buf = vec![0u8; head_size];
    file.read_exact(&mut head_buf).ok()?;
    let head_str = String::from_utf8_lossy(&head_buf).into_owned();

    // Tail: only if file is larger than one chunk
    if size > chunk_bytes {
        let tail_start = size - chunk_bytes;
        file.seek(SeekFrom::Start(tail_start)).ok()?;
        let mut tail_buf = vec![0u8; chunk_bytes as usize];
        let bytes_read = file.read(&mut tail_buf).ok()?;
        tail_buf.truncate(bytes_read);
        let tail_raw = String::from_utf8_lossy(&tail_buf).into_owned();
        // Skip first line (may be truncated by seek)
        let tail_str = if let Some(nl_pos) = tail_raw.find('\n') {
            tail_raw[nl_pos + 1..].to_string()
        } else {
            String::new()
        };
        Some((head_str, tail_str))
    } else {
        Some((head_str, String::new()))
    }
}

/// Accumulator for events.jsonl line parsing (shared between full and fast scans).
struct ScanAccum {
    tools_set: HashSet<String>,
    files_set: HashSet<String>,
    tool_call_count: u32,
    num_turns: u64,
    has_errors: bool,
    error_summary: Option<String>,
    permission_denied_count: u32,
    is_per_turn_cost: bool,
    total_cost: f64,
    prev_cost: f64,
    peak_cost: f64,
    last_input: u64,
    last_output: u64,
    total_duration_ms: u64,
    last_num_turns: u64,
}

impl ScanAccum {
    fn new(is_per_turn_cost: bool) -> Self {
        Self {
            tools_set: HashSet::new(),
            files_set: HashSet::new(),
            tool_call_count: 0,
            num_turns: 0,
            has_errors: false,
            error_summary: None,
            permission_denied_count: 0,
            is_per_turn_cost,
            total_cost: 0.0,
            prev_cost: 0.0,
            peak_cost: 0.0,
            last_input: 0,
            last_output: 0,
            total_duration_ms: 0,
            last_num_turns: 0,
        }
    }

    fn parse_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }

        // Pre-filter: only parse lines containing relevant event types
        if !line.contains("\"tool_start\"")
            && !line.contains("\"tool_end\"")
            && !line.contains("\"files_persisted\"")
            && !line.contains("\"usage_update\"")
            && !line.contains("\"run_state\"")
            && !line.contains("\"permission_denied\"")
            && !line.contains("\"user_message\"")
        {
            return;
        }

        let envelope: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };

        let event = if envelope
            .get("_bus")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            match envelope.get("event") {
                Some(e) => e,
                None => return,
            }
        } else {
            &envelope
        };

        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "tool_start" => {
                if let Some(tool_name) = event.get("tool_name").and_then(|v| v.as_str()) {
                    self.tools_set.insert(tool_name.to_string());
                }
                self.tool_call_count += 1;
                if let Some(fp) = event
                    .get("input")
                    .and_then(|i| i.get("file_path"))
                    .and_then(|v| v.as_str())
                {
                    self.files_set.insert(fp.to_string());
                }
            }
            "tool_end" => {
                if let Some(fp) = event
                    .get("tool_use_result")
                    .and_then(|r| r.get("filePath"))
                    .and_then(|v| v.as_str())
                {
                    self.files_set.insert(fp.to_string());
                }
            }
            "files_persisted" => {
                if let Some(files) = event.get("files").and_then(|v| v.as_array()) {
                    for f in files {
                        if let Some(fname) = f.get("filename").and_then(|v| v.as_str()) {
                            self.files_set.insert(fname.to_string());
                        }
                    }
                }
            }
            "usage_update" => {
                let cost = event
                    .get("total_cost_usd")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                if self.is_per_turn_cost {
                    self.total_cost += cost;
                } else {
                    if cost < self.prev_cost * 0.9 && self.prev_cost > 0.0 {
                        self.total_cost += self.peak_cost;
                        self.peak_cost = 0.0;
                    }
                    if cost > self.peak_cost {
                        self.peak_cost = cost;
                    }
                    self.prev_cost = cost;
                }

                if self.is_per_turn_cost {
                    self.last_input += event
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    self.last_output += event
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                } else {
                    self.last_input = event
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(self.last_input);
                    self.last_output = event
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(self.last_output);
                }

                if let Some(d) = event.get("duration_ms").and_then(|v| v.as_u64()) {
                    self.total_duration_ms += d;
                }
                self.last_num_turns = event
                    .get("num_turns")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(self.last_num_turns);
            }
            "run_state" => {
                if let Some(err) = event.get("error").and_then(|v| v.as_str()) {
                    self.has_errors = true;
                    let truncated = if err.len() > MAX_ERROR_LEN {
                        format!(
                            "{}...",
                            &err[..floor_char_boundary(err, MAX_ERROR_LEN)]
                        )
                    } else {
                        err.to_string()
                    };
                    self.error_summary = Some(truncated);
                }
            }
            "permission_denied" => {
                self.permission_denied_count += 1;
            }
            "user_message" => {
                self.num_turns += 1;
            }
            _ => {}
        }
    }

    fn finalize_cost(&mut self) {
        if !self.is_per_turn_cost {
            self.total_cost += self.peak_cost;
        }
    }
}

/// Scan a single run's events.jsonl + meta.json to produce a RunIndexEntry.
pub fn scan_run(run_id: &str, events_path: &Path, meta_json: &serde_json::Value) -> RunIndexEntry {
    let started_at = meta_json
        .get("started_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let ended_at = meta_json
        .get("ended_at")
        .and_then(|v| v.as_str())
        .map(String::from);

    let is_per_turn_cost = meta_json.get("source").and_then(|v| v.as_str()) == Some("cli_import");
    let mut accum = ScanAccum::new(is_per_turn_cost);

    if let Ok(file) = fs::File::open(events_path) {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            accum.parse_line(&line);
        }
    }

    accum.finalize_cost();
    build_entry(run_id, &started_at, ended_at.as_deref(), meta_json, accum, EntryTier::Full)
}

/// Fast scan using head 64KB + tail 64KB byte-range reads.
/// Produces a Lite entry with approximate data for instant display.
pub fn scan_run_fast(run_id: &str, events_path: &Path, meta_json: &serde_json::Value) -> RunIndexEntry {
    let started_at = meta_json
        .get("started_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let ended_at = meta_json
        .get("ended_at")
        .and_then(|v| v.as_str())
        .map(String::from);

    let is_per_turn_cost = meta_json.get("source").and_then(|v| v.as_str()) == Some("cli_import");
    let mut accum = ScanAccum::new(is_per_turn_cost);

    let file_size = fs::metadata(events_path).map(|m| m.len()).unwrap_or(0);
    if let Some((head_str, tail_str)) = read_head_tail(events_path, file_size, LITE_READ_BYTES) {
        // Parse head region (all lines valid)
        for line in head_str.lines() {
            accum.parse_line(line);
        }
        // Parse tail region (first line already stripped by read_head_tail)
        if !tail_str.is_empty() {
            for line in tail_str.lines() {
                accum.parse_line(line);
            }
        }
    }

    accum.finalize_cost();
    build_entry(run_id, &started_at, ended_at.as_deref(), meta_json, accum, EntryTier::Lite)
}

/// Build a RunIndexEntry from meta.json fields + a ScanAccum.
fn build_entry(
    run_id: &str,
    started_at: &str,
    ended_at: Option<&str>,
    meta_json: &serde_json::Value,
    accum: ScanAccum,
    tier: EntryTier,
) -> RunIndexEntry {
    let final_num_turns = if accum.last_num_turns > 0 {
        accum.last_num_turns
    } else {
        accum.num_turns
    };

    let final_duration = if accum.total_duration_ms > 0 {
        accum.total_duration_ms
    } else {
        calc_duration_ms(started_at, ended_at).unwrap_or(0)
    };

    let mut tools_used: Vec<String> = accum.tools_set.into_iter().collect();
    tools_used.sort();
    let mut files_touched: Vec<String> = accum.files_set.into_iter().collect();
    files_touched.sort();

    RunIndexEntry {
        run_id: run_id.to_string(),
        cwd: meta_json
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        agent: meta_json
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("claude")
            .to_string(),
        model: meta_json
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        status: meta_json
            .get("status")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(RunStatus::Completed),
        started_at: started_at.to_string(),
        ended_at: ended_at.map(String::from),
        name: meta_json
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from),
        prompt_preview: {
            let prompt = meta_json
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if prompt.len() > MAX_PREVIEW_LEN {
                format!(
                    "{}...",
                    &prompt[..floor_char_boundary(prompt, MAX_PREVIEW_LEN)]
                )
            } else {
                prompt.to_string()
            }
        },
        tools_used,
        tool_call_count: accum.tool_call_count,
        files_touched,
        total_cost_usd: accum.total_cost,
        input_tokens: accum.last_input,
        output_tokens: accum.last_output,
        duration_ms: final_duration,
        num_turns: final_num_turns,
        error_summary: accum.error_summary,
        has_errors: accum.has_errors,
        permission_denied_count: accum.permission_denied_count,
        entry_tier: tier,
    }
}

/// Try to compute duration in ms from ISO timestamps.
fn calc_duration_ms(started: &str, ended: Option<&str>) -> Option<u64> {
    let ended = ended?;
    if started.is_empty() || ended.is_empty() {
        return None;
    }
    let start = chrono::DateTime::parse_from_rfc3339(started).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(ended).ok()?;
    let duration = end.signed_duration_since(start);
    if duration.num_milliseconds() >= 0 {
        Some(duration.num_milliseconds() as u64)
    } else {
        None
    }
}

// ── Index management ──

/// Build or incrementally update the run index.
pub fn build_or_update_index() -> Result<Vec<RunIndexEntry>, String> {
    // Fast path: check cache TTL
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                log::debug!("[run_index] cache hit ({} entries)", cached.entries.len());
                return Ok(cached.entries.clone());
            }
        }
    }

    // Acquire compute lock (prevents concurrent rebuilds)
    let _lock = COMPUTE_LOCK.lock().unwrap();

    // Double-check cache after acquiring lock
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                return Ok(cached.entries.clone());
            }
        }
    }

    log::debug!("[run_index] rebuilding index");
    let start = Instant::now();

    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        log::debug!("[run_index] no runs dir, returning empty");
        let entries = vec![];
        update_cache(entries.clone());
        return Ok(entries);
    }

    // Load existing manifest
    let mut manifest = load_manifest();
    let mut all_entries: Vec<RunIndexEntry> = vec![];

    // Load existing index entries (to reuse unchanged runs)
    let existing_entries = load_index_file();
    let mut existing_by_run: HashMap<String, RunIndexEntry> = HashMap::new();
    for entry in existing_entries {
        existing_by_run.insert(entry.run_id.clone(), entry);
    }

    // Collect current run IDs
    let mut current_run_ids: HashSet<String> = HashSet::new();

    if let Ok(dir_entries) = fs::read_dir(&runs_dir) {
        for entry in dir_entries.flatten() {
            let run_id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let events_path = entry.path().join("events.jsonl");
            let meta_path = entry.path().join("meta.json");

            if !events_path.exists() || !meta_path.exists() {
                continue;
            }

            // Read meta.json
            let meta_content = match fs::read_to_string(&meta_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta_json: serde_json::Value = match serde_json::from_str(&meta_content) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Skip soft-deleted runs
            if meta_json
                .get("deleted_at")
                .and_then(|v| v.as_str())
                .is_some()
            {
                continue;
            }

            current_run_ids.insert(run_id.clone());

            // Dual fingerprint: events.jsonl + meta.json
            let events_fp = file_fingerprint(&events_path);
            let meta_fp = file_fingerprint(&meta_path);

            let current_fp = match (events_fp, meta_fp) {
                (Some((em, es)), Some((mm, ms))) => Some((em, es, mm, ms)),
                _ => None,
            };
            let cached_fp = manifest.runs.get(&run_id).cloned();

            if current_fp == cached_fp
                && cached_fp.is_some()
                && existing_by_run.contains_key(&run_id)
            {
                // Unchanged - reuse cached entry
                if let Some(entry) = existing_by_run.remove(&run_id) {
                    all_entries.push(entry);
                }
            } else {
                // Changed or new - rescan
                log::debug!("[run_index] scanning run: {}", run_id);
                let entry = scan_run(&run_id, &events_path, &meta_json);
                all_entries.push(entry);

                // Update manifest
                if let Some(fp) = current_fp {
                    manifest.runs.insert(run_id, fp);
                }
            }
        }
    }

    // Remove deleted runs from manifest
    manifest.runs.retain(|id, _| current_run_ids.contains(id));

    // Write index + manifest atomically
    let index_content: String = all_entries
        .iter()
        .filter_map(|e| serde_json::to_string(e).ok())
        .collect::<Vec<_>>()
        .join("\n");

    super::ensure_dir(super::data_dir().as_path()).map_err(|e| e.to_string())?;
    write_atomic(&index_path(), &index_content)?;

    let manifest_json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    write_atomic(&manifest_path(), &manifest_json)?;

    let elapsed = start.elapsed();
    log::debug!(
        "[run_index] index built: {} entries in {:?}",
        all_entries.len(),
        elapsed
    );

    update_cache(all_entries.clone());
    Ok(all_entries)
}

/// Invalidate the in-memory cache (e.g. after a run completes).
pub fn invalidate_cache() {
    {
        let mut cache = CACHE.lock().unwrap();
        *cache = None;
    }
    {
        let mut lite_cache = LITE_CACHE.lock().unwrap();
        *lite_cache = None;
    }
    log::debug!("[run_index] cache invalidated (main + lite)");
}

/// Build a lite index for fast initial display using head+tail byte-range reads.
/// - If the main cache has valid Full entries, returns those
/// - If the lite cache is still fresh (< 30s), returns it
/// - Otherwise builds lite entries using scan_run_fast
pub fn build_lite_index() -> Result<Vec<RunIndexEntry>, String> {
    // Fast path 1: main cache has valid full entries
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref cached) = *cache {
            if cached.computed_at.elapsed().as_secs() < CACHE_TTL_SECS {
                let all_full = cached.entries.iter().all(|e| e.entry_tier == EntryTier::Full);
                if all_full {
                    log::debug!("[run_index] lite: returning main cache ({} full entries)", cached.entries.len());
                    return Ok(cached.entries.clone());
                }
            }
        }
    }

    // Fast path 2: lite cache still fresh
    {
        let lite_cache = LITE_CACHE.lock().unwrap();
        if let Some(ref cached) = *lite_cache {
            if cached.computed_at.elapsed().as_secs() < LITE_CACHE_TTL_SECS {
                log::debug!("[run_index] lite: returning lite cache ({} entries)", cached.entries.len());
                return Ok(cached.entries.clone());
            }
        }
    }

    log::debug!("[run_index] lite: building fresh lite index");
    let start = Instant::now();

    let runs_dir = super::runs_dir();
    if !runs_dir.exists() {
        let entries = vec![];
        update_lite_cache(entries.clone());
        return Ok(entries);
    }

    let mut all_entries: Vec<RunIndexEntry> = vec![];

    if let Ok(dir_entries) = fs::read_dir(&runs_dir) {
        for entry in dir_entries.flatten() {
            let run_id = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let events_path = entry.path().join("events.jsonl");
            let meta_path = entry.path().join("meta.json");

            if !events_path.exists() || !meta_path.exists() {
                continue;
            }

            let meta_content = match fs::read_to_string(&meta_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta_json: serde_json::Value = match serde_json::from_str(&meta_content) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if meta_json
                .get("deleted_at")
                .and_then(|v| v.as_str())
                .is_some()
            {
                continue;
            }

            let entry = scan_run_fast(&run_id, &events_path, &meta_json);
            all_entries.push(entry);
        }
    }

    let elapsed = start.elapsed();
    log::debug!(
        "[run_index] lite index built: {} entries in {:?}",
        all_entries.len(),
        elapsed
    );

    update_lite_cache(all_entries.clone());
    Ok(all_entries)
}

fn update_lite_cache(entries: Vec<RunIndexEntry>) {
    let mut lite_cache = LITE_CACHE.lock().unwrap();
    *lite_cache = Some(CachedIndex {
        computed_at: Instant::now(),
        entries,
    });
}

fn load_manifest() -> Manifest {
    let path = manifest_path();
    if !path.exists() {
        return Manifest {
            version: MANIFEST_VERSION,
            runs: HashMap::new(),
        };
    }
    match fs::read_to_string(&path) {
        Ok(content) => {
            let m: Manifest = serde_json::from_str(&content).unwrap_or(Manifest {
                version: MANIFEST_VERSION,
                runs: HashMap::new(),
            });
            // Version mismatch -> force full rescan
            if m.version != MANIFEST_VERSION {
                log::debug!(
                    "[run_index] manifest version {} != {}, forcing full rescan",
                    m.version,
                    MANIFEST_VERSION
                );
                return Manifest {
                    version: MANIFEST_VERSION,
                    runs: HashMap::new(),
                };
            }
            m
        }
        Err(_) => Manifest {
            version: MANIFEST_VERSION,
            runs: HashMap::new(),
        },
    }
}

fn load_index_file() -> Vec<RunIndexEntry> {
    let path = index_path();
    if !path.exists() {
        return vec![];
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn update_cache(entries: Vec<RunIndexEntry>) {
    let mut cache = CACHE.lock().unwrap();
    *cache = Some(CachedIndex {
        computed_at: Instant::now(),
        entries,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_meta(overrides: &serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "id": "test-run",
            "cwd": "/home/user/project",
            "agent": "claude",
            "status": "completed",
            "started_at": "2024-01-01T00:00:00.000Z",
            "ended_at": "2024-01-01T00:05:00.000Z",
            "prompt": "Hello world"
        });
        if let Some(obj) = overrides.as_object() {
            for (k, v) in obj {
                base[k] = v.clone();
            }
        }
        base
    }

    fn write_events_file(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_scan_extracts_tools() {
        let events = write_events_file(&[
            r#"{"_bus":true,"seq":1,"ts":"2024-01-01T00:00:00.000Z","event":{"type":"tool_start","tool_name":"Read","input":{"file_path":"/foo.ts"}}}"#,
            r#"{"_bus":true,"seq":2,"ts":"2024-01-01T00:00:01.000Z","event":{"type":"tool_start","tool_name":"Write","input":{"file_path":"/bar.ts"}}}"#,
            r#"{"_bus":true,"seq":3,"ts":"2024-01-01T00:00:02.000Z","event":{"type":"tool_start","tool_name":"Read","input":{"file_path":"/baz.ts"}}}"#,
        ]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        assert_eq!(entry.tools_used.len(), 2); // Read + Write (deduplicated)
        assert!(entry.tools_used.contains(&"Read".to_string()));
        assert!(entry.tools_used.contains(&"Write".to_string()));
        assert_eq!(entry.tool_call_count, 3); // 3 total calls
    }

    #[test]
    fn test_scan_extracts_files_from_three_sources() {
        let events = write_events_file(&[
            // ToolStart -> input.file_path
            r#"{"_bus":true,"seq":1,"ts":"2024-01-01T00:00:00.000Z","event":{"type":"tool_start","tool_name":"Read","input":{"file_path":"/src/a.ts"}}}"#,
            // ToolEnd -> tool_use_result.filePath
            r#"{"_bus":true,"seq":2,"ts":"2024-01-01T00:00:01.000Z","event":{"type":"tool_end","tool_use_result":{"filePath":"/src/b.ts"}}}"#,
            // FilesPersisted -> files[].filename
            r#"{"_bus":true,"seq":3,"ts":"2024-01-01T00:00:02.000Z","event":{"type":"files_persisted","files":[{"filename":"/src/c.ts","file_id":"f-1"},{"filename":"/src/a.ts","file_id":"f-2"}]}}"#,
        ]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        // Should merge and dedup: a.ts, b.ts, c.ts
        assert_eq!(entry.files_touched.len(), 3);
        assert!(entry.files_touched.contains(&"/src/a.ts".to_string()));
        assert!(entry.files_touched.contains(&"/src/b.ts".to_string()));
        assert!(entry.files_touched.contains(&"/src/c.ts".to_string()));
    }

    #[test]
    fn test_scan_extracts_cost() {
        let events = write_events_file(&[
            r#"{"_bus":true,"seq":1,"ts":"2024-01-01T00:00:00.000Z","event":{"type":"usage_update","total_cost_usd":0.1,"input_tokens":100,"output_tokens":50,"duration_ms":1000,"num_turns":1}}"#,
            r#"{"_bus":true,"seq":2,"ts":"2024-01-01T00:00:01.000Z","event":{"type":"usage_update","total_cost_usd":0.3,"input_tokens":200,"output_tokens":100,"duration_ms":2000,"num_turns":2}}"#,
            r#"{"_bus":true,"seq":3,"ts":"2024-01-01T00:00:02.000Z","event":{"type":"usage_update","total_cost_usd":0.5,"input_tokens":300,"output_tokens":150,"duration_ms":3000,"num_turns":3}}"#,
        ]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        // Peak detection: single segment, peak = 0.5
        assert!((entry.total_cost_usd - 0.5).abs() < 0.001);
        assert_eq!(entry.input_tokens, 300);
        assert_eq!(entry.output_tokens, 150);
        assert_eq!(entry.duration_ms, 6000); // sum of duration_ms
        assert_eq!(entry.num_turns, 3);
    }

    #[test]
    fn test_scan_extracts_errors() {
        let long_error = "x".repeat(300);
        let line = format!(
            r#"{{"_bus":true,"seq":1,"ts":"2024-01-01T00:00:00.000Z","event":{{"type":"run_state","state":"error","error":"{}"}}}}"#,
            long_error
        );
        let events = write_events_file(&[&line]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        assert!(entry.has_errors);
        assert!(entry.error_summary.is_some());
        let summary = entry.error_summary.unwrap();
        // Truncated to MAX_ERROR_LEN + "..."
        assert!(summary.len() <= MAX_ERROR_LEN + 3 + 4); // +4 for potential char boundary overshoot
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn test_scan_permission_denied() {
        let events = write_events_file(&[
            r#"{"_bus":true,"seq":1,"ts":"2024-01-01T00:00:00.000Z","event":{"type":"permission_denied"}}"#,
            r#"{"_bus":true,"seq":2,"ts":"2024-01-01T00:00:01.000Z","event":{"type":"permission_denied"}}"#,
        ]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        assert_eq!(entry.permission_denied_count, 2);
    }

    #[test]
    fn test_scan_empty_events() {
        let events = write_events_file(&[]);
        let meta = make_meta(&serde_json::json!({}));
        let entry = scan_run("test", events.path(), &meta);

        assert_eq!(entry.tool_call_count, 0);
        assert_eq!(entry.tools_used.len(), 0);
        assert_eq!(entry.files_touched.len(), 0);
        assert!((entry.total_cost_usd - 0.0).abs() < 0.001);
        assert_eq!(entry.input_tokens, 0);
        assert_eq!(entry.output_tokens, 0);
        assert!(!entry.has_errors);
        assert_eq!(entry.permission_denied_count, 0);
        assert_eq!(entry.num_turns, 0);
    }
}
