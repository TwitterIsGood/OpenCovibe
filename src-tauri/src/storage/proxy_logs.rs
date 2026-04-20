use crate::models::{now_iso, ProxyDayHealth, ProxyLogFilter, ProxyRequestLog};
use chrono::Timelike;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

fn log_path() -> PathBuf {
    super::data_dir().join("proxy-logs.jsonl")
}

#[derive(Debug)]
pub struct ProxyLogStore {
    seq: AtomicU64,
    write: Mutex<File>,
}

impl ProxyLogStore {
    pub fn new() -> Self {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = super::ensure_dir(parent);
        }
        // Count existing lines to determine next seq
        let next_seq = if path.exists() {
            let file = File::open(&path).unwrap_or_else(|e| {
                panic!("proxy-logs.jsonl open failed: {e}");
            });
            BufReader::new(file).lines().count() as u64 + 1
        } else {
            1
        };

        let write_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| {
                panic!("proxy-logs.jsonl create failed: {e}");
            });

        Self {
            seq: AtomicU64::new(next_seq),
            write: Mutex::new(write_file),
        }
    }

    /// Append a log entry.
    pub fn append(&self, mut entry: ProxyRequestLog) {
        entry.id = self.seq.fetch_add(1, Ordering::Relaxed);
        entry.ts = now_iso();
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[proxy_logs] serialize error: {e}");
                return;
            }
        };
        if let Ok(mut f) = self.write.lock() {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Query logs with filter, limit, offset. Returns (entries, total_matching).
    pub fn query(
        &self,
        filter: &ProxyLogFilter,
        limit: u32,
        offset: u32,
    ) -> (Vec<ProxyRequestLog>, u64) {
        let path = log_path();
        if !path.exists() {
            return (vec![], 0);
        }
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return (vec![], 0),
        };

        let cutoff = filter.days.map(|d| {
            let secs = (d as u64) * 86400;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now - secs
        });

        let model_filter = filter.model.as_deref();
        let provider_filter = filter.provider_id.as_deref();

        let mut all: Vec<ProxyRequestLog> = Vec::new();

        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            let entry: ProxyRequestLog = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Apply filters
            if let Some(mf) = model_filter {
                if entry.model != mf && entry.actual_model != mf {
                    continue;
                }
            }
            if let Some(pf) = provider_filter {
                if entry.provider_id != pf {
                    continue;
                }
            }
            if let Some(cutoff_ts) = cutoff {
                // Compare via ts string prefix date
                if let Ok(entry_time) = entry.ts[..19].parse::<chrono::DateTime<chrono::Utc>>() {
                    let entry_epoch = entry_time.timestamp() as u64;
                    if entry_epoch < cutoff_ts {
                        continue;
                    }
                }
            }
            all.push(entry);
        }

        let total = all.len() as u64;
        // Reverse chronological order (newest first)
        all.sort_by(|a, b| b.id.cmp(&a.id));
        let entries: Vec<ProxyRequestLog> = all
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        (entries, total)
    }

    /// Get health summary aggregated by 30-minute slots.
    /// Returns all providers combined, one entry per slot.
    pub fn health(&self, hours: u32) -> Vec<ProxyDayHealth> {
        let path = log_path();
        if !path.exists() {
            return vec![];
        }
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return vec![],
        };

        let cutoff_secs = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            now - (hours as u64) * 3600
        };

        use std::collections::HashMap;
        // key: slot string "2026-04-20T13:00" -> (success, error)
        let mut map: HashMap<String, (u32, u32)> = HashMap::new();

        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            let entry: ProxyRequestLog = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };

            if let Ok(entry_time) = entry.ts[..19].parse::<chrono::DateTime<chrono::Utc>>() {
                let entry_epoch = entry_time.timestamp() as u64;
                if entry_epoch < cutoff_secs {
                    continue;
                }
                // Floor to 30-min slot: "2026-04-20T13:27" -> "2026-04-20T13:00"
                let minute = entry_time.minute();
                let slot_min = if minute < 30 { 0 } else { 30 };
                let slot = format!(
                    "{}T{:02}:{:02}",
                    entry_time.format("%Y-%m-%d"),
                    entry_time.hour(),
                    slot_min
                );
                let (s, e) = map.entry(slot).or_insert((0, 0));
                if entry.result == "success" {
                    *s += 1;
                } else {
                    *e += 1;
                }
            }
        }

        let mut result: Vec<ProxyDayHealth> = map
            .into_iter()
            .map(|(slot, (success_count, error_count))| ProxyDayHealth {
                date: slot,
                provider_id: String::new(),
                success_count,
                error_count,
            })
            .collect();
        result.sort_by(|a, b| a.date.cmp(&b.date));
        result
    }

    /// Get distinct models and provider IDs for filter dropdowns.
    pub fn distinct_values(&self) -> (Vec<String>, Vec<String>) {
        let path = log_path();
        if !path.exists() {
            return (vec![], vec![]);
        }
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return (vec![], vec![]),
        };

        let mut models = std::collections::BTreeSet::new();
        let mut providers = std::collections::BTreeSet::new();

        let reader = BufReader::new(file);
        for line in reader.lines().flatten() {
            let entry: ProxyRequestLog = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.model.is_empty() {
                models.insert(entry.model.clone());
            }
            if !entry.provider_id.is_empty() {
                providers.insert(entry.provider_id.clone());
            }
        }

        (
            models.into_iter().collect(),
            providers.into_iter().collect(),
        )
    }
}
