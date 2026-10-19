//! Tracker pre-push state capture.
//!
//! Ported from the pre-merge `commands/hooks/push_hooks.rs`: before a push
//! mutates the remote, capture the remote's current branch -> sha map so
//! `report_pushed_commits` can later compute which commits were newly pushed.

use crate::git::cli_parser::{ParsedGitInvocation, is_dry_run};
use crate::git::repository::Repository;
use std::collections::HashMap;

/// Decide whether this push invocation is one the tracker should report.
/// Dry runs, branch deletions, and mirror pushes carry no new reportable commits.
pub fn should_skip_tracker_push(command_args: &[String]) -> bool {
    is_dry_run(command_args)
        || command_args.iter().any(|a| a == "-d" || a == "--delete")
        || command_args.iter().any(|a| a == "--mirror")
}

/// Capture the remote refs that exist *before* the push runs, plus the remote
/// the push targets. Returns `(refs, remote)`; either may be `None` when the
/// state can't be determined (no remote resolvable, or ls-remote failed).
pub fn capture_tracker_state(
    parsed_args: &ParsedGitInvocation,
    repository: &Repository,
) -> (
    Option<std::collections::HashMap<String, String>>,
    Option<String>,
) {
    if should_skip_tracker_push(&parsed_args.command_args) {
        return (None, None);
    }

    let remote = resolve_push_remote(parsed_args, repository);

    let refs = remote.as_deref().and_then(|remote_name| {
        // Use resolve_work_tree to handle Windows paths where repository.path()
        // returns the .git directory (e.g. C:\...\repo\.git) instead of the work tree.
        let repo_path_raw = repository.path().to_string_lossy().to_string();
        let work_tree = super::resolve_work_tree(&repo_path_raw);
        std::process::Command::new("git")
            .args(["-C", &work_tree, "ls-remote", "--heads", remote_name])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let mut map = HashMap::new();
                    for line in text.lines() {
                        // Trim trailing \r for Windows CRLF output
                        let line = line.trim_end_matches('\r');
                        let parts: Vec<&str> = line.splitn(2, '\t').collect();
                        if parts.len() == 2 {
                            let sha = parts[0].trim().to_string();
                            let refname = parts[1].trim();
                            if let Some(branch) = refname.strip_prefix("refs/heads/") {
                                map.insert(branch.to_string(), sha);
                            }
                        }
                    }
                    Some(map)
                } else {
                    None
                }
            })
    });

    (refs, remote)
}

fn resolve_push_remote(
    parsed_args: &ParsedGitInvocation,
    repository: &Repository,
) -> Option<String> {
    let remote_names: Vec<String> = repository.remotes().unwrap_or_default();
    let upstream_remote = repository.upstream_remote().ok().flatten();
    let default_remote = repository.get_default_remote().ok().flatten();

    resolve_push_remote_from_parts(
        &parsed_args.command_args,
        &remote_names,
        upstream_remote,
        default_remote,
    )
}

fn resolve_push_remote_from_parts(
    command_args: &[String],
    known_remotes: &[String],
    upstream_remote: Option<String>,
    default_remote: Option<String>,
) -> Option<String> {
    let positional_remote = extract_remote_from_push_args(command_args, known_remotes);

    let specified_remote = positional_remote.or_else(|| {
        command_args
            .iter()
            .find(|arg| known_remotes.iter().any(|remote| remote == *arg))
            .cloned()
    });

    specified_remote.or(upstream_remote).or(default_remote)
}

fn extract_remote_from_push_args(args: &[String], known_remotes: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            return args.get(i + 1).cloned();
        }
        if arg.starts_with('-') {
            if let Some((flag, value)) = is_push_option_with_inline_value(arg) {
                if flag == "--repo" {
                    return Some(value.to_string());
                }
                i += 1;
                continue;
            }

            if option_consumes_separate_value(arg.as_str()) {
                if arg == "--repo" {
                    return args.get(i + 1).cloned();
                }
                i += 2;
                continue;
            }

            i += 1;
            continue;
        }
        return Some(arg.clone());
    }

    known_remotes
        .iter()
        .find(|r| args.iter().any(|arg| arg == *r))
        .cloned()
}

fn is_push_option_with_inline_value(arg: &str) -> Option<(&str, &str)> {
    if let Some((flag, value)) = arg.split_once('=') {
        Some((flag, value))
    } else if (arg.starts_with("-C") || arg.starts_with("-c")) && arg.len() > 2 {
        // Treat -C<path> or -c<name>=<value> as inline values
        let flag = &arg[..2];
        let value = &arg[2..];
        Some((flag, value))
    } else {
        None
    }
}

fn option_consumes_separate_value(arg: &str) -> bool {
    matches!(
        arg,
        "--repo" | "--receive-pack" | "--exec" | "-o" | "--push-option" | "-c" | "-C"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn skip_tracker_push_when_dry_run() {
        assert!(should_skip_tracker_push(&strings(&["--dry-run"])));
    }

    #[test]
    fn skip_tracker_push_when_delete() {
        assert!(should_skip_tracker_push(&strings(&["--delete"])));
        assert!(should_skip_tracker_push(&strings(&["-d"])));
    }

    #[test]
    fn skip_tracker_push_when_mirror() {
        assert!(should_skip_tracker_push(&strings(&["--mirror"])));
    }

    #[test]
    fn resolve_push_remote_prefers_positional_remote() {
        let args = strings(&["origin", "main"]);
        let remote = resolve_push_remote_from_parts(
            &args,
            &strings(&["origin", "upstream"]),
            Some("upstream".to_string()),
            Some("origin".to_string()),
        );
        assert_eq!(remote.as_deref(), Some("origin"));
    }

    #[test]
    fn resolve_push_remote_prefers_repo_flag() {
        let args = strings(&["--repo", "upstream", "HEAD"]);
        let remote = resolve_push_remote_from_parts(
            &args,
            &strings(&["origin", "upstream"]),
            Some("origin".to_string()),
            None,
        );
        assert_eq!(remote.as_deref(), Some("upstream"));
    }

    #[test]
    fn resolve_push_remote_falls_back_to_upstream_then_default() {
        let args = Vec::<String>::new();
        let with_upstream = resolve_push_remote_from_parts(
            &args,
            &strings(&["origin"]),
            Some("upstream".to_string()),
            Some("origin".to_string()),
        );
        assert_eq!(with_upstream.as_deref(), Some("upstream"));

        let with_default = resolve_push_remote_from_parts(
            &args,
            &strings(&["origin"]),
            None,
            Some("origin".to_string()),
        );
        assert_eq!(with_default.as_deref(), Some("origin"));
    }

    #[test]
    fn extract_remote_skips_options() {
        let args = strings(&["--tags", "origin", "main"]);
        let remote = extract_remote_from_push_args(&args, &strings(&["origin"]));
        assert_eq!(remote.as_deref(), Some("origin"));
    }
}
