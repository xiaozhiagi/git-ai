//! Report token usage from AI coding sessions to the tracker server.
//!
//! Triggered by `Stop` hooks in Claude Code and Codex after a session ends.
//! Reads the latest session data from each platform's local database and
//! uploads token usage statistics.

pub mod claude;
pub mod codex;

use crate::commands::tracker::config as tracker_config;
use serde::Serialize;

const API_PATH: &str = "/ai-code-boost/open/report/token/usage";

/// Token usage payload sent to the tracker server.
#[derive(Debug, Serialize)]
pub struct TokenUsagePayload {
    pub team_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_key: Option<String>,
    pub platform: String,
    pub session_id: String,
    pub model: String,
    pub username: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_prompts: Option<String>,
    pub reported_at: String,
}

/// Result of reading token usage from a platform's local data.
pub struct TokenUsageData {
    pub session_id: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: Option<f64>,
    pub repo_url: Option<String>,
    pub project_name: Option<String>,
    pub user_prompts: Option<String>,
}

/// Get the current user's git email for attribution.
fn get_git_username() -> String {
    // Try local, then global git config
    for scope in &["--local", "--global"] {
        if let Ok(output) = std::process::Command::new("git")
            .args([scope, "user.email"])
            .output()
        {
            if output.status.success() {
                let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !email.is_empty() {
                    return email;
                }
            }
        }
    }
    // Fallback to username from USER env var
    if let Ok(username) = std::env::var("USER") {
        if !username.is_empty() {
            return username;
        }
    }
    "unknown".to_string()
}

/// Get current repo URL if in a git repository.
fn get_repo_url() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

/// Upload token usage to the tracker server.
fn upload_usage(
    config: &tracker_config::TrackerConfig,
    usage: &TokenUsagePayload,
) -> Result<(), String> {
    let payload_str = serde_json::to_string(usage).map_err(|e| e.to_string())?;
    let url = format!("{}{}", config.tracker_url, API_PATH);

    let agent = crate::http::build_agent(None);
    let request = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("X-Team-Key", &config.team_key);

    let response = crate::http::send_with_body(request, &payload_str)?;

    if response.status_code >= 200 && response.status_code < 300 {
        tracing::debug!(
            "token usage reported successfully: session={} platform={}",
            usage.session_id,
            usage.platform
        );
        Ok(())
    } else {
        Err(format!("HTTP {}", response.status_code))
    }
}

/// Main entry point: `git-ai report-token-usage <platform>`
pub fn handle_report_token_usage(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: git-ai report-token-usage <platform>");
        eprintln!("Platforms: claude-code, codex");
        std::process::exit(1);
    }

    let platform = &args[0];

    // Load tracker config
    let config = match tracker_config::load_config() {
        Some(c) => c,
        None => {
            tracing::debug!("report-token-usage: tracker config not found, skipping");
            return;
        }
    };

    // Get user identity: prefer config username, fallback to git email
    let username = config
        .username
        .as_ref()
        .filter(|u| !u.is_empty())
        .cloned()
        .unwrap_or_else(get_git_username);
    let repo_url = get_repo_url();

    // Read token usage from platform-specific data source
    let usage_data = match platform.as_str() {
        "claude-code" => claude::read_latest_session(),
        "codex" => codex::read_latest_thread(),
        _ => {
            eprintln!("Unknown platform: {}", platform);
            eprintln!("Supported platforms: claude-code, codex");
            std::process::exit(1);
        }
    };

    let usage_data = match usage_data {
        Ok(Some(data)) => data,
        Ok(None) => {
            tracing::debug!("report-token-usage: no session data found for {}", platform);
            return;
        }
        Err(e) => {
            tracing::debug!(
                "report-token-usage: failed to read {} data: {}",
                platform,
                e
            );
            return;
        }
    };

    // Parse team_id with proper error handling
    let team_id = match config.team_id.parse::<i64>() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(
                "report-token-usage: invalid team_id '{}': {}. Skipping report.",
                config.team_id,
                e
            );
            return;
        }
    };

    // Build payload
    let payload = TokenUsagePayload {
        team_id,
        team_key: Some(config.team_key.clone()),
        platform: platform.to_string(),
        session_id: usage_data.session_id,
        model: usage_data.model,
        username,
        input_tokens: usage_data.input_tokens,
        output_tokens: usage_data.output_tokens,
        cache_read_tokens: usage_data.cache_read_tokens,
        cache_creation_tokens: usage_data.cache_creation_tokens,
        total_tokens: usage_data.total_tokens,
        cost_usd: usage_data.cost_usd,
        repo_url: usage_data.repo_url.or(repo_url),
        project_name: usage_data.project_name,
        user_prompts: usage_data.user_prompts,
        reported_at: {
            let offset = chrono::FixedOffset::east_opt(8 * 3600)
                .expect("valid offset");
            chrono::Utc::now().with_timezone(&offset).to_rfc3339()
        },
    };

    // Upload
    match upload_usage(&config, &payload) {
        Ok(()) => {
            println!(
                "[git-ai token-report] {} reported: {} tokens",
                payload.platform, payload.total_tokens
            );
        }
        Err(e) => {
            tracing::debug!("report-token-usage upload failed: {}", e);
        }
    }
}
