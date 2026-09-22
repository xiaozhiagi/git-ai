use crate::config;
use crate::error::GitAiError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub source: String,
    pub relative_path: String,
    pub kind: ManagedEntryKind,
    pub sha256: Option<String>,
    pub symlink_target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub backup_id: String,
    pub created_at: String,
    pub version: String,
    pub platform: String,
    pub source: String,
    pub entries: Vec<BackupEntry>,
}

pub fn backups_dir() -> Result<PathBuf, GitAiError> {
    config::git_ai_dir_path()
        .map(|path| path.join("backups"))
        .ok_or_else(|| GitAiError::Generic("Could not determine backup directory".to_string()))
}

fn app_dir() -> Result<PathBuf, GitAiError> {
    config::git_ai_dir_path()
        .ok_or_else(|| GitAiError::Generic("Could not determine application directory".to_string()))
}

fn managed_paths() -> Result<Vec<(PathBuf, String)>, GitAiError> {
    let app = app_dir()?;
    let mut paths = vec![
        (app.join("bin"), "app/bin".to_string()),
        (app.join("config.json"), "app/config.json".to_string()),
        (
            app.join("tracker-config.json"),
            "app/tracker-config.json".to_string(),
        ),
        (app.join("skills"), "app/skills".to_string()),
    ];

    if let Some(home) = dirs::home_dir() {
        paths.push((
            home.join(".local").join("bin").join("easylife-ai"),
            "links/local-easylife-ai".to_string(),
        ));
        for (base, label) in [
            (home.join(".agents").join("skills"), "links/agents-skills"),
            (home.join(".claude").join("skills"), "links/claude-skills"),
            (home.join(".cursor").join("skills"), "links/cursor-skills"),
        ] {
            for skill in ["ask", "git-ai-search", "prompt-analysis"] {
                paths.push((base.join(skill), format!("{}/{}", label, skill)));
            }
        }
    }

    Ok(paths)
}

fn sha256_file(path: &Path) -> Result<String, GitAiError> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn copy_entry(
    source: &Path,
    destination: &Path,
    relative_path: &str,
    entries: &mut Vec<BackupEntry>,
) -> Result<(), GitAiError> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(source)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, destination)?;
        #[cfg(windows)]
        {
            if target.is_dir() {
                std::os::windows::fs::symlink_dir(&target, destination)?;
            } else {
                std::os::windows::fs::symlink_file(&target, destination)?;
            }
        }
        entries.push(BackupEntry {
            source: source.to_string_lossy().to_string(),
            relative_path: relative_path.to_string(),
            kind: ManagedEntryKind::Symlink,
            sha256: None,
            symlink_target: Some(target.to_string_lossy().to_string()),
        });
    } else if file_type.is_dir() {
        fs::create_dir_all(destination)?;
        entries.push(BackupEntry {
            source: source.to_string_lossy().to_string(),
            relative_path: relative_path.to_string(),
            kind: ManagedEntryKind::Directory,
            sha256: None,
            symlink_target: None,
        });
        for child in fs::read_dir(source)? {
            let child = child?;
            let child_name = child.file_name().to_string_lossy().to_string();
            copy_entry(
                &child.path(),
                &destination.join(&child_name),
                &format!("{}/{}", relative_path, child_name),
                entries,
            )?;
        }
    } else if file_type.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        entries.push(BackupEntry {
            source: source.to_string_lossy().to_string(),
            relative_path: relative_path.to_string(),
            kind: ManagedEntryKind::File,
            sha256: Some(sha256_file(source)?),
            symlink_target: None,
        });
    }
    Ok(())
}

pub fn create_backup(source: &str) -> Result<BackupManifest, GitAiError> {
    let root = backups_dir()?;
    fs::create_dir_all(&root)?;
    let version = env!("CARGO_PKG_VERSION");
    let backup_id = format!(
        "{}-{}-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        version,
        std::process::id()
    );
    let backup_path = root.join(&backup_id);
    fs::create_dir_all(&backup_path)?;

    let mut entries = Vec::new();
    for (path, relative) in managed_paths()? {
        if path.exists() || path.symlink_metadata().is_ok() {
            copy_entry(&path, &backup_path.join(&relative), &relative, &mut entries)?;
        }
    }

    let manifest = BackupManifest {
        backup_id: backup_id.clone(),
        created_at: Utc::now().to_rfc3339(),
        version: version.to_string(),
        platform: std::env::consts::OS.to_string(),
        source: source.to_string(),
        entries,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(backup_path.join("manifest.json"), manifest_bytes)?;
    println!("Created backup {} at {}", backup_id, backup_path.display());
    Ok(manifest)
}

fn remove_path(path: &Path) -> Result<(), GitAiError> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn restore_backup(backup_id: &str) -> Result<(), GitAiError> {
    let backup_path = backups_dir()?.join(backup_id);
    let manifest_path = backup_path.join("manifest.json");
    let manifest: BackupManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;

    for entry in &manifest.entries {
        let source = backup_path.join(&entry.relative_path);
        let target = PathBuf::from(&entry.source);
        if matches!(entry.kind, ManagedEntryKind::Directory) {
            fs::create_dir_all(&target)?;
            continue;
        }
        remove_path(&target)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match entry.kind {
            ManagedEntryKind::File => {
                fs::copy(&source, &target)?;
            }
            ManagedEntryKind::Symlink => {
                let link_target = entry.symlink_target.as_deref().ok_or_else(|| {
                    GitAiError::Generic("Backup symlink has no target".to_string())
                })?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(link_target, &target)?;
                #[cfg(windows)]
                {
                    let link_target = Path::new(link_target);
                    if link_target.is_dir() {
                        std::os::windows::fs::symlink_dir(link_target, &target)?;
                    } else {
                        std::os::windows::fs::symlink_file(link_target, &target)?;
                    }
                }
            }
            ManagedEntryKind::Directory => {}
        }
    }
    println!("Restored backup {}", backup_id);
    Ok(())
}

/// Remove a backup from disk once it is no longer needed.
///
/// Used after a successful upgrade to discard the pre-upgrade snapshot.
pub fn delete_backup(backup_id: &str) -> Result<(), GitAiError> {
    let backup_path = backups_dir()?.join(backup_id);
    if !backup_path.exists() {
        return Err(GitAiError::Generic(format!(
            "Backup {} does not exist",
            backup_id
        )));
    }
    fs::remove_dir_all(&backup_path)?;
    Ok(())
}

pub fn list_backups() -> Result<(), GitAiError> {
    let root = backups_dir()?;
    if !root.exists() {
        println!("No backups found.");
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(root)?.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let manifest_path = entry.path().join("manifest.json");
        if manifest_path.exists()
            && let Ok(manifest) =
                serde_json::from_slice::<BackupManifest>(&fs::read(&manifest_path)?)
        {
            println!(
                "{}\t{}\t{}",
                manifest.backup_id, manifest.version, manifest.created_at
            );
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), GitAiError> {
    match args.first().map(String::as_str) {
        None => create_backup("manual").map(|_| ()),
        Some("--list") | Some("list") => list_backups(),
        Some("--restore") | Some("restore") => {
            let id = args.get(1).ok_or_else(|| {
                GitAiError::Generic("backup restore requires a backup id".to_string())
            })?;
            restore_backup(id)
        }
        Some(value) => Err(GitAiError::Generic(format!(
            "Unknown backup argument: {}",
            value
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct HomeGuard {
        previous_home: Option<std::ffi::OsString>,
        previous_userprofile: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let previous_home = std::env::var_os("HOME");
            let previous_userprofile = std::env::var_os("USERPROFILE");
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("USERPROFILE", home);
            }
            Self {
                previous_home,
                previous_userprofile,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.previous_home {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
                match &self.previous_userprofile {
                    Some(value) => std::env::set_var("USERPROFILE", value),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }
    }

    #[test]
    fn backup_manifest_serializes_kind() {
        let entry = BackupEntry {
            source: "/tmp/file".to_string(),
            relative_path: "app/file".to_string(),
            kind: ManagedEntryKind::File,
            sha256: Some("abc".to_string()),
            symlink_target: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("file"));
    }

    #[test]
    #[serial]
    fn backup_and_restore_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let _guard = HomeGuard::set(home);

        let app = home.join(config::APP_DIR_NAME);
        let bin = app.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("easylife-ai"), b"original-binary").unwrap();
        fs::write(app.join("config.json"), b"{\"a\":1}").unwrap();

        let manifest = create_backup("test").unwrap();
        assert!(!manifest.entries.is_empty());

        // Mutate the installation, then verify restore brings the original back.
        fs::write(bin.join("easylife-ai"), b"updated-binary").unwrap();
        fs::remove_file(app.join("config.json")).unwrap();

        restore_backup(&manifest.backup_id).unwrap();

        assert_eq!(
            fs::read(bin.join("easylife-ai")).unwrap(),
            b"original-binary"
        );
        assert_eq!(fs::read(app.join("config.json")).unwrap(), b"{\"a\":1}");
    }

    #[test]
    #[serial]
    fn backup_preserves_user_data_outside_managed_paths() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let _guard = HomeGuard::set(home);

        let app = home.join(config::APP_DIR_NAME);
        fs::create_dir_all(app.join("bin")).unwrap();
        fs::write(app.join("bin").join("easylife-ai"), b"binary").unwrap();

        // A user-owned file inside the app directory that is not managed by the
        // client must never be captured or overwritten by a restore.
        fs::write(app.join("tracker-config.json"), b"{\"team\":1}").unwrap();
        fs::write(home.join(".zshrc"), b"export PATH=x").unwrap();

        let manifest = create_backup("test").unwrap();
        let paths: Vec<&str> = manifest
            .entries
            .iter()
            .map(|entry| entry.relative_path.as_str())
            .collect();
        assert!(!paths.iter().any(|path| path.contains("zshrc")));
    }

    #[test]
    #[serial]
    fn delete_backup_removes_the_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let _guard = HomeGuard::set(home);

        let app = home.join(config::APP_DIR_NAME);
        fs::create_dir_all(app.join("bin")).unwrap();
        fs::write(app.join("bin").join("easylife-ai"), b"binary").unwrap();

        let manifest = create_backup("test").unwrap();
        assert!(backups_dir().unwrap().join(&manifest.backup_id).exists());

        delete_backup(&manifest.backup_id).unwrap();
        assert!(!backups_dir().unwrap().join(&manifest.backup_id).exists());

        // Deleting a backup that is already gone is a reported error, not a silent success.
        assert!(delete_backup(&manifest.backup_id).is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn backup_records_symlinks_without_following_them() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let _guard = HomeGuard::set(home);

        let skill_source = home.join(config::APP_DIR_NAME).join("skills").join("ask");
        fs::create_dir_all(&skill_source).unwrap();
        fs::write(skill_source.join("SKILL.md"), b"skill").unwrap();

        let link = home.join(".agents").join("skills").join("ask");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&skill_source, &link).unwrap();

        let manifest = create_backup("test").unwrap();
        let symlink_entry = manifest
            .entries
            .iter()
            .find(|entry| entry.relative_path.ends_with("agents-skills/ask"))
            .expect("expected symlink entry in manifest");
        assert!(matches!(symlink_entry.kind, ManagedEntryKind::Symlink));
        assert!(symlink_entry.sha256.is_none());
        assert_eq!(
            symlink_entry.symlink_target.as_deref(),
            Some(skill_source.to_string_lossy().as_ref())
        );
    }
}
