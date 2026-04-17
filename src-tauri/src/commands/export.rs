use crate::storage;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExportRange {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "range")]
    Range { from_seq: u64, to_seq: u64 },
    #[serde(rename = "messages")]
    Messages { seqs: Vec<u64> },
}

#[tauri::command]
pub fn export_conversation(run_id: String) -> Result<String, String> {
    export_conversation_markdown(run_id, ExportRange::Full)
}

#[tauri::command]
pub fn export_conversation_markdown(
    run_id: String,
    range: ExportRange,
) -> Result<String, String> {
    log::debug!(
        "[export] export_conversation_markdown: run_id={}, range={:?}",
        run_id,
        range
    );

    let run = storage::runs::get_run(&run_id)
        .ok_or_else(|| format!("Run {} not found", run_id))?;

    let mut md = String::new();
    let title = run.name.as_deref().unwrap_or(&run_id);
    md.push_str(&format!("# {}\n\n", title));

    if let Some(model) = &run.model {
        md.push_str(&format!("**Model:** {}\n\n", model));
    }
    if !run.cwd.is_empty() {
        md.push_str(&format!("**Working directory:** `{}`\n\n", run.cwd));
    }
    let started = &run.started_at;
    md.push_str(&format!("**Started:** {}\n\n---\n\n", started));

    // Try bus events first, fall back to legacy
    // list_bus_events returns the inner event objects (not the _bus envelope)
    let bus_events = storage::events::list_bus_events(&run_id, None);
    if !bus_events.is_empty() {
        let mut turns: Vec<(u64, String, String)> = Vec::new(); // (seq, role, text)
        let mut pending_tool_name: Option<String> = None;

        for evt in &bus_events {
            // list_bus_events injects _seq into each event
            let seq = evt
                .get("_seq")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Match by range filter
            if !matches_range(seq, &range) {
                continue;
            }

            let event_type = evt
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match event_type {
                "user_message" => {
                    let text = extract_text(evt);
                    if !text.is_empty() {
                        turns.push((seq, "User".into(), text));
                    }
                }
                "message_complete" => {
                    let text = extract_text(evt);
                    if !text.is_empty() {
                        turns.push((seq, "Assistant".into(), text));
                    }
                }
                "tool_start" => {
                    pending_tool_name = evt
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                "tool_end" => {
                    if let Some(tool_name) = pending_tool_name.take() {
                        let output_preview = evt
                            .get("output_preview")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !output_preview.is_empty() {
                            turns.push((
                                seq,
                                "Tool".into(),
                                format!("**Tool: {}**\n```\n{}\n```", tool_name, output_preview),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        // Sort by seq ascending
        turns.sort_by_key(|(seq, _, _)| *seq);

        for (_, role, text) in turns {
            md.push_str(&format!("## {}\n\n{}\n\n---\n\n", role, text));
        }
    } else {
        // Fallback to legacy events
        let events = storage::events::list_events(&run_id, 0);
        for event in events {
            let seq = event.seq;
            if !matches_range(seq, &range) {
                continue;
            }
            let type_str = format!("{}", event.event_type);
            if type_str != "user" && type_str != "assistant" {
                continue;
            }
            let text = event
                .payload
                .get("text")
                .or_else(|| event.payload.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.is_empty() {
                continue;
            }
            let role = if type_str == "user" {
                "User"
            } else {
                "Assistant"
            };
            md.push_str(&format!("## {}\n\n{}\n\n---\n\n", role, text));
        }
    }

    if md.trim().ends_with("---") {
        let _ = md.trim_end_matches('-').trim_end_matches('\n');
    }

    Ok(md)
}

fn matches_range(seq: u64, range: &ExportRange) -> bool {
    match range {
        ExportRange::Full => true,
        ExportRange::Range { from_seq, to_seq } => seq >= *from_seq && seq <= *to_seq,
        ExportRange::Messages { seqs } => seqs.contains(&seq),
    }
}

fn extract_text(event_data: &serde_json::Value) -> String {
    event_data
        .get("text")
        .or_else(|| event_data.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[tauri::command]
pub async fn write_export_file(path: String, content: String) -> Result<(), String> {
    log::debug!(
        "[export] write_export_file: path={}, content_len={}",
        path,
        content.len()
    );

    let ext = Path::new(&path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html") | Some("htm") | Some("md") => {}
        _ => {
            return Err("write_export_file: only .html/.htm/.md paths allowed".into());
        }
    }

    if let Some(parent) = Path::new(&path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create dir: {}", e))?;
    }

    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("Failed to write: {}", e))
}
