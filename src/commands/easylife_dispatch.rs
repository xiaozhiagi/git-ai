//! easylife-ai fork-specific subcommands, dispatched from one isolated arm in
//! `git_ai_handlers.rs`.
//!
//! Everything easylife-ai adds on top of the upstream git-ai command set lives
//! here so future upstream merges touch this fork in a single, well-marked
//! place. See `docs/easylife-integration-points.md` for the full list of
//! integration points.

use crate::commands;

pub const EASYLIFE_COMMANDS: &[&str] = &[
    "backup",
    "easylife-uninstall",
    "flush-cas",
    "share",
    "sync-prompts",
    "prompts",
    "prompt",
    "search",
    "continue",
    "tracker",
    "report-token-usage",
    "ops-check",
    "doctor",
    "legacy-cleanup",
];

pub fn is_easylife_command(name: &str) -> bool {
    EASYLIFE_COMMANDS.contains(&name)
}

/// Handle one easylife-ai subcommand. Only called with names that returned
/// `true` from `is_easylife_command`.
pub fn handle(name: &str, args: &[String]) {
    match name {
        "backup" => {
            if let Err(e) = commands::backup::run(args) {
                eprintln!("Backup failed: {}", e);
                std::process::exit(1);
            }
        }
        // easylife-ai: named `easylife-uninstall` so upstream keeps `uninstall`
        // as its `uninstall-hooks` alias without clashing with the fork's full
        // uninstaller (which also stops the daemon).
        "easylife-uninstall" => {
            if let Err(e) = commands::uninstall::run(args) {
                eprintln!("Uninstall failed: {}", e);
                std::process::exit(1);
            }
        }
        "flush-cas" => {
            commands::flush_cas::handle_flush_cas(args);
        }
        "share" => {
            commands::share::handle_share(args);
        }
        "sync-prompts" => {
            commands::sync_prompts::handle_sync_prompts(args);
        }
        "prompts" => {
            commands::prompts_db::handle_prompts(args);
        }
        "prompt" => {
            commands::prompt_replay::handle_prompt_replay(args);
        }
        "search" => {
            commands::search::handle_search(args);
        }
        "continue" => {
            commands::continue_session::handle_continue(args);
        }
        "tracker" => {
            handle_tracker(args);
        }
        "report-token-usage" => {
            commands::report_token_usage::handle_report_token_usage(args);
        }
        "ops-check" => {
            commands::ops::handle_ops(args);
        }
        "doctor" => {
            commands::doctor::handle_doctor(args);
        }
        "legacy-cleanup" => {
            commands::legacy_cleanup::handle_legacy_cleanup(args);
        }
        _ => unreachable!("not an easylife command: {name}"),
    }
}

/// Help entries for all easylife-ai subcommands, appended to the main help
/// output by a single call from `print_help()`.
pub fn print_help_section() {
    eprintln!("  backup             Create, list, or restore a client backup");
    eprintln!("  easylife-uninstall Remove easylife-ai and managed hooks (also stops the daemon)");
    eprintln!("  prompts            Create local SQLite database for prompt analysis");
    eprintln!("    --since <time>        Only include prompts after this time (default: 30d)");
    eprintln!("    --author <name>       Filter by human author (default: current git user)");
    eprintln!("    --all-authors         Include prompts from all authors");
    eprintln!("    exec \"<SQL>\"          Execute arbitrary SQL on prompts.db");
    eprintln!("    list                  List prompts as TSV");
    eprintln!("    next                  Get next prompt as JSON (iterator pattern)");
    eprintln!("    reset                 Reset iteration pointer to start");
    eprintln!("  prompt             Prompt 管理命令");
    eprintln!("    replay <keyword>      语义搜索相似高质量提示词");
    eprintln!("      --top-k N           返回数量（默认 3）");
    eprintln!("      --sort-by score|time 排序方式（默认 score）");
    eprintln!("      --json              JSON 格式输出");
    eprintln!("  search             Search AI prompt history");
    eprintln!("    --commit <rev>        Search by commit (SHA, branch, tag, symbolic ref)");
    eprintln!("    --file <path>         Search by file path");
    eprintln!("    --lines <start-end>   Limit to line range (requires --file; repeatable)");
    eprintln!("    --pattern <text>      Full-text search in prompt messages");
    eprintln!("    --prompt-id <id>      Look up specific prompt");
    eprintln!("    --tool <name>         Filter by AI tool (claude, cursor, etc.)");
    eprintln!("    --author <name>       Filter by human author");
    eprintln!("    --since <time>        Only prompts after this time");
    eprintln!("    --until <time>        Only prompts before this time");
    eprintln!("    --json                Output as JSON");
    eprintln!("    --verbose             Include full transcripts");
    eprintln!("    --porcelain           Stable machine-parseable format");
    eprintln!("    --count               Just show result count");
    eprintln!("  continue           Restore AI session context and launch agent");
    eprintln!("    --commit <rev>        Continue from a specific commit");
    eprintln!("    --file <path>         Continue from a specific file");
    eprintln!("    --lines <start-end>   Limit to line range (requires --file)");
    eprintln!("    --prompt-id <id>      Continue from a specific prompt");
    eprintln!("    --agent <name>        Select agent (claude, cursor; default: claude)");
    eprintln!("    --launch              Launch agent CLI with restored context");
    eprintln!("    --clipboard           Copy context to system clipboard");
    eprintln!("    --json                Output context as structured JSON");
    eprintln!("  report-token-usage   Report AI session token usage to tracker");
    eprintln!("    <platform>            Platform: claude-code, codex");
    eprintln!(
        "  ops-check          Print a prompt for an AI agent to troubleshoot via the ops skill"
    );
    eprintln!("    [question...]         Your question; omit for a placeholder");
    eprintln!("    --skill, -s <name>    Use a specific skill (default: tracker-troubleshoot)");
    eprintln!("    --list, -l            List installed ops skills");
    eprintln!(
        "  doctor             End-to-end self check of the reporting pipelines (really uploads)"
    );
    eprintln!("    --repo-url <url>      Verify against a real repo instead of a local one");
    eprintln!("    --no-fix              Only report missing hooks/plugins, do not repair");
    eprintln!("    --keep                Keep the temporary repository for inspection");
    eprintln!("    --json                Print a machine-readable summary");
    eprintln!("  legacy-cleanup     Report or remove a leftover ~/.git-ai install");
    eprintln!("    (no flag)             Report only; changes nothing");
    eprintln!("    --yes                 Stop the old background service and rename the directory");
    eprintln!("    --purge               Delete an already-renamed legacy directory");
}

fn handle_tracker(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: easylife-ai tracker <subcommand>");
        eprintln!("Subcommands:");
        eprintln!("  retry              Process retry queue");
        eprintln!("  log [-n <lines>]   Show upload log (default: 100 lines)");
        eprintln!("  blacklist list     List blacklist patterns");
        eprintln!("  blacklist add <pattern>    Add pattern to blacklist");
        eprintln!("  blacklist remove <pattern> Remove pattern from blacklist");
        std::process::exit(1);
    }

    match args[0].as_str() {
        "retry" => {
            let config = match commands::tracker::config::load_config() {
                Some(c) => c,
                None => {
                    eprintln!("tracker config not found at ~/.easylife-ai/tracker-config.json");
                    std::process::exit(1);
                }
            };
            match commands::tracker::retry::process_retries(&config) {
                Ok(()) => println!("tracker retry queue processed"),
                Err(e) => {
                    eprintln!("tracker retry failed: {}", e);
                    std::process::exit(1);
                }
            }
        }
        "log" => {
            let lines = if args.len() > 1 && args[1] == "-n" && args.len() > 2 {
                args[2].parse::<usize>().unwrap_or(100)
            } else {
                100
            };
            commands::tracker::log::print_log(lines);
        }
        "blacklist" => {
            if args.len() < 2 {
                eprintln!("Usage: easylife-ai tracker blacklist <list|add|remove> [pattern]");
                std::process::exit(1);
            }
            match args[1].as_str() {
                "list" => match commands::tracker::config::list_blacklist() {
                    Ok(patterns) => {
                        if patterns.is_empty() {
                            println!("Blacklist is empty");
                        } else {
                            for pattern in patterns {
                                println!("{}", pattern);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        std::process::exit(1);
                    }
                },
                "add" => {
                    let pattern =
                        args.get(2)
                            .cloned()
                            .unwrap_or_else(|| match current_repo_url() {
                                Some(url) => url,
                                None => {
                                    eprintln!("未检测到 git remote origin，请手动指定 repo URL");
                                    eprintln!(
                                        "Usage: easylife-ai tracker blacklist add <repo_url>"
                                    );
                                    std::process::exit(1);
                                }
                            });
                    match commands::tracker::config::add_to_blacklist(&pattern) {
                        Ok(()) => println!("已将 '{}' 加入黑名单", pattern),
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                "remove" => {
                    let pattern =
                        args.get(2)
                            .cloned()
                            .unwrap_or_else(|| match current_repo_url() {
                                Some(url) => url,
                                None => {
                                    eprintln!("未检测到 git remote origin，请手动指定 repo URL");
                                    eprintln!(
                                        "Usage: easylife-ai tracker blacklist remove <repo_url>"
                                    );
                                    std::process::exit(1);
                                }
                            });
                    match commands::tracker::config::remove_from_blacklist(&pattern) {
                        Ok(()) => println!("已将 '{}' 从黑名单移除", pattern),
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                _ => {
                    eprintln!("Unknown blacklist subcommand: {}", args[1]);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Unknown tracker subcommand: {}", args[0]);
            std::process::exit(1);
        }
    }
}

fn current_repo_url() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}
