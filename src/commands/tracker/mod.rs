pub mod config;
pub mod diff;
pub mod filter;
pub mod notes;
pub mod retry;
pub mod upload;

use std::collections::HashMap;

pub fn report_pushed_commits(
    repo_path: &str,
    pre_push_refs: &HashMap<String, String>,
    remote: &str,
) {
    let _ = (repo_path, pre_push_refs, remote);
}
