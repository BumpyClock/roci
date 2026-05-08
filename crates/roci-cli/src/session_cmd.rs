use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use roci::session::{
    CreateSessionOptions, ImportPolicy, LocalSessionStore, PathConventions, SessionId,
    SessionMetadata, SessionSnapshot,
};

use crate::cli::{
    SessionArgs, SessionCommands, SessionCreateArgs, SessionDeleteArgs, SessionExportArgs,
    SessionImportArgs, SessionListArgs,
};

pub async fn handle_session(args: SessionArgs) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        SessionCommands::Create(args) => {
            let summary = create_session(args).await?;
            print_create_summary(&summary)?;
        }
        SessionCommands::List(args) => {
            let json = args.json;
            let entries = list_sessions(args)?;
            print_list(&entries, json)?;
        }
        SessionCommands::Delete(args) => {
            let summary = delete_session(args)?;
            println!("Deleted session {}", summary.id);
            println!("Path: {}", summary.path.display());
        }
        SessionCommands::Export(args) => {
            let summary = export_session(args).await?;
            print_export_summary(&summary)?;
        }
        SessionCommands::Import(args) => {
            let summary = import_session(args).await?;
            print_import_summary(&summary)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionSummary {
    id: String,
    root: PathBuf,
    title: Option<String>,
    host_cwd: Option<PathBuf>,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionListEntry {
    id: String,
    title: Option<String>,
    last_activity_at: DateTime<Utc>,
    host_cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeleteSummary {
    id: String,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportSummary {
    id: String,
    output: PathBuf,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportSummary {
    id: String,
    root: PathBuf,
    json: bool,
}

async fn create_session(
    args: SessionCreateArgs,
) -> Result<SessionSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let id = args.id.map(SessionId::parse).transpose()?;
    let host_cwd = Some(std::env::current_dir()?);
    let store = LocalSessionStore::new(root.clone());
    let state = store
        .create(CreateSessionOptions {
            id,
            title: args.title,
            host_cwd: host_cwd.clone(),
            import_source: None,
            default_thread_id: None,
        })
        .await?;
    Ok(SessionSummary {
        id: state.metadata.id.to_string(),
        root,
        title: state.metadata.title,
        host_cwd,
        json: args.json,
    })
}

fn list_sessions(
    args: SessionListArgs,
) -> Result<Vec<SessionListEntry>, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }

        let Ok(dir_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(dir_id) = SessionId::parse(&dir_name) else {
            continue;
        };
        let metadata_path = entry.path().join("metadata.json");
        let Ok(metadata) = SessionMetadata::read_from_path(&metadata_path) else {
            continue;
        };
        if metadata.id != dir_id {
            continue;
        }
        entries.push(SessionListEntry {
            id: metadata.id.to_string(),
            title: metadata.title,
            last_activity_at: metadata.last_activity_at,
            host_cwd: metadata.host_cwd,
        });
    }
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(entries)
}

fn delete_session(args: SessionDeleteArgs) -> Result<DeleteSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let id = SessionId::parse(args.id)?;
    let session_dir = PathConventions::for_session(&root, &id)
        .root()
        .to_path_buf();
    let metadata_path = session_dir.join("metadata.json");
    if !session_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("session '{}' not found", id.as_str()),
        )
        .into());
    }
    let metadata = SessionMetadata::read_from_path(&metadata_path)?;
    if metadata.id != id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("session '{}' metadata id mismatch", id.as_str()),
        )
        .into());
    }

    fs::remove_dir_all(&session_dir)?;
    Ok(DeleteSummary {
        id: id.to_string(),
        path: session_dir,
    })
}

async fn export_session(
    args: SessionExportArgs,
) -> Result<ExportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let id = SessionId::parse(args.id)?;
    let store = LocalSessionStore::new(root);
    let snapshot = store.export_snapshot(id.clone()).await?;
    write_pretty_json(&args.output, &snapshot)?;
    Ok(ExportSummary {
        id: id.to_string(),
        output: args.output,
        json: args.json,
    })
}

async fn import_session(
    args: SessionImportArgs,
) -> Result<ImportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let bytes = fs::read(&args.input)?;
    let snapshot: SessionSnapshot = serde_json::from_slice(&bytes)?;
    let requested_id = args.id.map(SessionId::parse).transpose()?;
    let store = LocalSessionStore::new(root.clone());
    let state = store
        .import_snapshot(snapshot, ImportPolicy::NewId(requested_id))
        .await?;
    Ok(ImportSummary {
        id: state.metadata.id.to_string(),
        root,
        json: args.json,
    })
}

fn resolve_root(override_root: Option<PathBuf>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(root) = override_root {
        return Ok(root);
    }

    let dirs = ProjectDirs::from("", "", "roci").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve roci app data directory",
        )
    })?;
    Ok(dirs.data_dir().join("sessions"))
}

fn write_pretty_json(
    path: &Path,
    value: &SessionSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn print_create_summary(summary: &SessionSummary) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": summary.id,
                "root": summary.root,
                "title": summary.title,
                "host_cwd": summary.host_cwd,
            }))?
        );
    } else {
        println!("Created session {}", summary.id);
        println!("Root: {}", summary.root.display());
    }
    Ok(())
}

fn print_list(entries: &[SessionListEntry], json: bool) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        let values = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id,
                    "title": entry.title,
                    "last_activity_at": entry.last_activity_at,
                    "host_cwd": entry.host_cwd,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!("ID\tLAST_ACTIVITY\tTITLE\tHOST_CWD");
    for entry in entries {
        println!(
            "{}\t{}\t{}\t{}",
            entry.id,
            entry.last_activity_at.to_rfc3339(),
            entry.title.as_deref().unwrap_or(""),
            entry
                .host_cwd
                .as_ref()
                .map_or_else(String::new, |path| path.display().to_string())
        );
    }
    Ok(())
}

fn print_export_summary(summary: &ExportSummary) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": summary.id,
                "output": summary.output,
            }))?
        );
    } else {
        println!(
            "Exported session {} to {}",
            summary.id,
            summary.output.display()
        );
    }
    Ok(())
}

fn print_import_summary(summary: &ImportSummary) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": summary.id,
                "root": summary.root,
            }))?
        );
    } else {
        println!("Imported session {}", summary.id);
        println!("Root: {}", summary.root.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn root_args(root: &Path) -> Option<PathBuf> {
        Some(root.to_path_buf())
    }

    #[tokio::test]
    async fn create_session_writes_metadata_with_cwd() {
        let root = tempdir().unwrap();

        let summary = create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("session-one".to_string()),
            title: Some("Session One".to_string()),
            json: true,
        })
        .await
        .unwrap();

        assert_eq!(summary.id, "session-one");
        assert_eq!(summary.title.as_deref(), Some("Session One"));
        assert!(summary.host_cwd.is_some());
        let metadata =
            SessionMetadata::read_from_path(root.path().join("session-one").join("metadata.json"))
                .unwrap();
        assert_eq!(metadata.id.as_str(), "session-one");
        assert_eq!(metadata.title.as_deref(), Some("Session One"));
        assert!(metadata.host_cwd.is_some());
    }

    #[test]
    fn list_sessions_returns_empty_for_missing_root() {
        let root = tempdir().unwrap();
        let missing = root.path().join("missing");

        let entries = list_sessions(SessionListArgs {
            root: Some(missing),
            json: false,
        })
        .unwrap();

        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn list_sessions_ignores_children_without_valid_metadata() {
        let root = tempdir().unwrap();
        create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("valid-session".to_string()),
            title: None,
            json: false,
        })
        .await
        .unwrap();
        fs::create_dir(root.path().join("missing-metadata")).unwrap();
        fs::create_dir(root.path().join("bad-metadata")).unwrap();
        fs::write(
            root.path().join("bad-metadata").join("metadata.json"),
            b"not json",
        )
        .unwrap();
        fs::create_dir(root.path().join("alias-session")).unwrap();
        SessionMetadata::new(SessionId::parse("other-session").unwrap(), None, None)
            .write_to_path(root.path().join("alias-session").join("metadata.json"))
            .unwrap();

        let entries = list_sessions(SessionListArgs {
            root: root_args(root.path()),
            json: false,
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "valid-session");
    }

    #[tokio::test]
    async fn delete_session_removes_session_dir_and_missing_fails() {
        let root = tempdir().unwrap();
        create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("delete-me".to_string()),
            title: None,
            json: false,
        })
        .await
        .unwrap();

        let summary = delete_session(SessionDeleteArgs {
            root: root_args(root.path()),
            id: "delete-me".to_string(),
        })
        .unwrap();

        assert_eq!(summary.id, "delete-me");
        assert_eq!(summary.path, root.path().join("delete-me"));
        assert!(!root.path().join("delete-me").exists());
        assert!(delete_session(SessionDeleteArgs {
            root: root_args(root.path()),
            id: "delete-me".to_string(),
        })
        .is_err());
    }

    #[test]
    fn delete_session_rejects_non_session_dirs_and_mismatched_metadata() {
        let root = tempdir().unwrap();
        fs::create_dir(root.path().join("not-session")).unwrap();
        fs::create_dir(root.path().join("alias-session")).unwrap();
        SessionMetadata::new(SessionId::parse("other-session").unwrap(), None, None)
            .write_to_path(root.path().join("alias-session").join("metadata.json"))
            .unwrap();

        assert!(delete_session(SessionDeleteArgs {
            root: root_args(root.path()),
            id: "not-session".to_string(),
        })
        .is_err());
        assert!(root.path().join("not-session").is_dir());

        assert!(delete_session(SessionDeleteArgs {
            root: root_args(root.path()),
            id: "alias-session".to_string(),
        })
        .is_err());
        assert!(root.path().join("alias-session").is_dir());
    }

    #[tokio::test]
    async fn export_session_writes_pretty_snapshot_json() {
        let root = tempdir().unwrap();
        let output = root.path().join("exported").join("session.json");
        create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("export-me".to_string()),
            title: Some("Export me".to_string()),
            json: false,
        })
        .await
        .unwrap();

        let summary = export_session(SessionExportArgs {
            root: root_args(root.path()),
            id: "export-me".to_string(),
            output: output.clone(),
            json: true,
        })
        .await
        .unwrap();

        assert_eq!(summary.id, "export-me");
        let json = fs::read_to_string(&output).unwrap();
        assert!(json.contains('\n'));
        let snapshot: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot.metadata.id.as_str(), "export-me");
    }

    #[tokio::test]
    async fn import_session_uses_requested_or_generated_new_id() {
        let source_root = tempdir().unwrap();
        let target_root = tempdir().unwrap();
        let output = source_root.path().join("session.json");
        create_session(SessionCreateArgs {
            root: root_args(source_root.path()),
            id: Some("source-session".to_string()),
            title: Some("Source".to_string()),
            json: false,
        })
        .await
        .unwrap();
        export_session(SessionExportArgs {
            root: root_args(source_root.path()),
            id: "source-session".to_string(),
            output: output.clone(),
            json: false,
        })
        .await
        .unwrap();

        let requested = import_session(SessionImportArgs {
            root: root_args(target_root.path()),
            input: output.clone(),
            id: Some("imported-session".to_string()),
            json: true,
        })
        .await
        .unwrap();
        let generated = import_session(SessionImportArgs {
            root: root_args(target_root.path()),
            input: output,
            id: None,
            json: false,
        })
        .await
        .unwrap();

        assert_eq!(requested.id, "imported-session");
        assert!(target_root.path().join("imported-session").is_dir());
        assert_ne!(generated.id, "source-session");
        assert!(target_root.path().join(generated.id).is_dir());
    }
}
