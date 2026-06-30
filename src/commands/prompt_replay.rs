//! Prompt replay command - semantic search for high-quality prompts
//!
//! Provides `easylife-ai prompt replay` functionality to search historical
//! high-quality prompts using semantic similarity (vector search).
//!
//! Uses team_id + team_key authentication (no EMBEDDING_API_KEY required).

use crate::commands::tracker::config::load_config;
use crate::http::{build_agent, send_with_body};
use serde::{Deserialize, Serialize};

/// Handle the `prompt replay` command
pub fn handle_prompt_replay(args: &[String]) {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // 1. Load tracker config to get tracker_url, team_id, team_key
    let tracker_config = match load_config() {
        Some(config) => config,
        None => {
            eprintln!("Error: tracker config not found, run install first");
            eprintln!("       Expected: ~/.git-ai/tracker-config.json");
            std::process::exit(1);
        }
    };

    // 2. Build API URL (new endpoint)
    let api_url = format!(
        "{}/ai-code-boost/open/report/prompt/replay",
        tracker_config.tracker_url
    );

    // 3. Build request payload with team_id + team_key authentication
    let payload = serde_json::json!({
        "team_id": tracker_config.team_id,
        "team_key": tracker_config.team_key,
        "query_text": parsed.keyword,
        "top_k": parsed.top_k,
        "sort_by": parsed.sort_by
    });

    // 4. Send HTTP request
    let agent = build_agent(Some(30));
    let request = agent
        .post(&api_url)
        .set("Content-Type", "application/json");

    let response = match send_with_body(request, &payload.to_string()) {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("Error: API request failed: {}", e);
            eprintln!("       URL: {}", api_url);
            std::process::exit(1);
        }
    };

    // 5. Check HTTP status
    if response.status_code != 200 {
        let body = response.as_str().unwrap_or("Unable to read response body");
        eprintln!("Error: API returned status {}", response.status_code);
        eprintln!("       Response: {}", body);
        std::process::exit(1);
    }

    // 6. Parse response
    let body = match response.as_str() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Error: Invalid response encoding");
            std::process::exit(1);
        }
    };

    let api_response: ApiResponse = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: Failed to parse API response: {}", e);
            eprintln!("       Response: {}", body);
            std::process::exit(1);
        }
    };

    // 7. Check API code
    if api_response.code != 200 {
        eprintln!("Error: API returned error code {}", api_response.code);
        eprintln!("       Message: {}", api_response.msg);
        std::process::exit(1);
    }

    // 8. Output
    if parsed.json {
        print_json_output(&api_response.data.results);
    } else {
        print_console_output(&api_response.data.results);
    }
}

/// Command arguments
struct ParsedArgs {
    keyword: String,
    top_k: usize,
    sort_by: String, // "score" or "time"
    json: bool,
}

/// Parse command arguments
fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    if args.is_empty() {
        return Err("Missing subcommand. Usage: prompt replay <keyword> [options]".to_string());
    }

    if args[0] != "replay" {
        return Err(format!("Unknown subcommand: {}. Expected: replay", args[0]));
    }

    if args.len() < 2 {
        return Err("Missing keyword argument. Usage: prompt replay <keyword> [options]".to_string());
    }

    let keyword = args[1].clone();
    let mut top_k = 3;
    let mut sort_by = "score".to_string();
    let mut json = false;

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--top-k" => {
                if i + 1 >= args.len() {
                    return Err("--top-k requires a value".to_string());
                }
                i += 1;
                top_k = args[i]
                    .parse::<usize>()
                    .map_err(|_| "--top-k must be a positive integer")?;
                if top_k == 0 {
                    return Err("--top-k must be greater than 0".to_string());
                }
            }
            "--sort-by" => {
                if i + 1 >= args.len() {
                    return Err("--sort-by requires a value (score or time)".to_string());
                }
                i += 1;
                let value = args[i].as_str();
                if value != "score" && value != "time" {
                    return Err(format!(
                        "--sort-by must be 'score' or 'time', got: {}",
                        value
                    ));
                }
                sort_by = value.to_string();
            }
            "--json" => {
                json = true;
            }
            arg if arg.starts_with('-') => {
                return Err(format!("Unknown option: {}", arg));
            }
            _ => {
                // Ignore positional args after keyword
            }
        }
        i += 1;
    }

    Ok(ParsedArgs {
        keyword,
        top_k,
        sort_by,
        json,
    })
}

/// API response structure (matches PromptReplayRespVO)
#[derive(Deserialize)]
struct ApiResponse {
    code: i32,
    msg: String,
    data: ResponseData,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ResponseData {
    count: i32,
    results: Vec<PromptResult>,
}

/// Single prompt result (matches PromptReplayRespVO.PromptResult)
#[derive(Deserialize, Serialize)]
struct PromptResult {
    prompt_text: String,
    quality_score: f64,
    created_at: String,
    distance: f64,
}

/// Print JSON output (--json mode)
fn print_json_output(results: &[PromptResult]) {
    println!("{}", serde_json::to_string_pretty(results).unwrap_or_else(|_| "[]".to_string()));
}

/// Print console-formatted output (default mode)
fn print_console_output(results: &[PromptResult]) {
    if results.is_empty() {
        println!("未找到相似提示词");
        return;
    }

    println!("找到 {} 条相似提示词：\n", results.len());

    for (i, result) in results.iter().enumerate() {
        // Format header with index, quality_score, and distance
        println!(
            "[{}] quality_score: {:.1} | 相似度距离: {:.3}",
            i + 1,
            result.quality_score,
            result.distance
        );

        // Format time
        println!("    时间: {}", result.created_at);

        // Truncate long prompt_text for preview
        let preview = if result.prompt_text.len() > 100 {
            format!("{}...", &result.prompt_text[..100])
        } else {
            result.prompt_text.clone()
        };

        // Replace newlines with spaces for single-line preview
        let preview_clean = preview.replace('\n', " ").replace('\r', "");

        println!("    内容: {}", preview_clean);
        println!();
    }
}

/// Print help for prompt command
pub fn print_prompt_help() {
    eprintln!("Usage: easylife-ai prompt <subcommand>");
    eprintln!();
    eprintln!("Subcommands:");
    eprintln!("  replay <keyword>    语义搜索相似高质量提示词");
    eprintln!();
    eprintln!("Options for replay:");
    eprintln!("    --top-k N            返回数量（默认 3）");
    eprintln!("    --sort-by score|time 排序方式（默认 score）");
    eprintln!("                         score: 按相似度（距离越小越相似）");
    eprintln!("                         time: 按时间降序（最新优先）");
    eprintln!("    --json               JSON 格式输出");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  easylife-ai prompt replay \"日志解析器\"");
    eprintln!("  easylife-ai prompt replay \"API 开发\" --top-k 5");
    eprintln!("  easylife-ai prompt replay \"测试\" --sort-by time");
    eprintln!("  easylife-ai prompt replay \"重构\" --json");
    eprintln!();
    eprintln!("Requirements:");
    eprintln!("  - ~/.git-ai/tracker-config.json must exist");
    eprintln!("  - team_id and team_key must be configured");
    eprintln!();
    std::process::exit(0);
}