//! Tolerant recovery for damaged local session artifacts.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::runtime::chat::{
    ChatProjector, ChatRuntimeConfig, RuntimeSnapshot, AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
};
use crate::session::store::{
    build_resource_manifest, resource_events_from_manifest, LocalSessionStore,
};
use crate::types::ModelMessage;

use super::{
    LocalProviderLedger, LocalSessionFs, LocalSessionResources, PathConventions, RuntimeCursor,
    RuntimeSnapshotCache, SessionConfig, SessionError, SessionId, SessionLease, SessionMetadata,
    SessionResourceManifest, SessionResult, SessionResumeState, SessionSnapshot, ThreadId,
};

mod events;
mod provider;

pub const RECOVERED_SESSION_ARTIFACT_TYPE: &str = "roci_recovered_session";
pub const RECOVERED_SESSION_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveredSession {
    pub artifact_type: String,
    pub schema_version: u16,
    pub snapshot: SessionSnapshot,
    pub report: RecoveryReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub importable_runtime_state: bool,
    pub sources: Vec<RecoverySourceReport>,
    pub warnings: Vec<RecoveryWarning>,
    pub stats: RecoveryStats,
    pub cache_preview: Option<RuntimeSnapshotCachePreview>,
    pub provider_context: ProviderRecoveryReport,
    pub resource_refs_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySourceReport {
    pub source: RecoverySource,
    pub path: PathBuf,
    pub status: RecoverySourceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySource {
    Metadata,
    EventsJsonl,
    ProviderLedgerJsonl,
    RuntimeSnapshotCache,
    Resources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySourceStatus {
    Missing,
    Read,
    RecoveredWithWarnings,
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryWarning {
    pub source: RecoverySource,
    pub line: Option<usize>,
    pub record_index: Option<usize>,
    pub severity: RecoverySeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoveryStats {
    pub events: RecoverySourceStats,
    pub provider_ledger: RecoverySourceStats,
    pub resources: RecoverySourceStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoverySourceStats {
    pub records_read: usize,
    pub records_recovered: usize,
    pub records_skipped: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshotCachePreview {
    pub parsed: bool,
    pub generated_at: Option<DateTime<Utc>>,
    pub thread_count: Option<usize>,
    pub latest_cursors: Vec<RuntimeCursor>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRecoveryReport {
    pub default_thread_id: ThreadId,
    pub recovered_threads: Vec<ThreadId>,
    pub imported_thread_id: ThreadId,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecoverySource {
    SessionId(SessionId),
    SessionDir {
        path: PathBuf,
        source_id: Option<SessionId>,
    },
}

pub(crate) struct RecoveredEvents {
    pub events: Vec<super::AgentRuntimeEvent>,
    pub first_thread_id: Option<ThreadId>,
    pub stats: RecoverySourceStats,
    pub warnings: Vec<RecoveryWarning>,
}

pub(crate) struct RecoveredProviderLedger {
    pub summary: super::ProviderLedgerSummary,
    pub report: ProviderRecoveryReport,
    pub stats: RecoverySourceStats,
    pub warnings: Vec<RecoveryWarning>,
}

pub(crate) struct RecoveredProviderLedgerScan {
    pub histories: HashMap<ThreadId, Vec<ModelMessage>>,
    pub recovered_threads: Vec<ThreadId>,
    pub latest_seq: u64,
    pub degraded: bool,
    pub stats: RecoverySourceStats,
    pub warnings: Vec<RecoveryWarning>,
}

/// Export a tolerant recovery artifact from local session storage.
pub async fn recover_export_from_store(
    store: &LocalSessionStore,
    source: SessionRecoverySource,
) -> SessionResult<RecoveredSession> {
    let resolved = ResolvedRecoverySource::new(store, source);
    let conventions = PathConventions::new(resolved.path.clone());
    let mut warnings = Vec::new();
    let mut sources = Vec::new();

    let (metadata, metadata_importable, metadata_status) =
        recover_metadata(&conventions, resolved.source_id, &mut warnings);
    sources.push(RecoverySourceReport {
        source: RecoverySource::Metadata,
        path: conventions.metadata_file(),
        status: metadata_status,
    });

    let (cache_preview, cache_default_thread_id, cache_status) =
        read_runtime_cache_preview(&conventions, &mut warnings);
    sources.push(RecoverySourceReport {
        source: RecoverySource::RuntimeSnapshotCache,
        path: conventions.runtime_snapshot_file(),
        status: cache_status,
    });
    let recovered_events = events::recover_events(conventions.events_file())?;
    sources.push(RecoverySourceReport {
        source: RecoverySource::EventsJsonl,
        path: conventions.events_file(),
        status: source_status_for_scan(
            &conventions.events_file(),
            recovered_events.stats.records_read,
            recovered_events.stats.warnings,
        ),
    });
    warnings.extend(recovered_events.warnings.clone());

    let mut provider_scan = provider::recover_provider_ledger(conventions.provider_ledger_file())?;
    let provider_thread_ids = provider_scan.recovered_threads.clone();
    let (default_thread_id, default_thread_importable) = choose_default_thread_id(
        cache_default_thread_id,
        recovered_events.first_thread_id,
        &provider_thread_ids,
        &mut provider_scan,
    );
    let recovered_provider = provider_scan.into_recovered_provider_ledger(default_thread_id);
    sources.push(RecoverySourceReport {
        source: RecoverySource::ProviderLedgerJsonl,
        path: conventions.provider_ledger_file(),
        status: source_status_for_scan(
            &conventions.provider_ledger_file(),
            recovered_provider.stats.records_read,
            recovered_provider.stats.warnings,
        ),
    });
    warnings.extend(recovered_provider.warnings.clone());

    let (runtime, projection_importable) = project_recovered_events(
        &conventions,
        default_thread_id,
        &recovered_events.events,
        &mut warnings,
    );
    let importable_runtime_state =
        metadata_importable && projection_importable && default_thread_importable;

    let resources = if importable_runtime_state {
        build_resource_manifest(&conventions, &runtime)
    } else {
        SessionResourceManifest::default()
    };
    let resource_stats = resource_stats(&resources, importable_runtime_state);
    sources.push(RecoverySourceReport {
        source: RecoverySource::Resources,
        path: conventions.root().to_path_buf(),
        status: if importable_runtime_state {
            RecoverySourceStatus::Read
        } else {
            RecoverySourceStatus::Unusable
        },
    });

    let snapshot = SessionSnapshot {
        schema_version: 1,
        metadata,
        default_thread_id,
        runtime,
        events: recovered_events.events,
        provider_ledger: recovered_provider.summary,
        resources,
        exported_at: Utc::now(),
    };

    Ok(RecoveredSession {
        artifact_type: RECOVERED_SESSION_ARTIFACT_TYPE.to_string(),
        schema_version: RECOVERED_SESSION_SCHEMA_VERSION,
        snapshot,
        report: RecoveryReport {
            importable_runtime_state,
            sources,
            warnings,
            stats: RecoveryStats {
                events: recovered_events.stats,
                provider_ledger: recovered_provider.stats,
                resources: resource_stats,
            },
            cache_preview,
            provider_context: recovered_provider.report,
            resource_refs_only: true,
        },
    })
}

/// Import a validated recovery artifact into local session storage.
pub async fn recover_import_into_store(
    store: &LocalSessionStore,
    recovered: RecoveredSession,
    target_id: SessionId,
) -> SessionResult<SessionResumeState> {
    validate_recovered_artifact(&recovered)?;

    let default_thread_id = recovered.snapshot.default_thread_id;
    let provider_history = recovered.snapshot.provider_ledger.effective_history.clone();
    let target_config = SessionConfig::new(target_id.clone(), store.root());
    let target_dir = target_config.conventions().root().to_path_buf();
    std::fs::create_dir_all(store.root())
        .map_err(|source| SessionError::io(store.root(), source))?;
    let lease = SessionLease::acquire(target_dir.clone())?;
    if target_dir.exists() {
        return Err(SessionError::AlreadyExists { path: target_dir });
    }

    let staging_dir = create_unique_staging_dir(store.root(), &target_id)?;

    if let Err(error) = write_recovered_staging(&staging_dir, &recovered, target_id.clone()).await {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(error);
    }

    if let Err(source) = std::fs::rename(&staging_dir, &target_dir) {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(SessionError::io(&target_dir, source));
    }

    match store
        .load_state(target_config, default_thread_id, lease)
        .await
    {
        Ok(state)
            if state.default_thread_id == default_thread_id
                && state.model_messages == provider_history =>
        {
            Ok(state)
        }
        Ok(state) => {
            drop(state);
            let _ = std::fs::remove_dir_all(&target_dir);
            Err(SessionError::InvalidRecoveredSession {
                message: "imported recovery did not replay expected provider context".to_string(),
            })
        }
        Err(error) => {
            let _ = std::fs::remove_dir_all(&target_dir);
            Err(error)
        }
    }
}

fn create_unique_staging_dir(root: &Path, target_id: &SessionId) -> SessionResult<PathBuf> {
    for _ in 0..8 {
        let staging_dir = root.join(format!(".recovering-{target_id}-{}", SessionId::new_v4()));
        match std::fs::create_dir(&staging_dir) {
            Ok(()) => return Ok(staging_dir),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(SessionError::io(&staging_dir, source)),
        }
    }
    Err(SessionError::AlreadyExists {
        path: root.join(format!(".recovering-{target_id}")),
    })
}

struct ResolvedRecoverySource {
    path: PathBuf,
    source_id: Option<SessionId>,
}

impl ResolvedRecoverySource {
    fn new(store: &LocalSessionStore, source: SessionRecoverySource) -> Self {
        match source {
            SessionRecoverySource::SessionId(id) => Self {
                path: store.root().join(id.as_str()),
                source_id: Some(id),
            },
            SessionRecoverySource::SessionDir { path, source_id } => {
                let source_id = source_id.or_else(|| source_id_from_basename(&path));
                Self { path, source_id }
            }
        }
    }
}

fn source_id_from_basename(path: &Path) -> Option<SessionId> {
    path.file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| SessionId::parse(value).ok())
}

fn recover_metadata(
    conventions: &PathConventions,
    source_id: Option<SessionId>,
    warnings: &mut Vec<RecoveryWarning>,
) -> (SessionMetadata, bool, RecoverySourceStatus) {
    match SessionMetadata::read_from_path(conventions.metadata_file()) {
        Ok(metadata) => (metadata, true, RecoverySourceStatus::Read),
        Err(error) => {
            let has_source_id = source_id.is_some();
            let missing = matches!(
                &error,
                SessionError::Io { source, .. } if source.kind() == ErrorKind::NotFound
            );
            warnings.push(RecoveryWarning {
                source: RecoverySource::Metadata,
                line: None,
                record_index: None,
                severity: if has_source_id {
                    RecoverySeverity::Warning
                } else {
                    RecoverySeverity::Error
                },
                code: if missing {
                    "metadata_missing".to_string()
                } else {
                    "metadata_unusable".to_string()
                },
                message: error.to_string(),
            });

            let fallback_id = source_id.unwrap_or_else(|| {
                SessionId::parse("unidentified-recovered-session").expect("valid fallback id")
            });
            let metadata =
                SessionMetadata::new(fallback_id, None, Some(conventions.root().to_path_buf()));
            let status = if has_source_id {
                RecoverySourceStatus::RecoveredWithWarnings
            } else if missing {
                RecoverySourceStatus::Missing
            } else {
                RecoverySourceStatus::Unusable
            };
            (metadata, has_source_id, status)
        }
    }
}

fn read_runtime_cache_preview(
    conventions: &PathConventions,
    warnings: &mut Vec<RecoveryWarning>,
) -> (
    Option<RuntimeSnapshotCachePreview>,
    Option<ThreadId>,
    RecoverySourceStatus,
) {
    let path = conventions.runtime_snapshot_file();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return (None, None, RecoverySourceStatus::Missing);
        }
        Err(source) => {
            warnings.push(cache_warning(
                RecoverySeverity::Warning,
                "runtime_snapshot_cache_unreadable",
                source.to_string(),
            ));
            return (
                Some(RuntimeSnapshotCachePreview {
                    parsed: false,
                    generated_at: None,
                    thread_count: None,
                    latest_cursors: Vec::new(),
                    parse_error: Some(source.to_string()),
                }),
                None,
                RecoverySourceStatus::Unusable,
            );
        }
    };

    match serde_json::from_slice::<RuntimeSnapshotCache>(&bytes) {
        Ok(cache) => {
            warnings.push(cache_warning(
                RecoverySeverity::Info,
                "runtime_snapshot_cache_preview_only",
                "runtime snapshot cache is diagnostic only",
            ));
            (
                Some(RuntimeSnapshotCachePreview {
                    parsed: true,
                    generated_at: Some(cache.generated_at),
                    thread_count: Some(cache.runtime.threads.len()),
                    latest_cursors: cache.latest_cursors,
                    parse_error: None,
                }),
                Some(cache.default_thread_id),
                RecoverySourceStatus::Read,
            )
        }
        Err(source) => {
            warnings.push(cache_warning(
                RecoverySeverity::Warning,
                "runtime_snapshot_cache_malformed",
                source.to_string(),
            ));
            (
                Some(RuntimeSnapshotCachePreview {
                    parsed: false,
                    generated_at: None,
                    thread_count: None,
                    latest_cursors: Vec::new(),
                    parse_error: Some(source.to_string()),
                }),
                None,
                RecoverySourceStatus::Unusable,
            )
        }
    }
}

fn cache_warning(
    severity: RecoverySeverity,
    code: &'static str,
    message: impl Into<String>,
) -> RecoveryWarning {
    RecoveryWarning {
        source: RecoverySource::RuntimeSnapshotCache,
        line: None,
        record_index: None,
        severity,
        code: code.to_string(),
        message: message.into(),
    }
}

fn choose_default_thread_id(
    cache_default_thread_id: Option<ThreadId>,
    first_event_thread_id: Option<ThreadId>,
    provider_thread_ids: &[ThreadId],
    provider_scan: &mut RecoveredProviderLedgerScan,
) -> (ThreadId, bool) {
    if let Some(thread_id) = cache_default_thread_id {
        return (thread_id, true);
    }
    if let Some(thread_id) = first_event_thread_id {
        return (thread_id, true);
    }
    if let [thread_id] = provider_thread_ids {
        return (*thread_id, true);
    }
    if provider_thread_ids.len() > 1 {
        provider_scan.warn(
            None,
            None,
            "provider_default_thread_ambiguous",
            format!(
                "provider ledger has {} recovered threads but no runtime cache or event default thread",
                provider_thread_ids.len()
            ),
            true,
            RecoverySeverity::Error,
        );
        return (ThreadId::nil(), false);
    }
    (ThreadId::default(), true)
}

impl RecoveredProviderLedgerScan {
    fn into_recovered_provider_ledger(
        self,
        default_thread_id: ThreadId,
    ) -> RecoveredProviderLedger {
        let effective_history = self
            .histories
            .get(&default_thread_id)
            .cloned()
            .unwrap_or_default();
        RecoveredProviderLedger {
            summary: super::ProviderLedgerSummary {
                thread_id: default_thread_id,
                latest_seq: self.latest_seq,
                effective_history,
            },
            report: ProviderRecoveryReport {
                default_thread_id,
                recovered_threads: self.recovered_threads,
                imported_thread_id: default_thread_id,
                degraded: self.degraded,
            },
            stats: self.stats,
            warnings: self.warnings,
        }
    }

    fn warn(
        &mut self,
        line: Option<usize>,
        record_index: Option<usize>,
        code: &'static str,
        message: impl Into<String>,
        marks_degraded: bool,
        severity: RecoverySeverity,
    ) {
        self.degraded |= marks_degraded;
        self.stats.warnings += 1;
        self.warnings.push(RecoveryWarning {
            source: RecoverySource::ProviderLedgerJsonl,
            line,
            record_index,
            severity,
            code: code.to_string(),
            message: message.into(),
        });
    }
}

fn project_recovered_events(
    conventions: &PathConventions,
    default_thread_id: ThreadId,
    events: &[super::AgentRuntimeEvent],
    warnings: &mut Vec<RecoveryWarning>,
) -> (RuntimeSnapshot, bool) {
    match ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(default_thread_id),
            ..ChatRuntimeConfig::default()
        },
        events.iter().cloned(),
    ) {
        Ok(projector) => (projector.read_snapshot(), true),
        Err(source) => {
            warnings.push(RecoveryWarning {
                source: RecoverySource::EventsJsonl,
                line: None,
                record_index: None,
                severity: RecoverySeverity::Error,
                code: "events_projection_failed".to_string(),
                message: format!(
                    "runtime events from {} did not project: {source}",
                    conventions.events_file().display()
                ),
            });
            (
                RuntimeSnapshot {
                    schema_version: AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
                    threads: Vec::new(),
                },
                false,
            )
        }
    }
}

fn source_status_for_scan(
    path: &Path,
    records_read: usize,
    warnings: usize,
) -> RecoverySourceStatus {
    if !path.exists() && records_read == 0 {
        RecoverySourceStatus::Missing
    } else if warnings > 0 {
        RecoverySourceStatus::RecoveredWithWarnings
    } else {
        RecoverySourceStatus::Read
    }
}

fn resource_stats(manifest: &SessionResourceManifest, importable: bool) -> RecoverySourceStats {
    if !importable {
        return RecoverySourceStats::default();
    }
    let count = manifest.plan.iter().count()
        + manifest.workspace.iter().count()
        + manifest.artifacts.len()
        + manifest.temp_files.len()
        + manifest.checkpoints.len()
        + manifest.files.len();
    RecoverySourceStats {
        records_read: count,
        records_recovered: count,
        records_skipped: 0,
        warnings: 0,
    }
}

fn validate_recovered_artifact(recovered: &RecoveredSession) -> SessionResult<()> {
    if recovered.artifact_type != RECOVERED_SESSION_ARTIFACT_TYPE {
        return Err(SessionError::InvalidRecoveredSession {
            message: format!(
                "expected artifact_type {RECOVERED_SESSION_ARTIFACT_TYPE}, got {}",
                recovered.artifact_type
            ),
        });
    }
    if recovered.schema_version != RECOVERED_SESSION_SCHEMA_VERSION {
        return Err(SessionError::InvalidRecoveredSession {
            message: format!("unsupported schema version {}", recovered.schema_version),
        });
    }
    if !recovered.report.importable_runtime_state {
        return Err(SessionError::NonImportableRecovery {
            message: "recovered runtime state is diagnostic only".to_string(),
        });
    }
    if recovered.snapshot.provider_ledger.thread_id != recovered.snapshot.default_thread_id {
        return Err(SessionError::InvalidRecoveredSession {
            message: "provider ledger thread must match default thread".to_string(),
        });
    }
    Ok(())
}

async fn write_recovered_staging(
    staging_dir: &Path,
    recovered: &RecoveredSession,
    target_id: SessionId,
) -> SessionResult<()> {
    let snapshot = &recovered.snapshot;
    let staging_conventions = PathConventions::new(staging_dir.to_path_buf());
    std::fs::create_dir_all(staging_conventions.root())
        .map_err(|source| SessionError::io(staging_conventions.root(), source))?;
    LocalSessionFs::with_conventions(staging_conventions.clone())?;
    LocalSessionResources::with_conventions(staging_conventions.clone())?;

    validate_events_project(
        &staging_conventions,
        snapshot.default_thread_id,
        &snapshot.events,
    )?;

    let mut metadata = snapshot.metadata.clone();
    metadata.id = target_id;
    metadata.updated_at = Utc::now();
    metadata.last_activity_at = metadata.updated_at;
    metadata.write_to_path(staging_conventions.metadata_file())?;

    write_strict_events(&staging_conventions, snapshot).await?;
    write_strict_provider_ledger(&staging_conventions, snapshot)?;

    Ok(())
}

fn validate_events_project(
    conventions: &PathConventions,
    default_thread_id: ThreadId,
    events: &[super::AgentRuntimeEvent],
) -> SessionResult<()> {
    ChatProjector::from_events(
        ChatRuntimeConfig {
            default_thread_id: Some(default_thread_id),
            ..ChatRuntimeConfig::default()
        },
        events.iter().cloned(),
    )
    .map_err(|source| SessionError::RuntimeProjection {
        path: conventions.events_file(),
        message: source.to_string(),
    })?;
    Ok(())
}

async fn write_strict_events(
    conventions: &PathConventions,
    snapshot: &SessionSnapshot,
) -> SessionResult<()> {
    let events_path = conventions.events_file();
    let event_store = crate::agent::runtime::chat::JsonlAgentRuntimeEventStore::open(&events_path)
        .map_err(|source| SessionError::RuntimeProjection {
            path: events_path.clone(),
            message: source.to_string(),
        })?;
    let mut events = snapshot.events.clone();
    let next_seq = events
        .iter()
        .filter(|event| event.thread_id == snapshot.default_thread_id)
        .map(|event| event.seq)
        .max()
        .unwrap_or(0)
        + 1;
    events.extend(resource_events_from_manifest(
        snapshot.default_thread_id,
        next_seq,
        &snapshot.resources,
    ));
    events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));

    validate_events_project(conventions, snapshot.default_thread_id, &events)?;

    for event in events {
        use crate::agent::runtime::chat::AgentRuntimeEventStore;
        event_store
            .append(event)
            .await
            .map_err(|source| SessionError::RuntimeProjection {
                path: conventions.events_file(),
                message: source.to_string(),
            })?;
    }
    Ok(())
}

fn write_strict_provider_ledger(
    conventions: &PathConventions,
    snapshot: &SessionSnapshot,
) -> SessionResult<()> {
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())?;
    ledger.append_compacted(
        snapshot.provider_ledger.thread_id,
        snapshot.provider_ledger.effective_history.clone(),
    )?;
    drop(ledger);

    let replayed = LocalProviderLedger::open(conventions.provider_ledger_file())?;
    let state = replayed.state_for_thread(snapshot.provider_ledger.thread_id);
    if state.effective_history != snapshot.provider_ledger.effective_history {
        return Err(SessionError::InvalidProviderLedger {
            path: conventions.provider_ledger_file(),
            message: "recovered provider summary did not replay strictly".to_string(),
        });
    }
    Ok(())
}
