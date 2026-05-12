use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use roci::session::{
    CreateSessionOptions, ImportPolicy, LocalSessionStore, PathConventions, RecoveredSession,
    RecoveryReport, SessionId, SessionMetadata, SessionRecoverySource, SessionSnapshot,
    RECOVERED_SESSION_ARTIFACT_TYPE,
};

use crate::cli::{
    SessionArgs, SessionCommands, SessionCreateArgs, SessionDeleteArgs, SessionExportArgs,
    SessionImportArgs, SessionListArgs, SessionRecoverExportArgs, SessionRecoverImportArgs,
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
        SessionCommands::RecoverExport(args) => {
            let summary = recover_export_session(args).await?;
            print_recover_export_summary(&summary)?;
        }
        SessionCommands::RecoverImport(args) => {
            let summary = recover_import_session(args).await?;
            print_recover_import_summary(&summary)?;
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

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct RecoverExportSummary {
    id: String,
    source: String,
    output: PathBuf,
    root: PathBuf,
    importable_runtime_state: bool,
    warning_count: usize,
    recovered_events: usize,
    recovered_provider_ledger_records: usize,
    recovered_resources_records: usize,
    report: RecoveryReport,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct RecoverImportSummary {
    id: String,
    root: PathBuf,
    importable_runtime_state: bool,
    warning_count: usize,
    recovered_events: usize,
    recovered_provider_ledger_records: usize,
    recovered_resources_records: usize,
    report: RecoveryReport,
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
    write_pretty_json_any(&args.output, &snapshot)?;
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

async fn recover_export_session(
    args: SessionRecoverExportArgs,
) -> Result<RecoverExportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let source = match (args.id, args.session_dir) {
        (Some(id), None) => SessionRecoverySource::SessionId(SessionId::parse(id)?),
        (None, Some(session_dir)) => SessionRecoverySource::SessionDir {
            path: session_dir,
            source_id: args.source_id.map(SessionId::parse).transpose()?,
        },
        (None, None) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "session id or --session-dir required",
            )
            .into());
        }
        (Some(_), Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--session-dir cannot be used with positional session id",
            )
            .into());
        }
    };
    let source = source.clone();
    let source_label = match &source {
        SessionRecoverySource::SessionId(id) => format!("session id {id}"),
        SessionRecoverySource::SessionDir { path, source_id } => match source_id {
            Some(id) => format!("session-dir {path:?} with source-id {id}"),
            None => format!("session-dir {path:?}"),
        },
    };
    let store = LocalSessionStore::new(root.clone());
    let recovered = store.recover_export(source).await?;
    let report = recovered.report.clone();
    write_pretty_json_any(&args.output, &recovered)?;
    Ok(RecoverExportSummary {
        id: recovered.snapshot.metadata.id.to_string(),
        source: source_label,
        output: args.output,
        root,
        importable_runtime_state: report.importable_runtime_state,
        warning_count: report.warnings.len(),
        recovered_events: report.stats.events.records_recovered,
        recovered_provider_ledger_records: report.stats.provider_ledger.records_recovered,
        recovered_resources_records: report.stats.resources.records_recovered,
        report: recovered.report,
        json: args.json,
    })
}

async fn recover_import_session(
    args: SessionRecoverImportArgs,
) -> Result<RecoverImportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let target_id = SessionId::parse(args.id)?;
    let bytes = fs::read(&args.input)?;
    let recovered_value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let artifact_type = recovered_value
        .get("artifact_type")
        .and_then(|value| value.as_str());
    if artifact_type != Some(RECOVERED_SESSION_ARTIFACT_TYPE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "input is not a recovered session artifact. Use `session import` for plain SessionSnapshot exports.",
        )
        .into());
    }
    let recovered: RecoveredSession = serde_json::from_value(recovered_value)?;
    let report = recovered.report.clone();
    let recovered_events = report.stats.events.records_recovered;
    let recovered_provider_ledger_records = report.stats.provider_ledger.records_recovered;
    let recovered_resources_records = report.stats.resources.records_recovered;
    let warning_count = report.warnings.len();
    let importable_runtime_state = report.importable_runtime_state;
    let store = LocalSessionStore::new(root.clone());
    let state = store.recover_import(recovered, target_id).await?;
    Ok(RecoverImportSummary {
        id: state.metadata.id.to_string(),
        root,
        importable_runtime_state,
        warning_count,
        recovered_events,
        recovered_provider_ledger_records,
        recovered_resources_records,
        report,
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

fn write_pretty_json_any<T: serde::Serialize>(
    path: &Path,
    value: &T,
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

fn print_recover_export_summary(
    summary: &RecoverExportSummary,
) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!("{}", serde_json::to_string_pretty(summary)?);
        return Ok(());
    }
    println!(
        "Recovered session {} from {} to {}",
        summary.id,
        summary.source,
        summary.output.display()
    );
    println!(
        "Importable runtime state: {}",
        summary.importable_runtime_state
    );
    println!(
        "Warnings: {} | Recovered events: {} | Recovered provider ledger records: {} | Recovered resources: {}",
        summary.warning_count,
        summary.recovered_events,
        summary.recovered_provider_ledger_records,
        summary.recovered_resources_records
    );
    Ok(())
}

fn print_recover_import_summary(
    summary: &RecoverImportSummary,
) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!("{}", serde_json::to_string_pretty(summary)?);
        return Ok(());
    }
    println!("Imported recovered session {}", summary.id);
    println!("Root: {}", summary.root.display());
    println!(
        "Importable runtime state: {} | Warnings: {} | Recovered events: {}",
        summary.importable_runtime_state, summary.warning_count, summary.recovered_events
    );
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

    #[tokio::test]
    async fn recover_export_and_import_session_round_trip() {
        let root = tempdir().unwrap();
        let output = root.path().join("recovered.json");
        create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("source-session".to_string()),
            title: Some("Source session".to_string()),
            json: false,
        })
        .await
        .unwrap();

        let conventions =
            PathConventions::for_session(root.path(), &SessionId::parse("source-session").unwrap());
        fs::write(conventions.events_file(), b"not json\n").unwrap();

        let export_summary = recover_export_session(SessionRecoverExportArgs {
            root: root_args(root.path()),
            id: Some("source-session".to_string()),
            session_dir: None,
            source_id: None,
            output: output.clone(),
            json: true,
        })
        .await
        .unwrap();

        assert_eq!(export_summary.id, "source-session");
        assert!(output.exists());
        assert!(!export_summary.report.warnings.is_empty());

        let import_summary = recover_import_session(SessionRecoverImportArgs {
            root: root_args(root.path()),
            input: output,
            id: "imported-session".to_string(),
            json: false,
        })
        .await
        .unwrap();

        assert_eq!(import_summary.id, "imported-session");
        assert!(root
            .path()
            .join("imported-session")
            .join("metadata.json")
            .is_file());
    }

    #[tokio::test]
    async fn recover_import_plain_snapshot_reports_session_import_hint() {
        let root = tempdir().unwrap();
        let output = root.path().join("snapshot.json");
        create_session(SessionCreateArgs {
            root: root_args(root.path()),
            id: Some("source-session".to_string()),
            title: Some("Source".to_string()),
            json: false,
        })
        .await
        .unwrap();
        export_session(SessionExportArgs {
            root: root_args(root.path()),
            id: "source-session".to_string(),
            output: output.clone(),
            json: false,
        })
        .await
        .unwrap();

        let err = recover_import_session(SessionRecoverImportArgs {
            root: root_args(root.path()),
            input: output,
            id: "imported-session".to_string(),
            json: false,
        })
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("session import"));
        assert!(
            message.contains("SessionSnapshot") || message.contains("plain session snapshot"),
            "{message}"
        );
    }
}
