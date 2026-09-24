//! Read token usage from Codex's local data — per-turn parsing.
//!
//! Data source: `~/.codex/sessions/**/*.jsonl` — per-turn JSONL session logs.
//!
//! Each turn is identified by `task_started` / `task_complete` events sharing
//! the same `turn_id`. User messages, assistant responses, tool calls, and
//! token counts are all associated with a turn_id.

use super::TokenUsageData;
use crate::mdm::utils::home_dir;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Per-turn data parsed from a Codex JSONL file.
struct CodexTurn {
    model: String,
    user_content: String,
    assistant_text: String,
    tool_uses_json: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    total_tokens: i64,
}

/// Parse all turns from a Codex JSONL file content.
fn parse_turns_from_content(content: &str) -> Vec<CodexTurn> {
    // Collect data per turn_id
    let mut user_messages: HashMap<String, (String, String)> = HashMap::new(); // turn_id → (timestamp, message)
    let mut assistant_messages: HashMap<String, (String, String)> = HashMap::new(); // turn_id → (timestamp, message)
    let mut token_counts: HashMap<String, (i64, i64, i64, i64, i64)> = HashMap::new(); // turn_id → (input, output, cache_read, cache_create, total)
    let mut models: HashMap<String, String> = HashMap::new(); // turn_id → model
    let mut tool_uses: HashMap<String, Vec<Value>> = HashMap::new(); // turn_id → list of tool call JSON
    let mut turn_order: Vec<String> = Vec::new(); // preserve order
    let mut aborted_turns: HashMap<String, bool> = HashMap::new(); // turn_id → is_aborted

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match event_type {
            "event_msg" => {
                let payload = v.get("payload");
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                match payload_type {
                    "task_started" => {
                        if let Some(turn_id) = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(|t| t.as_str())
                        {
                            if !user_messages.contains_key(turn_id) {
                                turn_order.push(turn_id.to_string());
                            }
                        }
                    }
                    "user_message" => {
                        let turn_id = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let message = payload
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");

                        if !message.is_empty()
                            && !message.starts_with("# Context from my IDE setup")
                        {
                            if !turn_id.is_empty() {
                                user_messages.insert(
                                    turn_id.to_string(),
                                    (timestamp.to_string(), message.to_string()),
                                );
                            }
                        }
                    }
                    "agent_message" => {
                        let turn_id = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let phase = payload
                            .and_then(|p| p.get("phase"))
                            .and_then(|p| p.as_str())
                            .unwrap_or("");
                        let message = payload
                            .and_then(|p| p.get("message"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        let timestamp = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");

                        // Only take final_answer, which is the definitive response
                        if phase == "final_answer" && !message.is_empty() && !turn_id.is_empty() {
                            assistant_messages.insert(
                                turn_id.to_string(),
                                (timestamp.to_string(), message.to_string()),
                            );
                        }
                    }
                    "token_count" => {
                        // Extract turn_id from the event
                        // Codex token_count events don't always have turn_id directly,
                        // but we can infer from context. Use the latest turn.
                        if let Some(info) = payload.and_then(|p| p.get("info")) {
                            if let Some(last_usage) = info.get("last_token_usage") {
                                let input = last_usage
                                    .get("input_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0);
                                let output = last_usage
                                    .get("output_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0);
                                let cached = last_usage
                                    .get("cached_input_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0);
                                let total = last_usage
                                    .get("total_tokens")
                                    .and_then(|v| v.as_i64())
                                    .unwrap_or(0);

                                // Associate with the latest turn_id
                                if !turn_order.is_empty() {
                                    let latest_turn = turn_order.last().unwrap().clone();
                                    token_counts
                                        .insert(latest_turn, (input, output, cached, 0, total));
                                }
                            }
                        }
                    }
                    "turn_aborted" => {
                        let turn_id = payload
                            .and_then(|p| p.get("turn_id"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if !turn_id.is_empty() {
                            aborted_turns.insert(turn_id.to_string(), true);
                        }
                    }
                    _ => {}
                }
            }
            "turn_context" => {
                let payload = v.get("payload");
                let turn_id = payload
                    .and_then(|p| p.get("turn_id"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let model = payload
                    .and_then(|p| p.get("model"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("");

                if !turn_id.is_empty() && !model.is_empty() {
                    models.insert(turn_id.to_string(), model.to_string());
                }

                // Also register the turn in order
                if !turn_id.is_empty() && !user_messages.contains_key(turn_id) {
                    turn_order.push(turn_id.to_string());
                }
            }
            "response_item" => {
                let payload = v.get("payload");
                let payload_type = payload
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");

                if payload_type == "function_call" {
                    let name = payload
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("");
                    let call_id = payload
                        .and_then(|p| p.get("call_id"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let arguments_str = payload
                        .and_then(|p| p.get("arguments"))
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}");

                    // Parse arguments JSON
                    let arguments: Value = serde_json::from_str(arguments_str)
                        .unwrap_or(Value::Object(serde_json::Map::new()));

                    let tool_entry = serde_json::json!({
                        "name": name,
                        "id": call_id,
                        "arguments": arguments,
                    });

                    // Associate with latest turn
                    if !turn_order.is_empty() {
                        let latest_turn = turn_order.last().unwrap().clone();
                        tool_uses
                            .entry(latest_turn)
                            .or_insert_with(Vec::new)
                            .push(tool_entry);
                    }
                }
            }
            _ => {}
        }
    }

    // Build turns from collected data
    let mut turns = Vec::new();
    for turn_id in &turn_order {
        // Skip aborted turns
        if aborted_turns.contains_key(turn_id) {
            continue;
        }

        let (_user_ts, user_content) = user_messages.get(turn_id).cloned().unwrap_or_default();
        let (_assistant_ts, assistant_text) =
            assistant_messages.get(turn_id).cloned().unwrap_or_default();
        let (input, output, cache_read, cache_create, total) = token_counts
            .get(turn_id)
            .cloned()
            .unwrap_or((0, 0, 0, 0, 0));
        let model = models
            .get(turn_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());

        // Build tool_uses JSON
        let tool_uses_json = tool_uses.get(turn_id).and_then(|list| {
            if list.is_empty() {
                None
            } else {
                // Add order field
                let ordered: Vec<Value> = list
                    .iter()
                    .enumerate()
                    .map(|(i, entry)| {
                        let mut obj = entry.as_object().cloned().unwrap_or_default();
                        obj.insert("order".to_string(), Value::Number((i + 1).into()));
                        Value::Object(obj)
                    })
                    .collect();
                serde_json::to_string(&ordered).ok()
            }
        });

        // Skip turns with no data
        if user_content.is_empty() && total == 0 {
            continue;
        }

        // Codex input_tokens includes cached, need to subtract
        let non_cached_input = input.saturating_sub(cache_read);

        turns.push(CodexTurn {
            model,
            user_content,
            assistant_text,
            tool_uses_json,
            input_tokens: non_cached_input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_create,
            total_tokens: total,
        });
    }

    turns
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

/// Find the latest JSONL session file in `~/.codex/sessions/`.
fn find_latest_session() -> Option<(PathBuf, String)> {
    let sessions_dir = home_dir().join(".codex").join("sessions");
    if !sessions_dir.exists() {
        return None;
    }

    let mut latest: Option<(String, PathBuf)> = None;

    fn walk(dir: &std::path::Path, latest: &mut Option<(String, PathBuf)>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, latest);
                } else if path.extension().is_some_and(|e| e == "jsonl") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Some(start) = stem.strip_prefix("rollout-") {
                            if start.len() >= 19 {
                                let ts_part = &start[..19];
                                let sortable = format!(
                                    "{}:{}:{}",
                                    &ts_part[..10],
                                    &ts_part[11..13],
                                    &ts_part[14..16],
                                );
                                let full_sort = if ts_part.len() >= 19 {
                                    format!("{}:{}", sortable, &ts_part[17..19])
                                } else {
                                    sortable
                                };
                                match latest {
                                    None => *latest = Some((full_sort, path)),
                                    Some((best_ts, _)) if full_sort > *best_ts => {
                                        *latest = Some((full_sort, path));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    walk(&sessions_dir, &mut latest);

    let Some((_, path)) = latest else {
        return None;
    };

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|stem| {
            if let Some(dash_pos) = stem.find("-019") {
                let uuid_start = &stem[dash_pos + 1..];
                if uuid_start.len() >= 36 {
                    return Some(uuid_start[..36].to_string());
                }
            }
            Some(stem.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    Some((path, session_id))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse all turns from the latest Codex session.
///
/// Returns (jsonl_file_path, turns_with_indices_assigned).
pub fn parse_turns() -> Result<Option<(String, Vec<TokenUsageData>)>, String> {
    let (path, session_id) =
        find_latest_session().ok_or("No Codex session logs found in ~/.codex/sessions/")?;

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let parsed = parse_turns_from_content(&content);

    if parsed.is_empty() {
        return Ok(None);
    }

    let path_str = path.to_string_lossy().to_string();

    let mut turns_data = Vec::new();
    for (i, turn) in parsed.iter().enumerate() {
        turns_data.push(TokenUsageData {
            session_id: session_id.clone(),
            turn_index: (i + 1) as i64,
            model: turn.model.clone(),
            input_tokens: turn.input_tokens,
            output_tokens: turn.output_tokens,
            cache_read_tokens: turn.cache_read_tokens,
            cache_creation_tokens: turn.cache_creation_tokens,
            total_tokens: turn.total_tokens,
            cost_usd: None,
            repo_url: None,
            project_name: None,
            user_prompts: if turn.user_content.is_empty() {
                None
            } else {
                Some(turn.user_content.clone())
            },
            assistant_responses: if turn.assistant_text.is_empty() {
                None
            } else {
                Some(turn.assistant_text.clone())
            },
            tool_uses: turn.tool_uses_json.clone(),
        });
    }

    tracing::debug!(
        "report-token-usage: parsed {} turns from Codex session {}",
        turns_data.len(),
        session_id
    );

    Ok(Some((path_str, turns_data)))
}
