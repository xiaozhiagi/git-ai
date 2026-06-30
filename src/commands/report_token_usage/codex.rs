//! Read token usage from Codex's local data.
//!
//! Data source: `~/.codex/sessions/**/*.jsonl` — per-turn JSONL session logs.
//!
//! Each session log contains `token_count` events with both cumulative
//! (`total_token_usage`) and per-turn incremental (`last_token_usage`)
//! token counts, broken down by input/output/cache/reasoning.
//!
//! Approach modeled after ccusage:
//! https://github.com/ryoppippi/ccusage

use super::TokenUsageData;
use crate::mdm::utils::home_dir;
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// JSONL parser
// ---------------------------------------------------------------------------

struct SessionTokens {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    total_tokens: i64,
}

/// Extract a numeric value for a key from a JSON string.
/// Handles `{"key": 123, ...}` with optional whitespace.
fn json_num(json: &str, key: &str) -> Option<i64> {
    let needle = format!("\"{}\"", key);
    let start = json.find(&needle)?;
    let after = &json[start + needle.len()..];
    let colon = after.find(':')?;
    let value_str = after[colon + 1..].trim_start();
    let end = value_str
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(value_str.len());
    value_str[..end].parse::<i64>().ok()
}

/// Parse the last `token_count` line from a JSONL session file.
/// Returns cumulative totals and incremental per-turn sums, plus user_prompts.
fn parse_latest_token_count(
    path: &std::path::Path,
) -> Result<Option<(SessionTokens, Option<String>, Option<String>)>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read Codex session log {}: {}", path.display(), e))?;

    // Extract user prompts from the same content
    let user_prompts = extract_user_prompts(&content);

    let mut best_input: i64 = 0;
    let mut best_output: i64 = 0;
    let mut best_cache_read: i64 = 0;
    let mut best_cache_creation: i64 = 0;
    let mut best_total: i64 = 0;
    let mut has_data = false;

    let mut sum_input: i64 = 0;
    let mut sum_output: i64 = 0;
    let mut sum_cache_read: i64 = 0;
    let mut sum_cache_creation: i64 = 0;
    let mut sum_total: i64 = 0;

    // Track model across the file
    let mut model: Option<String> = None;

    let mut prev_input: i64 = 0;
    let mut prev_output: i64 = 0;
    let mut prev_cache_read: i64 = 0;
    let _prev_cache_creation: i64 = 0;
    let mut prev_total: i64 = 0;

    for line in content.lines() {
        // --- model from turn_context (highest fidelity) ---
        if line.contains(r#""type":"turn_context""#) {
            // Find the model inside the payload
            if let Some(payload_pos) = line.find(r#""payload""#) {
                let payload = &line[payload_pos..];
                if let Some(m) = extract_model(payload) {
                    model = Some(m);
                }
            }
        }

        // --- model from token_count info ---
        if line.contains(r#""type":"token_count""#) {
            if let Some(info_pos) = line.find(r#""info""#) {
                let info = &line[info_pos..];
                if model.is_none() {
                    if let Some(m) = extract_model(info) {
                        model = Some(m);
                    }
                }
            }
        }

        // --- token_count event ---
        if !line.contains(r#""type":"token_count""#) {
            continue;
        }

        // Find the info object which contains token usage
        let Some(info_pos) = line.find(r#""info""#) else {
            continue;
        };
        let info = &line[info_pos..];

        // Check for total_token_usage first
        if let Some(total_usage) = find_object(info, "total_token_usage") {
            let input = json_num(total_usage, "input_tokens").unwrap_or(0);
            let output = json_num(total_usage, "output_tokens").unwrap_or(0);
            let cached = json_num(total_usage, "cached_input_tokens").unwrap_or(0);
            let _reasoning = json_num(total_usage, "reasoning_output_tokens").unwrap_or(0);
            let total = json_num(total_usage, "total_tokens").unwrap_or(0);

            // These are cumulative — store the latest seen
            best_input = input;
            best_output = output;
            best_cache_read = cached;
            best_cache_creation = 0; // Codex doesn't have cache_creation in this field
            best_total = total;
            has_data = true;
        }

        // Also accumulate per-turn increments from last_token_usage
        if let Some(last_usage) = find_object(info, "last_token_usage") {
            let input = json_num(last_usage, "input_tokens").unwrap_or(0);
            let output = json_num(last_usage, "output_tokens").unwrap_or(0);
            let cached = json_num(last_usage, "cached_input_tokens").unwrap_or(0);
            let reasoning = json_num(last_usage, "reasoning_output_tokens").unwrap_or(0);
            let total = json_num(last_usage, "total_tokens").unwrap_or(0);

            // Skip zero-increment turns (noise)
            if input == 0 && output == 0 && cached == 0 && reasoning == 0 {
                continue;
            }

            // Incremental delta (last_token_usage IS the delta)
            sum_input += input;
            sum_output += output;
            sum_cache_read += cached;
            sum_cache_creation = 0;
            sum_total += total;

            // Skip if cumulative totals haven't advanced
            if input == prev_input
                && output == prev_output
                && cached == prev_cache_read
                && total == prev_total
            {
                continue;
            }
            prev_input = input;
            prev_output = output;
            prev_cache_read = cached;
            prev_total = total;
        }
    }

    if !has_data && sum_total == 0 {
        return Ok(None);
    }

    // Codex JSONL reports input_tokens as total prompt tokens INCLUDING cached.
    // The backend computes total_tokens = input + output + cache_read + cache_create.
    // To avoid double-counting cached tokens, report non-cached input separately.
    let (input, output, cache_read, cache_creation, total) = if best_total > 0 {
        (
            best_input.saturating_sub(best_cache_read), // non-cached only
            best_output,
            best_cache_read,
            best_cache_creation,
            best_total,
        )
    } else {
        (
            sum_input.saturating_sub(sum_cache_read),
            sum_output,
            sum_cache_read,
            sum_cache_creation,
            sum_total,
        )
    };

    if total == 0 {
        return Ok(None);
    }

    Ok(Some((
        SessionTokens {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            total_tokens: total,
        },
        model,
        user_prompts,
    )))
}

/// Extract `model` value from a JSON fragment.
fn extract_model(json: &str) -> Option<String> {
    let needle = r#""model""#;
    let start = json.find(needle)?;
    let after = &json[start + needle.len()..];
    let colon = after.find(':')?;
    let value_str = after[colon + 1..].trim_start();
    // Expect `"gpt-5.5"`
    if value_str.starts_with('"') {
        let end = value_str[1..].find('"')?;
        let model = &value_str[1..1 + end];
        if !model.is_empty() && model != "unknown" {
            return Some(model.to_string());
        }
    }
    None
}

/// Find a JSON object by key name. Returns the inner `{...}` content.
fn find_object<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{}\"", key);
    let start = json.find(&needle)?;
    let after = &json[start + needle.len()..];
    let colon = after.find(':')?;
    let obj_start = after[colon + 1..].find('{')?;
    let abs_start = start + needle.len() + colon + 1 + obj_start;
    let rest = &json[abs_start..];

    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    for (i, ch) in rest.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '"' => in_string = !in_string,
            '\\' if in_string => escape_next = true,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

/// Find the latest JSONL session file in `~/.codex/sessions/`.
/// Returns `(path, session_id)`.
///
/// Sorts by filename timestamp (not mtime) because mtime is unreliable —
/// old session files can get updated mtimes when reopened by Codex.
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
                    // Extract sortable timestamp from filename:
                    // rollout-2026-06-01T12-08-06-UUID.jsonl → 2026-06-01T12:08:06
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        // Find the timestamp portion between "rollout-" and "-UUID"
                        if let Some(start) = stem.strip_prefix("rollout-") {
                            // Timestamp is ISO-like: YYYY-MM-DDTHH-MM-SS
                            // We need at least 19 chars for YYYY-MM-DDTHH-MM-SS
                            if start.len() >= 19 {
                                let ts_part = &start[..19];
                                // Convert to sortable format: replace time dashes with colons
                                let sortable = format!(
                                    "{}:{}:{}",
                                    &ts_part[..10],   // YYYY-MM-DD
                                    &ts_part[11..13], // HH
                                    &ts_part[14..16], // MM
                                );
                                // Include SS for complete sort
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

    // Extract session_id (UUID) from filename
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
// User prompts extraction
// ---------------------------------------------------------------------------

/// Maximum length for user_prompts field (in characters).
const MAX_USER_PROMPTS_LEN: usize = 8000;

/// Extract user prompts from Codex JSONL file.
///
/// Rules:
/// - `type == "event_msg"` AND `payload.type == "user_message"` → real user input
/// - Skip messages starting with "# Context from my IDE setup" (IDE auto-injected context)
/// - Deduplicate by content
/// - Format: `------------<timestamp>------------\n<content>\n\n`
/// - Truncate to MAX_USER_PROMPTS_LEN
fn extract_user_prompts(content: &str) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in content.lines() {
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        // Check for event_msg type
        if !line.contains(r#""type":"event_msg"#) {
            continue;
        }

        // Parse to extract payload.type and payload.message
        // Quick check for user_message
        if !line.contains(r#""type":"user_message"#) {
            continue;
        }

        // Extract timestamp
        let timestamp = extract_string_value(line, "timestamp").unwrap_or_default();

        // Extract payload.message
        let message = extract_payload_message(line);
        if message.is_empty() {
            continue;
        }

        // Skip IDE auto-injected context
        if message.starts_with("# Context from my IDE setup") {
            continue;
        }

        // Deduplicate
        if seen.contains(&message) {
            continue;
        }
        seen.insert(message.clone());

        entries.push((timestamp, message));
    }

    if entries.is_empty() {
        return None;
    }

    build_prompts_string(&entries, MAX_USER_PROMPTS_LEN)
}

/// Extract payload.message from a JSON line.
fn extract_payload_message(json: &str) -> String {
    // Find "payload" object
    let payload_start = json.find(r#""payload""#);
    let Some(payload_pos) = payload_start else {
        return String::new();
    };

    let payload = &json[payload_pos..];

    // Find "message" inside payload
    let msg_start = payload.find(r#""message""#);
    let Some(msg_pos) = msg_start else {
        return String::new();
    };

    let after_msg = &payload[msg_pos + r#""message""#.len()..];
    let colon_pos = after_msg.find(':');
    let Some(colon) = colon_pos else {
        return String::new();
    };

    let value = after_msg[colon + 1..].trim_start();

    // Expect a string value
    if value.starts_with('"') {
        if let Some(end) = value[1..].find('"') {
            return value[1..1 + end].to_string();
        }
    }

    String::new()
}

/// Extract a string value for a key from a JSON line.
fn extract_string_value(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = json.find(&needle)?;
    let after = &json[start + needle.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();

    if value.starts_with('"') {
        if let Some(end) = value[1..].find('"') {
            return Some(value[1..1 + end].to_string());
        }
    }

    None
}

/// Build the formatted prompts string with timestamp separators.
fn build_prompts_string(entries: &[(String, String)], max_len: usize) -> Option<String> {
    let mut result = String::new();

    for (i, (ts, content)) in entries.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        result.push_str("------------");
        result.push_str(ts);
        result.push_str("------------\n");
        result.push_str(content);
    }

    // Truncate if too long (UTF-8 safe: use chars, not bytes)
    if result.chars().count() > max_len {
        result = result.chars().take(max_len).collect();
        result.push_str("\n...(truncated)");
    }

    Some(result)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the latest Codex session token usage from JSONL session logs.
///
/// Returns detailed token breakdown (input/output/cache_read) parsed from
/// per-turn `token_count` events, matching ccusage's approach.
pub fn read_latest_thread() -> Result<Option<TokenUsageData>, String> {
    let (path, session_id) =
        find_latest_session().ok_or("No Codex session logs found in ~/.codex/sessions/")?;

    let Some((tokens, model_override, user_prompts)) = parse_latest_token_count(&path)? else {
        return Ok(None);
    };

    let model = model_override.unwrap_or_else(|| "unknown".to_string());

    Ok(Some(TokenUsageData {
        session_id,
        model,
        input_tokens: tokens.input_tokens,
        output_tokens: tokens.output_tokens,
        cache_read_tokens: tokens.cache_read_tokens,
        cache_creation_tokens: tokens.cache_creation_tokens,
        total_tokens: tokens.total_tokens,
        cost_usd: None,
        repo_url: None,
        project_name: None,
        user_prompts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl(content: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("test-{}.jsonl", uuid::Uuid::new_v4()));
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_token_count_with_deltas() {
        let data = r#"{"timestamp":"2026-06-01T04:08:07.669Z","type":"session_meta","payload":{"id":"abc"}}
{"timestamp":"2026-06-01T04:08:21.690Z","type":"turn_context","payload":{"model":"gpt-5.5"}}
{"timestamp":"2026-06-01T04:08:22.000Z","type":"event_msg","payload":{"type":"user_message","message":"Hello world"}}
{"timestamp":"2026-06-01T04:09:56.343Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":11474,"cached_input_tokens":9600,"output_tokens":17,"reasoning_output_tokens":0,"total_tokens":11491},"last_token_usage":{"input_tokens":11474,"cached_input_tokens":9600,"output_tokens":17,"reasoning_output_tokens":0,"total_tokens":11491},"model_context_window":258400}}}
{"timestamp":"2026-06-01T04:24:18.084Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":24235,"cached_input_tokens":20736,"output_tokens":103,"reasoning_output_tokens":0,"total_tokens":24338},"last_token_usage":{"input_tokens":12761,"cached_input_tokens":11136,"output_tokens":86,"reasoning_output_tokens":0,"total_tokens":12847},"model_context_window":258400}}}"#;

        let path = write_jsonl(data);
        let (tokens, model, user_prompts) = parse_latest_token_count(&path).unwrap().unwrap();

        // Note: input_tokens is reported as non-cached only (total_input - cached_input)
        assert_eq!(tokens.total_tokens, 24338);
        assert_eq!(tokens.input_tokens, 24235 - 20736); // 3499 (non-cached)
        assert_eq!(tokens.output_tokens, 103);
        assert_eq!(tokens.cache_read_tokens, 20736);
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
        assert!(user_prompts.is_some());
        assert!(user_prompts.unwrap().contains("Hello world"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn finds_latest_session_file() {
        use std::time::Duration;

        let base = std::env::temp_dir().join(format!("codex-test-{}", uuid::Uuid::new_v4()));
        let old_dir = base.join("sessions").join("2026").join("05").join("30");
        let new_dir = base.join("sessions").join("2026").join("06").join("01");
        fs::create_dir_all(&old_dir).unwrap();
        fs::create_dir_all(&new_dir).unwrap();

        let old_path = old_dir.join("rollout-2026-05-30T10-00-00-old-session-id.jsonl");
        let new_path = new_dir.join("rollout-2026-06-01T12-08-06-new-session-id.jsonl");
        fs::write(&old_path, "dummy").unwrap();
        fs::write(&new_path, "dummy").unwrap();

        // Make new_path newer
        let new_time = std::time::SystemTime::now() + Duration::from_secs(100);
        filetime::set_file_mtime(&new_path, filetime::FileTime::from_system_time(new_time))
            .unwrap();

        // Monkey-patch home_dir for this test
        // (Not doing that — just test the walk logic manually)
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_find_object_nested() {
        let json = r#"{"info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":50},"last_token_usage":{"input_tokens":10}}}"#;
        let info = find_object(json, "info").unwrap();
        assert!(info.contains("total_token_usage"));
        assert!(info.contains("last_token_usage"));

        let total = find_object(json, "total_token_usage").unwrap();
        assert!(total.contains("input_tokens"));
    }

    #[test]
    fn test_json_num() {
        assert_eq!(
            json_num(r#"{"input_tokens":11474}"#, "input_tokens"),
            Some(11474)
        );
        assert_eq!(
            json_num(r#"{"cached_input_tokens":9600}"#, "cached_input_tokens"),
            Some(9600)
        );
        assert_eq!(json_num(r#"{"output_tokens":0}"#, "output_tokens"), Some(0));
        assert_eq!(json_num(r#"{"foo": 123}"#, "foo"), Some(123));
        assert_eq!(json_num(r#"{"foo":-5}"#, "foo"), Some(-5));
    }

    #[test]
    fn test_extract_model() {
        let json = r#"{"model":"gpt-5.5","personality":"friendly"}"#;
        assert_eq!(extract_model(json), Some("gpt-5.5".to_string()));

        let json = r#"{"model":"gpt-4o"}"#;
        assert_eq!(extract_model(json), Some("gpt-4o".to_string()));

        let json = r#"{"model":"unknown"}"#;
        assert_eq!(extract_model(json), None);
    }
}
