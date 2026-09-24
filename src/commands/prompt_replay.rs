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
            eprintln!("       Expected: ~/.easylife-ai/tracker-config.json");
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
    let request = agent.post(&api_url).set("Content-Type", "application/json");

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
    } else if parsed.markdown {
        print_markdown_output(&api_response.data.results);
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
    markdown: bool,
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
        return Err(
            "Missing keyword argument. Usage: prompt replay <keyword> [options]".to_string(),
        );
    }

    let keyword = args[1].clone();
    let mut top_k = 3;
    let mut sort_by = "score".to_string();
    let mut json = false;
    let mut markdown = true; // 默认使用 ANSI 终端渲染

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
                if markdown && !args.iter().any(|a| a == "--plain") {
                    // 允许 --json 和 --plain 同时使用（--plain 会被忽略）
                }
                json = true;
            }
            "--markdown" => {
                markdown = true;
            }
            "--plain" => {
                if json {
                    return Err("--json and --plain are mutually exclusive".to_string());
                }
                markdown = false;
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
        markdown,
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
    println!(
        "{}",
        serde_json::to_string_pretty(results).unwrap_or_else(|_| "[]".to_string())
    );
}

/// Print console-formatted output (default mode, full content, no truncation)
fn print_console_output(results: &[PromptResult]) {
    if results.is_empty() {
        println!("未找到相似提示词");
        return;
    }

    println!("找到 {} 条相似提示词：\n", results.len());

    for (i, result) in results.iter().enumerate() {
        println!(
            "[{}] quality_score: {:.1} | 相似度距离: {:.3}",
            i + 1,
            result.quality_score,
            result.distance
        );
        println!("    时间: {}", result.created_at);
        println!("    内容: {}", result.prompt_text);
        println!();
    }
}

// ── ANSI 终端渲染 (markdown 模式) ──────────────────────────────────

const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// 代码块背景色（深色主题）
const CODE_BG: &str = "\x1b[48;5;236m"; // 深灰背景
const CODE_FG: &str = "\x1b[38;5;252m"; // 浅灰前景
const CODE_RESET: &str = "\x1b[0m"; // 重置所有属性

/// 获取终端宽度（默认 80）
fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
}

/// 用背景色铺满整行：填充空格到终端宽度
fn pad_line_to_terminal_width(line: &str) -> String {
    let width = terminal_width();
    let char_len = line.chars().count();
    if char_len >= width {
        return line.to_string();
    }
    let padding = width - char_len;
    format!("{}{:padding$}", line, "", padding = padding)
}

// ── prompt_text 三段式解析 ──────────────────────────────────────────

/// 解析后的 prompt 三段
struct PromptSections {
    user_query: String,
    tool_calls: Vec<ToolCall>,
    assistant_responses: String,
}

/// 工具调用条目 — arguments 使用动态 JSON 兼容任意工具类型
#[derive(Deserialize, Debug)]
struct ToolCall {
    order: i32,
    name: String,
    arguments: serde_json::Value,
    #[allow(dead_code)]
    id: Option<String>,
    #[allow(dead_code)]
    result: Option<String>,
}

/// 解析 prompt_text 为三段式
fn parse_prompt_text(text: &str) -> PromptSections {
    // 1. 分割 user_query 和剩余部分
    let (user_query, rest) = if let Some(idx) = text.find("\n\ntool_use: ") {
        let query = text[..idx].trim();
        let rest = &text[idx + "\n\ntool_use: ".len()..];
        (query.to_string(), rest.to_string())
    } else {
        // 没有 tool_use，直接返回
        return PromptSections {
            user_query: text.trim().to_string(),
            tool_calls: vec![],
            assistant_responses: String::new(),
        };
    };

    // 2. 分割 tool_use JSON 和 assistant_responses
    let (tool_use_json, assistant_responses) =
        if let Some(idx) = rest.find("\n\nassistant_responses: ") {
            let tu = &rest[..idx];
            let ar = rest[idx + "\n\nassistant_responses: ".len()..].trim();
            (tu.to_string(), ar.to_string())
        } else {
            // 没有 assistant_responses
            (rest, String::new())
        };

    // 3. 解析 tool_use JSON 数组
    let tool_calls = serde_json::from_str::<Vec<ToolCall>>(&tool_use_json).unwrap_or_default();

    PromptSections {
        user_query,
        tool_calls,
        assistant_responses,
    }
}

/// 按工具名分支渲染单个工具调用 — 统一格式：遍历 arguments 所有字段
fn format_tool_call(tc: &ToolCall) -> String {
    let mut md = String::new();

    md.push_str(&format!("\n### {} {}\n", tc.order, tc.name));

    if let Some(obj) = tc.arguments.as_object() {
        for (key, value) in obj {
            let value_str = match value {
                serde_json::Value::String(s) => s.clone(),
                _ => value.to_string(),
            };
            md.push_str(&format!("\n{}:\n", key));
            md.push_str(&format!("````\n{}\n````", value_str));
        }
    }

    md
}

/// 将 PromptSections 组装为标准 markdown 字符串
fn format_prompt_sections(sections: &PromptSections) -> String {
    let mut md = String::new();

    // 用户输入
    md.push_str("## 用户输入\n\n");
    md.push_str(&sections.user_query);

    // 工具使用
    if !sections.tool_calls.is_empty() {
        md.push_str("\n\n## 相关工具使用\n");
        for tc in &sections.tool_calls {
            md.push_str(&format_tool_call(tc));
        }
    }

    // 最终回复
    if !sections.assistant_responses.is_empty() {
        md.push_str("\n\n## 最终回复\n\n");
        md.push_str(&sections.assistant_responses);
    }

    md
}

/// 将 markdown 文本渲染为终端 ANSI 格式输出
fn render_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut in_code_block = false;

    for line in text.lines() {
        // 代码块切换 (``` 或 ````)
        if line.trim().starts_with("```") {
            if in_code_block {
                // 关闭代码块：重置所有属性
                output.push_str(&format!("{}\n", CODE_RESET));
                in_code_block = false;
            } else {
                output.push('\n');
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            // 代码块内容：深色背景 + 浅色文字，填充到终端宽度
            let padded = pad_line_to_terminal_width(line);
            output.push_str(&format!("{}{}{}{}\n", CODE_BG, CODE_FG, padded, CODE_RESET));
            continue;
        }

        // 分隔线
        if line.trim() == "---" {
            output.push_str(&format!(
                "{}─────────────────────────────────────────────────{}{}\n",
                DIM, RESET, RESET
            ));
            continue;
        }

        // 表格行: | col | col |
        if line.trim().starts_with('|') && line.trim().ends_with('|') {
            let trimmed = line.trim();
            let inner = &trimmed[1..trimmed.len() - 1];
            let cells: Vec<&str> = inner.split('|').map(|c| c.trim()).collect();

            // 判断是否为分隔行 (|---|---|)
            let is_separator = cells
                .iter()
                .all(|c| c.chars().all(|ch| ch == '-' || ch == ':'));
            if is_separator {
                let sep_width = cells.iter().map(|c| c.len()).max().unwrap_or(10) * cells.len()
                    + cells.len() * 3;
                output.push_str(&format!("{}  │{}{}\n", DIM, "─".repeat(sep_width), RESET));
            } else {
                let rendered_cells: Vec<String> = cells.iter().map(|c| render_inline(c)).collect();
                let row = rendered_cells.join(&format!("{} │ {}{}", DIM, RESET, DIM));
                output.push_str(&format!("{}  │ {}{}{}\n", DIM, row, RESET, RESET));
            }
            continue;
        }

        // 标题: # ## ### #### (1-4 级)
        let (level, header_text) = if let Some(t) = line.strip_prefix("#### ") {
            (4, t)
        } else if let Some(t) = line.strip_prefix("### ") {
            (3, t)
        } else if let Some(t) = line.strip_prefix("## ") {
            (2, t)
        } else if let Some(t) = line.strip_prefix("# ") {
            (1, t)
        } else {
            (0, "")
        };

        if level > 0 {
            let rendered = render_inline(header_text);
            // 1-3级: 粗体+青色, 4级: 仅粗体
            let style = match level {
                1 | 2 | 3 => format!("{}{}{}", BOLD, CYAN, rendered),
                _ => format!("{}{}", BOLD, rendered),
            };
            output.push_str(&format!("{}{}{}\n", style, RESET, ""));
            continue;
        }

        // 列表: - text
        if let Some(body) = line.strip_prefix("- ") {
            let rendered = render_inline(body);
            output.push_str(&format!("  {}•{} {}\n", CYAN, RESET, rendered));
            continue;
        }

        // 普通行 — 渲染行内格式
        let rendered = render_inline(line);
        if rendered.is_empty() {
            output.push('\n');
        } else {
            output.push_str(&rendered);
            output.push('\n');
        }
    }

    // 关闭未闭合的代码块
    if in_code_block {
        output.push_str(&format!("{}\n", CODE_RESET));
    }

    output
}

/// 渲染行内 markdown: **bold**, `code`
fn render_inline(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 16);
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        // 粗体: **text**
        if c == '*' {
            if chars.peek() == Some(&'*') {
                chars.next();
                let mut bold_buf = String::new();
                let mut found = false;
                while let Some(cc) = chars.next() {
                    if cc == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        found = true;
                        break;
                    }
                    bold_buf.push(cc);
                }
                if found {
                    result.push_str(&format!("{}{}{}", BOLD, bold_buf, RESET));
                } else {
                    result.push_str("**");
                    result.push_str(&bold_buf);
                }
                continue;
            }
        }

        // 行内代码: `text`
        if c == '`' {
            let mut code_buf = String::new();
            let mut found = false;
            while let Some(cc) = chars.next() {
                if cc == '`' {
                    found = true;
                    break;
                }
                code_buf.push(cc);
            }
            if found {
                result.push_str(&format!("{}{}{}", GREEN, code_buf, RESET));
            } else {
                result.push('`');
                result.push_str(&code_buf);
            }
            continue;
        }

        result.push(c);
    }

    result
}

/// Print markdown-formatted output (--markdown mode, rendered to terminal)
fn print_markdown_output(results: &[PromptResult]) {
    if results.is_empty() {
        println!("未找到相似提示词");
        return;
    }

    println!("{}找到 {} 条相似提示词：{}", BOLD, results.len(), RESET);
    println!();

    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            println!();
        }

        // 标题行
        println!(
            "{}{}[{}] quality_score: {:.1} | 相似度距离: {:.3}{}",
            BOLD,
            CYAN,
            i + 1,
            result.quality_score,
            result.distance,
            RESET
        );

        // 时间
        println!("{}时间{}: {}", BOLD, RESET, result.created_at);

        // 内容 — 先解析三段式，再渲染
        println!("{}内容{}:", BOLD, RESET);

        let sections = parse_prompt_text(&result.prompt_text);
        let markdown = format_prompt_sections(&sections);
        print!("{}", render_markdown(&markdown));
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
    eprintln!("    --plain              纯文本输出（无 ANSI 渲染）");
    eprintln!("    --markdown           与默认模式相同（保留兼容）");
    eprintln!("    --json 和 --plain 互斥，不能同时使用");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  easylife-ai prompt replay \"日志解析器\"");
    eprintln!("  easylife-ai prompt replay \"API 开发\" --top-k 5");
    eprintln!("  easylife-ai prompt replay \"测试\" --sort-by time");
    eprintln!("  easylife-ai prompt replay \"重构\" --json");
    eprintln!("  easylife-ai prompt replay \"日志解析器\" --top-k 5 --sort-by time --plain");
    eprintln!();
    eprintln!("Requirements:");
    eprintln!("  - ~/.easylife-ai/tracker-config.json must exist");
    eprintln!("  - team_id and team_key must be configured");
    eprintln!();
    std::process::exit(0);
}
