//! easylife-ai fork compatibility layer for the prompt-history command cluster.
//!
//! Upstream git-ai moved transcripts out of checkpoints and authorship notes
//! (sessions v2 / CAS streaming) and dropped the `prompts` table accessors from
//! `InternalDatabase`. The easylife prompt commands (`prompts`, `prompt`,
//! `search`, `continue`, `share`, `sync-prompts`, prompt picker/TUI) still rely
//! on that storage, and the `prompts` SQL schema itself survives in upstream
//! migrations. Everything the fork needs to keep those commands working lives
//! here so upstream files stay untouched.
//!
//! Integration points (see `docs/easylife-integration-points.md`):
//! - `post_commit.rs` calls [`persist_prompts_to_internal_db`] after the
//!   authorship note is written.
//! - `authorship/internal_db.rs` exposes `pub(crate)` connection accessors.

use crate::authorship::internal_db::InternalDatabase;
use crate::authorship::prompt_utils::find_prompt_in_history;
use crate::authorship::secrets::redact_secrets_in_text;
use crate::authorship::transcript::{AiTranscript, Message};
use crate::authorship::working_log::AgentId;
use crate::error::GitAiError;
use crate::git::repository::Repository;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap};

/// Fork-owned prompt record: the pre-sessions-v2 prompt shape that carries the
/// full transcript inline. Upstream's `authorship_log::PromptRecord` no longer
/// has a `messages` field, so the easylife prompt commands convert to this type
/// at the boundary.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PromptRecord {
    pub agent_id: AgentId,
    pub human_author: Option<String>,
    pub messages: Vec<Message>,
    pub total_additions: u32,
    pub total_deletions: u32,
    pub accepted_lines: u32,
    pub overriden_lines: u32,
    pub messages_url: Option<String>,
    pub custom_attributes: Option<HashMap<String, String>>,
}

impl PromptRecord {
    /// Convert an upstream note prompt (metadata only, no transcript body).
    pub fn from_upstream(
        id: &str,
        prompt: &crate::authorship::authorship_log::PromptRecord,
    ) -> Self {
        let _ = id;
        Self {
            agent_id: prompt.agent_id.clone(),
            human_author: prompt.human_author.clone(),
            messages: Vec::new(),
            total_additions: prompt.total_additions,
            total_deletions: prompt.total_deletions,
            accepted_lines: prompt.accepted_lines,
            overriden_lines: prompt.overriden_lines,
            messages_url: prompt.messages_url.clone(),
            custom_attributes: prompt.custom_attributes.clone(),
        }
    }
}

/// Prompt record for database storage (fork-owned copy of the pre-sessions-v2
/// schema accessors).
#[derive(Debug, Clone)]
pub struct PromptDbRecord {
    pub id: String,
    pub workdir: Option<String>,
    pub tool: String,
    pub model: String,
    pub external_thread_id: String,
    pub messages: AiTranscript,
    pub commit_sha: Option<String>,
    pub agent_metadata: Option<HashMap<String, String>>,
    pub human_author: Option<String>,
    pub total_additions: Option<u32>,
    pub total_deletions: Option<u32>,
    pub accepted_lines: Option<u32>,
    pub overridden_lines: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl PromptDbRecord {
    /// Build a DB record from an authorship-note prompt (metadata only).
    ///
    /// Upstream notes no longer embed transcripts, so `messages` starts empty;
    /// the read paths in `prompts_db.rs` / `search.rs` resolve message bodies
    /// from the CAS cache when they are available.
    pub fn from_upstream_prompt(
        id: &str,
        prompt: &crate::authorship::authorship_log::PromptRecord,
        workdir: Option<String>,
        commit_sha: Option<String>,
    ) -> Self {
        let now = unix_now();

        Self {
            id: id.to_string(),
            workdir,
            tool: prompt.agent_id.tool.clone(),
            model: prompt.agent_id.model.clone(),
            external_thread_id: prompt.agent_id.id.clone(),
            messages: AiTranscript::new(),
            commit_sha,
            agent_metadata: None,
            human_author: prompt.human_author.clone(),
            total_additions: Some(prompt.total_additions),
            total_deletions: Some(prompt.total_deletions),
            accepted_lines: Some(prompt.accepted_lines),
            overridden_lines: Some(prompt.overriden_lines),
            created_at: now,
            updated_at: now,
        }
    }

    /// Convert PromptDbRecord to the fork prompt record (with transcript).
    pub fn to_prompt_record(&self) -> PromptRecord {
        PromptRecord {
            agent_id: AgentId {
                tool: self.tool.clone(),
                id: self.external_thread_id.clone(),
                model: self.model.clone(),
            },
            human_author: self.human_author.clone(),
            messages: self.messages.messages.clone(),
            total_additions: self.total_additions.unwrap_or(0),
            total_deletions: self.total_deletions.unwrap_or(0),
            accepted_lines: self.accepted_lines.unwrap_or(0),
            overriden_lines: self.overridden_lines.unwrap_or(0),
            messages_url: None,
            custom_attributes: None,
        }
    }

    /// Extract first user message snippet, truncated to max_length
    pub fn first_message_snippet(&self, max_length: usize) -> String {
        // Truncate at a valid UTF-8 character boundary (like floor_char_boundary, but stable)
        fn truncate_to_boundary(s: &str, max_bytes: usize) -> &str {
            if max_bytes >= s.len() {
                return s;
            }
            let mut boundary = max_bytes;
            while !s.is_char_boundary(boundary) && boundary > 0 {
                boundary -= 1;
            }
            &s[..boundary]
        }

        for message in &self.messages.messages {
            if let Message::User { text, .. } = message {
                if text.len() <= max_length {
                    return text.clone();
                } else {
                    return format!("{}...", truncate_to_boundary(text, max_length));
                }
            }
        }

        // Fallback: if no user message, try first AI message
        for message in &self.messages.messages {
            if let Message::Assistant { text, .. }
            | Message::Thinking { text, .. }
            | Message::Plan { text, .. } = message
            {
                if text.len() <= max_length {
                    return text.clone();
                } else {
                    return format!("{}...", truncate_to_boundary(text, max_length));
                }
            }
        }

        "(No messages)".to_string()
    }

    /// Count total messages in transcript
    pub fn message_count(&self) -> usize {
        self.messages.messages.len()
    }

    /// Format relative time ("1 day ago", "5 days ago", etc.)
    pub fn relative_time(&self) -> String {
        let now = unix_now();

        let diff = now - self.updated_at;

        if diff < 60 {
            return format!("{} second{} ago", diff, if diff == 1 { "" } else { "s" });
        }

        let minutes = diff / 60;
        if minutes < 60 {
            return format!(
                "{} minute{} ago",
                minutes,
                if minutes == 1 { "" } else { "s" }
            );
        }

        let hours = minutes / 60;
        if hours < 24 {
            return format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" });
        }

        let days = hours / 24;
        if days < 7 {
            return format!("{} day{} ago", days, if days == 1 { "" } else { "s" });
        }

        let weeks = days / 7;
        if weeks < 4 {
            return format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" });
        }

        let months = days / 30;
        if months < 12 {
            return format!("{} month{} ago", months, if months == 1 { "" } else { "s" });
        }

        let years = days / 365;
        format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// easylife-ai fork: re-adds the prompt-table accessors upstream removed.
/// The `prompts` schema itself is still created by upstream migrations.
impl InternalDatabase {
    /// Upsert a prompt record
    pub fn upsert_prompt(&mut self, record: &PromptDbRecord) -> Result<(), GitAiError> {
        let messages_json = serde_json::to_string(&record.messages)?;
        let metadata_json = record
            .agent_metadata
            .as_ref()
            .and_then(|m| serde_json::to_string(m).ok());

        self.fork_prompts_conn().execute(
            r#"
            INSERT INTO prompts (
                id, workdir, tool, model, external_thread_id,
                messages, commit_sha, agent_metadata, human_author,
                total_additions, total_deletions, accepted_lines,
                overridden_lines, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(id) DO UPDATE SET
                workdir = excluded.workdir,
                model = excluded.model,
                messages = excluded.messages,
                commit_sha = excluded.commit_sha,
                agent_metadata = excluded.agent_metadata,
                human_author = excluded.human_author,
                total_additions = excluded.total_additions,
                total_deletions = excluded.total_deletions,
                accepted_lines = excluded.accepted_lines,
                overridden_lines = excluded.overridden_lines,
                updated_at = excluded.updated_at
            "#,
            params![
                record.id,
                record.workdir,
                record.tool,
                record.model,
                record.external_thread_id,
                messages_json,
                record.commit_sha,
                metadata_json,
                record.human_author,
                record.total_additions,
                record.total_deletions,
                record.accepted_lines,
                record.overridden_lines,
                record.created_at,
                record.updated_at,
            ],
        )?;

        Ok(())
    }

    /// Batch upsert multiple prompts (for post-commit)
    pub fn batch_upsert_prompts(&mut self, records: &[PromptDbRecord]) -> Result<(), GitAiError> {
        if records.is_empty() {
            return Ok(());
        }

        let tx = self.fork_prompts_conn_mut().transaction()?;

        {
            // Prepare statement once and reuse for all records (much faster than parsing SQL each time)
            let mut stmt = tx.prepare_cached(
                r#"
                INSERT INTO prompts (
                    id, workdir, tool, model, external_thread_id,
                    messages, commit_sha, agent_metadata, human_author,
                    total_additions, total_deletions, accepted_lines,
                    overridden_lines, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                ON CONFLICT(id) DO UPDATE SET
                    workdir = excluded.workdir,
                    model = excluded.model,
                    messages = excluded.messages,
                    commit_sha = excluded.commit_sha,
                    agent_metadata = excluded.agent_metadata,
                    human_author = excluded.human_author,
                    total_additions = excluded.total_additions,
                    total_deletions = excluded.total_deletions,
                    accepted_lines = excluded.accepted_lines,
                    overridden_lines = excluded.overridden_lines,
                    updated_at = excluded.updated_at
                "#,
            )?;

            for record in records {
                let messages_json = serde_json::to_string(&record.messages)?;
                let metadata_json = record
                    .agent_metadata
                    .as_ref()
                    .and_then(|m| serde_json::to_string(m).ok());

                stmt.execute(params![
                    record.id,
                    record.workdir,
                    record.tool,
                    record.model,
                    record.external_thread_id,
                    messages_json,
                    record.commit_sha,
                    metadata_json,
                    record.human_author,
                    record.total_additions,
                    record.total_deletions,
                    record.accepted_lines,
                    record.overridden_lines,
                    record.created_at,
                    record.updated_at,
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Get a prompt by ID
    pub fn get_prompt(&self, id: &str) -> Result<Option<PromptDbRecord>, GitAiError> {
        let mut stmt = self.fork_prompts_conn().prepare(
            "SELECT id, workdir, tool, model, external_thread_id, messages,
                    commit_sha, agent_metadata, human_author,
                    total_additions, total_deletions, accepted_lines,
                    overridden_lines, created_at, updated_at
             FROM prompts WHERE id = ?1",
        )?;

        let result = stmt.query_row(params![id], fork_prompt_row);

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all prompts for a given commit
    pub fn get_prompts_by_commit(
        &self,
        commit_sha: &str,
    ) -> Result<Vec<PromptDbRecord>, GitAiError> {
        let mut stmt = self.fork_prompts_conn().prepare(
            "SELECT id, workdir, tool, model, external_thread_id, messages,
                    commit_sha, agent_metadata, human_author,
                    total_additions, total_deletions, accepted_lines,
                    overridden_lines, created_at, updated_at
             FROM prompts WHERE commit_sha = ?1",
        )?;

        let rows = stmt.query_map(params![commit_sha], fork_prompt_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }

        Ok(records)
    }

    /// List prompts with optional workdir and since filters, ordered by updated_at DESC
    pub fn list_prompts(
        &self,
        workdir: Option<&str>,
        since: Option<i64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<PromptDbRecord>, GitAiError> {
        let (query, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match (workdir, since) {
            (Some(wd), Some(ts)) => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts WHERE workdir = ?1 AND updated_at >= ?2 ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4".to_string(),
                vec![Box::new(wd.to_string()), Box::new(ts), Box::new(limit as i64), Box::new(offset as i64)],
            ),
            (Some(wd), None) => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts WHERE workdir = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3".to_string(),
                vec![Box::new(wd.to_string()), Box::new(limit as i64), Box::new(offset as i64)],
            ),
            (None, Some(ts)) => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts WHERE updated_at >= ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3".to_string(),
                vec![Box::new(ts), Box::new(limit as i64), Box::new(offset as i64)],
            ),
            (None, None) => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2".to_string(),
                vec![Box::new(limit as i64), Box::new(offset as i64)],
            ),
        };

        let mut stmt = self.fork_prompts_conn().prepare(&query)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(&params_refs[..], fork_prompt_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }

        Ok(records)
    }

    /// Search prompts by message content with optional workdir filter
    pub fn search_prompts(
        &self,
        search_query: &str,
        workdir: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<PromptDbRecord>, GitAiError> {
        let search_pattern = format!("%{}%", search_query);

        let (query, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match workdir {
            Some(wd) => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts WHERE messages LIKE ?1 AND workdir = ?2 ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4".to_string(),
                vec![Box::new(search_pattern), Box::new(wd.to_string()), Box::new(limit as i64), Box::new(offset as i64)],
            ),
            None => (
                "SELECT id, workdir, tool, model, external_thread_id, messages,
                        commit_sha, agent_metadata, human_author,
                        total_additions, total_deletions, accepted_lines,
                        overridden_lines, created_at, updated_at
                 FROM prompts WHERE messages LIKE ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3".to_string(),
                vec![Box::new(search_pattern), Box::new(limit as i64), Box::new(offset as i64)],
            ),
        };

        let mut stmt = self.fork_prompts_conn().prepare(&query)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(&params_refs[..], fork_prompt_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }

        Ok(records)
    }
}

fn fork_prompt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptDbRecord> {
    let messages_json: String = row.get(5)?;
    let messages: AiTranscript = serde_json::from_str(&messages_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let agent_metadata: Option<HashMap<String, String>> = row
        .get::<_, Option<String>>(7)?
        .and_then(|json| serde_json::from_str(&json).ok());

    Ok(PromptDbRecord {
        id: row.get(0)?,
        workdir: row.get(1)?,
        tool: row.get(2)?,
        model: row.get(3)?,
        external_thread_id: row.get(4)?,
        messages,
        commit_sha: row.get(6)?,
        agent_metadata,
        human_author: row.get(8)?,
        total_additions: row.get(9)?,
        total_deletions: row.get(10)?,
        accepted_lines: row.get(11)?,
        overridden_lines: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

/// Look up a prompt in the internal DB first, then fall back to repository
/// history. Restored for the `share` command.
pub fn find_prompt_with_db_fallback(
    prompt_id: &str,
    repo: Option<&Repository>,
) -> Result<(Option<String>, PromptRecord), GitAiError> {
    // First, try to get from database
    let db = InternalDatabase::global()?;
    let db_guard = db
        .lock()
        .map_err(|e| GitAiError::Generic(format!("Failed to lock database: {}", e)))?;

    if let Some(db_record) = db_guard.get_prompt(prompt_id)? {
        // Convert PromptDbRecord to the fork prompt record
        let prompt_record = db_record.to_prompt_record();
        return Ok((db_record.commit_sha, prompt_record));
    }

    // Not found in DB, try repository if provided
    if let Some(repo) = repo {
        // Try to find in history (most recent occurrence)
        match find_prompt_in_history(repo, prompt_id, 0) {
            Ok((commit_sha, prompt)) => Ok((
                Some(commit_sha),
                PromptRecord::from_upstream(prompt_id, &prompt),
            )),
            Err(_) => Err(GitAiError::Generic(format!(
                "Prompt '{}' not found in database or repository",
                prompt_id
            ))),
        }
    } else {
        Err(GitAiError::Generic(format!(
            "Prompt '{}' not found in database and no repository provided",
            prompt_id
        )))
    }
}

/// Redact secrets in every message of every prompt record.
/// Restored for `share` / `continue`; wraps upstream `redact_secrets_in_text`.
pub fn redact_secrets_from_prompts(prompts: &mut BTreeMap<String, PromptRecord>) -> usize {
    let mut total_redactions = 0;
    for record in prompts.values_mut() {
        for message in &mut record.messages {
            match message {
                Message::User { text, .. }
                | Message::Assistant { text, .. }
                | Message::Thinking { text, .. }
                | Message::Plan { text, .. } => {
                    let (redacted, count) = redact_secrets_in_text(text);
                    *text = redacted;
                    total_redactions += count;
                }
                Message::ToolUse { .. } => {
                    // Skip tool use messages - they contain structured data
                }
            }
        }
    }
    total_redactions
}

/// Result of attempting to update a prompt from a tool
pub enum PromptUpdateResult {
    Updated(AiTranscript, String), // (new_transcript, new_model)
    Unchanged,                     // No update available or needed
    Failed(GitAiError),            // Error occurred but not fatal
}

/// Update a prompt by fetching the latest transcript from the originating tool.
///
/// Degraded after the upstream sessions-v2 refactor: the per-tool transcript
/// readers (`transcript_and_model_from_*` preset methods) were removed upstream
/// and have not been ported yet, so this currently reports `Unchanged` for
/// every tool. `sync-prompts` remains safe to run — it reports records as
/// skipped instead of corrupting stored transcripts. Porting the readers back
/// as fork-owned modules is tracked in `docs/easylife-integration-points.md`.
pub fn update_prompt_from_tool(
    tool: &str,
    _external_thread_id: &str,
    _agent_metadata: Option<&HashMap<String, String>>,
    _current_model: &str,
) -> PromptUpdateResult {
    let _ = tool;
    PromptUpdateResult::Unchanged
}

/// Mirror authorship-note prompt metadata into the internal prompts DB.
///
/// Called from `post_commit.rs` after the note is written (async side-effect
/// path — never from the trace2 ingestion hot path). Message bodies are
/// resolved lazily by the read paths from the CAS cache.
pub fn persist_prompts_to_internal_db(
    prompts: &BTreeMap<String, crate::authorship::authorship_log::PromptRecord>,
    workdir: &str,
    commit_sha: &str,
) {
    if prompts.is_empty() {
        return;
    }

    let records: Vec<PromptDbRecord> = prompts
        .iter()
        .map(|(id, prompt)| {
            PromptDbRecord::from_upstream_prompt(
                id,
                prompt,
                Some(workdir.to_string()),
                Some(commit_sha.to_string()),
            )
        })
        .collect();

    let result = match InternalDatabase::global() {
        Ok(db) => db
            .lock()
            .map_err(|e| GitAiError::Generic(format!("Failed to lock database: {}", e)))
            .and_then(|mut guard| guard.batch_upsert_prompts(&records)),
        Err(e) => Err(e),
    };

    if let Err(e) = result {
        tracing::debug!("easylife: failed to persist prompt metadata: {}", e);
    }
}

/// Format a prompt record's messages into a human-readable transcript.
///
/// Filters out ToolUse messages; keeps User, Assistant, Thinking, and Plan.
/// Restored for the `search --verbose` output.
pub fn format_transcript(prompt: &PromptRecord) -> String {
    let mut output = String::new();
    for message in &prompt.messages {
        match message {
            Message::User { text, .. } => {
                output.push_str("User: ");
                output.push_str(text);
                output.push('\n');
            }
            Message::Assistant { text, .. } => {
                output.push_str("Assistant: ");
                output.push_str(text);
                output.push('\n');
            }
            Message::Thinking { text, .. } => {
                output.push_str("Thinking: ");
                output.push_str(text);
                output.push('\n');
            }
            Message::Plan { text, .. } => {
                output.push_str("Plan: ");
                output.push_str(text);
                output.push('\n');
            }
            Message::ToolUse { .. } => {
                // Skip tool use messages in formatted transcript
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_upstream_prompt() -> crate::authorship::authorship_log::PromptRecord {
        crate::authorship::authorship_log::PromptRecord {
            agent_id: AgentId {
                tool: "claude".to_string(),
                id: "session-1".to_string(),
                model: "opus".to_string(),
            },
            human_author: Some("Tester".to_string()),
            messages_url: None,
            total_additions: 12,
            total_deletions: 3,
            accepted_lines: 10,
            overriden_lines: 1,
            custom_attributes: None,
        }
    }

    #[test]
    fn from_upstream_prompt_maps_metadata_fields() {
        let record = PromptDbRecord::from_upstream_prompt(
            "s_abc::t_1",
            &sample_upstream_prompt(),
            Some("/repo".to_string()),
            Some("deadbeef".to_string()),
        );

        assert_eq!(record.id, "s_abc::t_1");
        assert_eq!(record.tool, "claude");
        assert_eq!(record.model, "opus");
        assert_eq!(record.commit_sha.as_deref(), Some("deadbeef"));
        assert_eq!(record.total_additions, Some(12));
        assert_eq!(record.total_deletions, Some(3));

        let back = record.to_prompt_record();
        assert_eq!(back.agent_id.id, "session-1");
        assert_eq!(back.total_additions, 12);
        assert_eq!(back.overriden_lines, 1);
        assert!(back.messages.is_empty());
    }

    #[test]
    fn snippet_falls_back_when_no_messages() {
        let record =
            PromptDbRecord::from_upstream_prompt("s_x", &sample_upstream_prompt(), None, None);

        assert_eq!(record.first_message_snippet(20), "(No messages)");
        assert_eq!(record.message_count(), 0);
    }
}
