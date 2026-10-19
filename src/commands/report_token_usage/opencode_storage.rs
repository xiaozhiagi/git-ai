//! easylife-ai fork: OpenCode storage access for token-usage reporting.
//!
//! Upstream moved OpenCode preset logic into
//! `checkpoint_agent/presets/opencode.rs` and made its storage helpers private
//! (they now serve checkpoint parsing only). The token reporter needs the same
//! three primitives, kept here as a fork-owned adapter so upstream files stay
//! untouched.

use crate::error::GitAiError;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// Namespace alias so the token reporter can keep calling the storage
/// primitives under the same name it used before the upstream refactor.
pub struct OpenCodeStorage;

impl OpenCodeStorage {
    pub fn opencode_data_path() -> Result<PathBuf, GitAiError> {
        opencode_data_path()
    }

    pub fn resolve_sqlite_db_path(path: &Path) -> Option<PathBuf> {
        resolve_sqlite_db_path(path)
    }

    pub fn open_sqlite_readonly(path: &Path) -> Result<Connection, GitAiError> {
        open_sqlite_readonly(path)
    }
}

/// Locate the OpenCode data directory for the current platform.
pub fn opencode_data_path() -> Result<PathBuf, GitAiError> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir()
            .ok_or_else(|| GitAiError::Generic("Could not determine home directory".to_string()))?;
        Ok(home.join(".local").join("share").join("opencode"))
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
            Ok(PathBuf::from(xdg_data).join("opencode"))
        } else {
            let home = dirs::home_dir().ok_or_else(|| {
                GitAiError::Generic("Could not determine home directory".to_string())
            })?;
            Ok(home.join(".local").join("share").join("opencode"))
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(app_data) = std::env::var("APPDATA") {
            Ok(PathBuf::from(app_data).join("opencode"))
        } else if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            Ok(PathBuf::from(local_app_data).join("opencode"))
        } else {
            Err(GitAiError::Generic(
                "Neither APPDATA nor LOCALAPPDATA is set".to_string(),
            ))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(GitAiError::PresetError(
            "OpenCode storage path not supported on this platform".to_string(),
        ))
    }
}

/// Resolve the path of the OpenCode SQLite database from a candidate path.
pub fn resolve_sqlite_db_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| *name == "opencode.db")
            .map(|_| path.to_path_buf());
    }

    if !path.is_dir() {
        return None;
    }

    let direct_db = path.join("opencode.db");
    if direct_db.exists() {
        return Some(direct_db);
    }

    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "storage")
    {
        let sibling_db = path.parent()?.join("opencode.db");
        if sibling_db.exists() {
            return Some(sibling_db);
        }
    }

    None
}

/// Open the OpenCode SQLite database in read-only mode.
pub fn open_sqlite_readonly(path: &Path) -> Result<Connection, GitAiError> {
    let conn =
        crate::sqlite::open_with_flags_and_memory_limits(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| GitAiError::Generic(format!("Failed to open {:?}: {}", path, e)))?;

    // Limit SQLite page cache to ~2MB to prevent unbounded memory growth
    // when scanning large databases without indexes.
    // Default can grow much larger during repeated full table scans.
    let _ = conn.execute_batch("PRAGMA cache_size = -2000;");

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_missing_paths() {
        assert!(resolve_sqlite_db_path(Path::new("/nonexistent/opencode")).is_none());
    }

    #[test]
    fn resolve_accepts_direct_db_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("opencode.db");
        std::fs::write(&db, b"x").expect("write db stub");
        assert_eq!(
            resolve_sqlite_db_path(&db),
            Some(db.clone()),
            "direct opencode.db file should resolve to itself"
        );
    }
}
