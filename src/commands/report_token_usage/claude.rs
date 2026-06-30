//! Read token usage from Claude Code's local data.
//!
//! Data source: `~/.claude/projects/<project>/<session>.jsonl`
//! Each JSONL line contains a `message.usage` object with token counts.
//! This is the same approach used by the `ccusage` project.
//!
//! File structure:
//!   - `~/.claude/projects/<project>/<session_id>.jsonl`  (main session)
//!   - `~/.claude/projects/<project>/<session_id>/subagents/<agent>.jsonl`  (sub-agents)

use super::TokenUsageData;
use crate::mdm::utils::home_dir;
use serde::Deserialize;

/// Usage data from a single JSONL line.
#[derive(Debug, Deserialize)]
struct JsonlEntry {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<JsonlMessage>,
    cost_usd: Option<f64>,
    #[serde(rename = "costUSD")]
    cost_usd_alt: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct JsonlMessage {
    usage: Option<JsonlUsage>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonlUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
}

/// User message entry for prompt extraction.
#[derive(Debug, Deserialize)]
struct UserMessageEntry {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<UserMessage>,
}

#[derive(Debug, Deserialize)]
struct UserMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

/// Maximum length for user_prompts field (in characters).
const MAX_USER_PROMPTS_LEN: usize = 8000;

/// Extract user prompts from Claude JSONL file.
/// 
/// Rules:
/// - `role == "user"` AND `content` is STRING (not LIST) → real user input
/// - Skip messages starting with "This session is being continued" (context continuation)
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

        // Parse the line
        let entry: UserMessageEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let Some(msg) = &entry.message else { continue };
        let Some(role) = &msg.role else { continue };
        if role != "user" {
            continue;
        }

        // Key filter: content must be STRING (not LIST with tool_result)
        let Some(content_value) = &msg.content else { continue };
        let content_str = match content_value.as_str() {
            Some(s) => s,
            None => continue, // LIST or other type → skip
        };

        // Skip context continuation messages
        if content_str.starts_with("This session is being continued") {
            continue;
        }

        // Deduplicate
        if seen.contains(content_str) {
            continue;
        }
        seen.insert(content_str.to_string());

        // Get timestamp (default to empty if not present)
        let timestamp = entry.timestamp.clone().unwrap_or_default();

        entries.push((timestamp, content_str.to_string()));
    }

    if entries.is_empty() {
        return None;
    }

    // Build the formatted string
    build_prompts_string(&entries, MAX_USER_PROMPTS_LEN)
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

/// Aggregated token usage for a single session file.
struct SessionAggregate {
    session_id: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    total_tokens: i64,
    cost_usd: Option<f64>,
    /// Project name extracted from file path (e.g. `-Users-xz-xm-demo` → `xm/demo`).
    project_name: Option<String>,
    /// File modification time, for picking the "latest" session.
    mtime_ms: i64,
    /// User prompts extracted from the session (multi-round, timestamp-separated).
    user_prompts: Option<String>,
}

/// Extract project name from a Claude JSONL file path.
/// The path format is `~/.claude/projects/<encoded_project>/<session>.jsonl`
/// where `<encoded_project>` is typically `-Users-xz-xm-project-name`.
/// Returns the project name with dashes replaced by slashes.
fn extract_project_name(path: &std::path::Path) -> Option<String> {
    let components: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();

    // Find "projects" component, then the next one is the project name
    for (i, comp) in components.iter().enumerate() {
        if comp == "projects" && i + 1 < components.len() {
            let raw = &components[i + 1];
            // Convert `-Users-xz-xm-demo` → `xm/demo` (skip `-Users-` prefix)
            if let Some(rest) = raw.strip_prefix("-Users-") {
                // Remove leading username segment (first part after -Users-)
                let parts: Vec<&str> = rest.split('-').collect();
                if parts.len() >= 2 {
                    // Skip username, join remaining with /
                    return Some(parts[1..].join("/"));
                }
            }
            return Some(raw.clone());
        }
    }
    None
}

/// Extract the session ID from a JSONL file path.
/// For main sessions: `projects/<project>/<session_id>.jsonl` → `<session_id>`
/// For subagents: `projects/<project>/<session_id>/subagents/<agent>.jsonl` → `<session_id>`
fn extract_session_id(path: &std::path::Path) -> Option<String> {
    let components: Vec<&std::ffi::OsStr> = path.components().map(|c| c.as_os_str()).collect();

    // Walk backwards looking for "subagents"
    for (i, comp) in components.iter().enumerate() {
        if comp.to_string_lossy() == "subagents" {
            // Session ID is the parent directory of "subagents"
            if i > 0 {
                return Some(components[i - 1].to_string_lossy().to_string());
            }
        }
    }

    // Main session: filename without .jsonl extension
    if let Some(file_name) = path.file_stem() {
        let name = file_name.to_string_lossy().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    None
}

/// Try to aggregate token usage from a single JSONL file.
fn aggregate_session_file(path: &std::path::Path) -> Option<SessionAggregate> {
    let content = std::fs::read_to_string(path).ok()?;

    let session_id_from_path = extract_session_id(path)?;
    let mut input_tokens: i64 = 0;
    let mut output_tokens: i64 = 0;
    let mut cache_read_tokens: i64 = 0;
    let mut cache_creation_tokens: i64 = 0;
    let mut model: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut total_cost: Option<f64> = None;

    // Extract user prompts from the same content
    let user_prompts = extract_user_prompts(&content);

    for line in content.lines() {
        // Fast path: skip lines without "usage" key
        if !line.contains(r#""usage""#) {
            continue;
        }

        let entry: JsonlEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let Some(msg) = &entry.message else { continue };
        let Some(usage) = &msg.usage else { continue };

        input_tokens += usage.input_tokens;
        output_tokens += usage.output_tokens;
        cache_read_tokens += usage.cache_read_input_tokens;
        cache_creation_tokens += usage.cache_creation_input_tokens;
        if model.is_none() {
            model = msg.model.clone();
        }
        if session_id.is_none() {
            session_id = entry.session_id.clone();
        }

        // Accumulate cost
        let cost = entry.cost_usd.or(entry.cost_usd_alt).unwrap_or(0.0);
        total_cost = Some(total_cost.unwrap_or(0.0) + cost);
    }

    let total = input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens;
    if total == 0 {
        return None;
    }

    let mtime_ms = path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    Some(SessionAggregate {
        session_id: session_id.unwrap_or(session_id_from_path),
        model: model.unwrap_or_else(|| "unknown".to_string()),
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        total_tokens: total,
        cost_usd: total_cost,
        project_name: extract_project_name(path),
        mtime_ms,
        user_prompts,
    })
}

/// Recursively find all .jsonl files under the projects directory.
fn find_jsonl_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            files.push(path);
        } else if file_type.is_dir() {
            find_jsonl_files(&path, files);
        }
    }
}

/// Find Claude project directories.
fn find_claude_project_dirs() -> Vec<std::path::PathBuf> {
    let home = home_dir();
    let mut dirs = Vec::new();

    // ~/.claude
    let claude_dir = home.join(".claude").join("projects");
    if claude_dir.is_dir() {
        dirs.push(claude_dir);
    }

    // ~/.config/claude (XDG)
    let xdg_dir = home.join(".config").join("claude").join("projects");
    if xdg_dir.is_dir() {
        dirs.push(xdg_dir);
    }

    // CLAUDE_CONFIG_DIR env var
    if let Ok(env_paths) = std::env::var("CLAUDE_CONFIG_DIR") {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let path = std::path::PathBuf::from(raw).join("projects");
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }

    dirs
}

/// Read the latest Claude Code session token usage.
/// Scans `~/.claude/projects/**/*.jsonl`, aggregates per session, returns the most recent.
pub fn read_latest_session() -> Result<Option<TokenUsageData>, String> {
    let project_dirs = find_claude_project_dirs();
    if project_dirs.is_empty() {
        tracing::debug!("report-token-usage: no Claude project directories found");
        return Ok(None);
    }

    let mut all_sessions = Vec::new();

    for dir in &project_dirs {
        let mut jsonl_files = Vec::new();
        find_jsonl_files(dir, &mut jsonl_files);

        for file in &jsonl_files {
            if let Some(agg) = aggregate_session_file(file) {
                all_sessions.push(agg);
            }
        }
    }

    if all_sessions.is_empty() {
        tracing::debug!("report-token-usage: no Claude session data with token usage found");
        return Ok(None);
    }

    // Pick the session with the most recent file modification time
    all_sessions.sort_by_key(|s| s.mtime_ms);
    let latest = all_sessions.last().unwrap();

    tracing::debug!(
        "report-token-usage: found latest session {} with {} tokens (model: {})",
        latest.session_id,
        latest.total_tokens,
        latest.model
    );

    Ok(Some(TokenUsageData {
        session_id: latest.session_id.clone(),
        model: latest.model.clone(),
        input_tokens: latest.input_tokens,
        output_tokens: latest.output_tokens,
        cache_read_tokens: latest.cache_read_tokens,
        cache_creation_tokens: latest.cache_creation_tokens,
        total_tokens: latest.total_tokens,
        cost_usd: latest.cost_usd,
        repo_url: None,
        project_name: latest.project_name.clone(),
        user_prompts: latest.user_prompts.clone(),
    }))
}
