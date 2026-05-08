use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;

use crate::agent::runtime::chat::{
    AgentRuntimeEvent, AgentRuntimeEventPayload, AgentRuntimeEventStore, ChatProjector,
    ChatRuntimeConfig, JsonlAgentRuntimeEventStore, RuntimeCursor, RuntimeSnapshot,
    SessionResourceSnapshot, ThreadId, AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
};

use super::{
    CreateSessionOptions, ImportPolicy, LocalProviderLedger, LocalSessionFs, LocalSessionResources,
    PathConventions, ProviderLedgerSummary, RuntimeSnapshotCache, SessionConfig, SessionError,
    SessionLease, SessionMetadata, SessionResourceManifest, SessionResourceNamespace,
    SessionResourceRef, SessionResult, SessionResumeState, SessionSnapshot,
};

#[derive(Debug, Clone)]
pub struct LocalSessionStore {
    root: PathBuf,
}

impl LocalSessionStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a new local session and return prepared resume state.
    ///
    /// # Errors
    ///
    /// Returns an error if the session already exists or files cannot be written.
    pub async fn create(&self, options: CreateSessionOptions) -> SessionResult<SessionResumeState> {
        let id = options.id.unwrap_or_else(super::SessionId::new_v4);
        let config = SessionConfig::new(id.clone(), &self.root);
        let conventions = config.conventions();
        std::fs::create_dir_all(&self.root)
            .map_err(|source| SessionError::io(&self.root, source))?;
        let lease = SessionLease::acquire(conventions.root().to_path_buf())?;
        if conventions.root().exists() {
            return Err(SessionError::AlreadyExists {
                path: conventions.root().to_path_buf(),
            });
        }

        LocalSessionFs::with_conventions(conventions.clone())?;
        LocalSessionResources::with_conventions(conventions.clone())?;

        let mut metadata = SessionMetadata::new(id, options.host_cwd, options.import_source);
        metadata.title = options.title;
        metadata.write_to_path(conventions.metadata_file())?;
        write_empty_file(&conventions.events_file())?;
        write_empty_file(&conventions.provider_ledger_file())?;

        let default_thread_id = options.default_thread_id.unwrap_or_default();
        self.load_state(config, default_thread_id, lease).await
    }

    /// Open an existing local session and return prepared resume state.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata/log replay fails or another writer is open.
    pub async fn open(&self, id: super::SessionId) -> SessionResult<SessionResumeState> {
        let config = SessionConfig::new(id, &self.root);
        let lease = SessionLease::acquire(config.conventions().root().to_path_buf())?;
        let default_thread_id = default_thread_id_from_cache_or_events(&config).await?;
        self.load_state(config, default_thread_id, lease).await
    }

    /// Export a manifest snapshot without resource bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if session open/replay fails.
    pub async fn export_snapshot(&self, id: super::SessionId) -> SessionResult<SessionSnapshot> {
        let state = self.open(id).await?;
        Ok(SessionSnapshot {
            schema_version: 1,
            metadata: state.metadata,
            default_thread_id: state.default_thread_id,
            runtime: state.runtime,
            events: state.events,
            provider_ledger: ProviderLedgerSummary {
                thread_id: state.default_thread_id,
                latest_seq: state.provider_ledger_seq,
                effective_history: state.model_messages,
            },
            resources: state.resources,
            exported_at: Utc::now(),
        })
    }

    /// Import a manifest snapshot into a new local session.
    ///
    /// # Errors
    ///
    /// Returns an error if the target exists or canonical logs cannot be written.
    pub async fn import_snapshot(
        &self,
        snapshot: SessionSnapshot,
        policy: ImportPolicy,
    ) -> SessionResult<SessionResumeState> {
        let target_id = match policy {
            ImportPolicy::FailIfExists => snapshot.metadata.id.clone(),
            ImportPolicy::NewId(Some(id)) => id,
            ImportPolicy::NewId(None) => super::SessionId::new_v4(),
        };
        let mut state = self
            .create(CreateSessionOptions {
                id: Some(target_id.clone()),
                title: snapshot.metadata.title.clone(),
                host_cwd: snapshot.metadata.host_cwd.clone(),
                import_source: snapshot.metadata.import_source.clone(),
                default_thread_id: Some(snapshot.default_thread_id),
            })
            .await?;
        let config = state.session_config.clone();
        let conventions = config.conventions();

        let event_store =
            JsonlAgentRuntimeEventStore::open(conventions.events_file()).map_err(|err| {
                SessionError::RuntimeProjection {
                    path: conventions.events_file(),
                    message: err.to_string(),
                }
            })?;
        let mut events = snapshot.events;
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
        for event in events {
            event_store
                .append(event)
                .await
                .map_err(|err| SessionError::RuntimeProjection {
                    path: conventions.events_file(),
                    message: err.to_string(),
                })?;
        }

        let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())?;
        if !snapshot.provider_ledger.effective_history.is_empty() {
            ledger.append_compacted(
                snapshot.provider_ledger.thread_id,
                snapshot.provider_ledger.effective_history,
            )?;
        }

        let lease = state.lease.clone();
        state = self
            .load_state(config, snapshot.default_thread_id, lease)
            .await?;
        Ok(state)
    }

    async fn load_state(
        &self,
        config: SessionConfig,
        default_thread_id: ThreadId,
        lease: Arc<SessionLease>,
    ) -> SessionResult<SessionResumeState> {
        let conventions = config.conventions();
        let metadata = SessionMetadata::read_from_path(conventions.metadata_file())?;
        if metadata.id != config.id {
            return Err(SessionError::InvalidMetadata {
                path: conventions.metadata_file(),
                message: "metadata id does not match session config id".to_string(),
            });
        }

        let event_store =
            JsonlAgentRuntimeEventStore::open(conventions.events_file()).map_err(|err| {
                SessionError::RuntimeProjection {
                    path: conventions.events_file(),
                    message: err.to_string(),
                }
            })?;
        let mut events = event_store.all_events().await;
        let mut projector = ChatProjector::from_events(
            ChatRuntimeConfig {
                default_thread_id: Some(default_thread_id),
                ..ChatRuntimeConfig::default()
            },
            events.clone(),
        )
        .map_err(|err| SessionError::RuntimeProjection {
            path: conventions.events_file(),
            message: err.to_string(),
        })?;
        let normalized =
            projector
                .normalize_for_resume()
                .map_err(|err| SessionError::RuntimeProjection {
                    path: conventions.events_file(),
                    message: err.to_string(),
                })?;
        for event in &normalized {
            event_store.append(event.clone()).await.map_err(|err| {
                SessionError::RuntimeProjection {
                    path: conventions.events_file(),
                    message: err.to_string(),
                }
            })?;
        }
        events.extend(normalized);

        let mut runtime = projector.read_snapshot();
        let resources = build_resource_manifest(&conventions, &runtime);
        scrub_unavailable_runtime_resources(&mut runtime, &resources);

        let event_cursors = runtime
            .threads
            .iter()
            .map(|thread| RuntimeCursor::new(thread.thread_id, thread.last_seq))
            .collect::<Vec<_>>();
        write_json_atomic(
            &conventions.runtime_snapshot_file(),
            &RuntimeSnapshotCache {
                schema_version: 1,
                default_thread_id,
                runtime: runtime.clone(),
                latest_cursors: event_cursors.clone(),
                generated_at: Utc::now(),
            },
        )?;

        let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())?;
        let ledger_state = ledger.state_for_thread(default_thread_id);
        write_json_atomic(
            &conventions.provider_ledger_snapshot_file(),
            &super::ProviderLedgerSnapshot {
                schema_version: 1,
                thread_id: default_thread_id,
                latest_seq: ledger_state.latest_seq,
                effective_history: ledger_state.effective_history.clone(),
                generated_at: Utc::now(),
            },
        )?;

        Ok(SessionResumeState {
            session_config: config,
            metadata,
            default_thread_id,
            runtime,
            model_messages: ledger_state.effective_history,
            resources,
            events,
            event_cursors,
            provider_ledger_seq: ledger_state.latest_seq,
            lease,
        })
    }
}

async fn default_thread_id_from_cache_or_events(config: &SessionConfig) -> SessionResult<ThreadId> {
    let conventions = config.conventions();
    if let Ok(bytes) = std::fs::read(conventions.runtime_snapshot_file()) {
        if let Ok(cache) = serde_json::from_slice::<RuntimeSnapshotCache>(&bytes) {
            return Ok(cache.default_thread_id);
        }
    }
    let event_store =
        JsonlAgentRuntimeEventStore::open(conventions.events_file()).map_err(|err| {
            SessionError::RuntimeProjection {
                path: conventions.events_file(),
                message: err.to_string(),
            }
        })?;
    let events = event_store.all_events().await;
    if let Some(event) = events.first() {
        return Ok(event.thread_id);
    }
    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())?;
    Ok(ledger.state().latest_thread_id.unwrap_or_default())
}

fn write_empty_file(path: &Path) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SessionError::io(parent, source))?;
    }
    std::fs::write(path, []).map_err(|source| SessionError::io(path, source))
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SessionError::io(parent, source))?;
    }
    let tmp = path.with_extension("tmp");
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|source| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        })?;
    std::fs::write(&tmp, bytes).map_err(|source| SessionError::io(&tmp, source))?;
    std::fs::rename(&tmp, path).map_err(|source| SessionError::io(path, source))
}

fn build_resource_manifest(
    conventions: &PathConventions,
    runtime: &RuntimeSnapshot,
) -> SessionResourceManifest {
    let mut manifest = SessionResourceManifest::default();
    merge_runtime_resources(&mut manifest, runtime, conventions);
    merge_filesystem_resources(&mut manifest, conventions);
    manifest
}

fn merge_runtime_resources(
    manifest: &mut SessionResourceManifest,
    runtime: &RuntimeSnapshot,
    conventions: &PathConventions,
) {
    for thread in &runtime.threads {
        if let Some(resource) = &thread.resources.plan {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
        if let Some(resource) = &thread.resources.workspace {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
        for resource in &thread.resources.artifacts {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
        for resource in &thread.resources.temp_files {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
        for resource in &thread.resources.checkpoints {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
        for resource in &thread.resources.files {
            upsert_ref(manifest, ref_from_snapshot(resource, conventions, false));
        }
    }
}

fn merge_filesystem_resources(
    manifest: &mut SessionResourceManifest,
    conventions: &PathConventions,
) {
    if let Some(resource) = ref_if_file(
        SessionResourceNamespace::Plan,
        None,
        conventions.root(),
        conventions.plan_file(),
    ) {
        upsert_ref(manifest, resource);
    }
    if let Some(resource) = ref_if_file(
        SessionResourceNamespace::Workspace,
        None,
        conventions.root(),
        conventions.workspace_file(),
    ) {
        upsert_ref(manifest, resource);
    }
    scan_namespace(
        manifest,
        SessionResourceNamespace::Artifacts,
        conventions.root(),
        conventions.artifacts_dir(),
        conventions.artifacts_dir(),
    );
    scan_namespace(
        manifest,
        SessionResourceNamespace::Temp,
        conventions.root(),
        conventions.temp_dir(),
        conventions.temp_dir(),
    );
    scan_namespace(
        manifest,
        SessionResourceNamespace::Checkpoints,
        conventions.root(),
        conventions.checkpoints_dir(),
        conventions.checkpoints_dir(),
    );
    scan_namespace(
        manifest,
        SessionResourceNamespace::Files,
        conventions.root(),
        conventions.files_dir(),
        conventions.files_dir(),
    );
}

fn scan_namespace(
    manifest: &mut SessionResourceManifest,
    namespace: SessionResourceNamespace,
    root: &Path,
    base_dir: PathBuf,
    dir: PathBuf,
) {
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_namespace(manifest, namespace, root, base_dir.clone(), path);
            continue;
        }
        let Ok(relative) = path.strip_prefix(&base_dir) else {
            continue;
        };
        let Ok(logical_path) = super::LogicalPath::parse(relative) else {
            continue;
        };
        if let Some(resource) = ref_if_file(namespace, Some(logical_path), root, path) {
            upsert_ref(manifest, resource);
        }
    }
}

fn ref_from_snapshot(
    resource: &SessionResourceSnapshot,
    conventions: &PathConventions,
    available: bool,
) -> SessionResourceRef {
    let storage_path = storage_path(resource.namespace, resource.path.as_ref());
    let local_path = conventions.root().join(&storage_path);
    let file_metadata = std::fs::metadata(&local_path).ok();
    SessionResourceRef {
        namespace: resource.namespace,
        logical_path: resource.path.clone(),
        storage_path,
        len: file_metadata
            .as_ref()
            .map_or(resource.len, std::fs::Metadata::len),
        updated_at: file_metadata
            .and_then(|metadata| metadata.modified().ok())
            .map(chrono::DateTime::<Utc>::from)
            .or(Some(resource.updated_at)),
        available: available || local_path.is_file(),
    }
}

fn ref_if_file(
    namespace: SessionResourceNamespace,
    logical_path: Option<super::LogicalPath>,
    root: &Path,
    path: PathBuf,
) -> Option<SessionResourceRef> {
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(SessionResourceRef {
        namespace,
        logical_path,
        storage_path: path.strip_prefix(root).ok()?.to_path_buf(),
        len: metadata.len(),
        updated_at: metadata.modified().ok().map(chrono::DateTime::<Utc>::from),
        available: true,
    })
}

fn storage_path(
    namespace: SessionResourceNamespace,
    logical_path: Option<&super::LogicalPath>,
) -> PathBuf {
    match namespace {
        SessionResourceNamespace::Plan => PathBuf::from("plan.md"),
        SessionResourceNamespace::Workspace => PathBuf::from("workspace.yaml"),
        SessionResourceNamespace::Artifacts => PathBuf::from("artifacts")
            .join(logical_path.map_or_else(PathBuf::new, super::LogicalPath::to_path_buf)),
        SessionResourceNamespace::Temp => PathBuf::from("tmp")
            .join(logical_path.map_or_else(PathBuf::new, super::LogicalPath::to_path_buf)),
        SessionResourceNamespace::Checkpoints => PathBuf::from("checkpoints")
            .join(logical_path.map_or_else(PathBuf::new, super::LogicalPath::to_path_buf)),
        SessionResourceNamespace::Files => PathBuf::from("files")
            .join(logical_path.map_or_else(PathBuf::new, super::LogicalPath::to_path_buf)),
    }
}

fn upsert_ref(manifest: &mut SessionResourceManifest, resource: SessionResourceRef) {
    match resource.namespace {
        SessionResourceNamespace::Plan => manifest.plan = Some(resource),
        SessionResourceNamespace::Workspace => manifest.workspace = Some(resource),
        SessionResourceNamespace::Artifacts => upsert_vec(&mut manifest.artifacts, resource),
        SessionResourceNamespace::Temp => upsert_vec(&mut manifest.temp_files, resource),
        SessionResourceNamespace::Checkpoints => upsert_vec(&mut manifest.checkpoints, resource),
        SessionResourceNamespace::Files => upsert_vec(&mut manifest.files, resource),
    }
}

fn upsert_vec(resources: &mut Vec<SessionResourceRef>, resource: SessionResourceRef) {
    if let Some(existing) = resources
        .iter_mut()
        .find(|existing| existing.storage_path == resource.storage_path)
    {
        *existing = resource;
    } else {
        resources.push(resource);
    }
}

fn scrub_unavailable_runtime_resources(
    runtime: &mut RuntimeSnapshot,
    manifest: &SessionResourceManifest,
) {
    for thread in &mut runtime.threads {
        thread.resources.artifacts.retain(|resource| {
            is_available(
                manifest,
                SessionResourceNamespace::Artifacts,
                resource.path.as_ref(),
            )
        });
        thread.resources.temp_files.retain(|resource| {
            is_available(
                manifest,
                SessionResourceNamespace::Temp,
                resource.path.as_ref(),
            )
        });
        thread.resources.checkpoints.retain(|resource| {
            is_available(
                manifest,
                SessionResourceNamespace::Checkpoints,
                resource.path.as_ref(),
            )
        });
        thread.resources.files.retain(|resource| {
            is_available(
                manifest,
                SessionResourceNamespace::Files,
                resource.path.as_ref(),
            )
        });
        if thread
            .resources
            .plan
            .as_ref()
            .is_some_and(|_| !manifest.plan.as_ref().is_some_and(|entry| entry.available))
        {
            thread.resources.plan = None;
        }
        if thread.resources.workspace.as_ref().is_some_and(|_| {
            !manifest
                .workspace
                .as_ref()
                .is_some_and(|entry| entry.available)
        }) {
            thread.resources.workspace = None;
        }
    }
}

fn is_available(
    manifest: &SessionResourceManifest,
    namespace: SessionResourceNamespace,
    logical_path: Option<&super::LogicalPath>,
) -> bool {
    let storage_path = storage_path(namespace, logical_path);
    let candidates = match namespace {
        SessionResourceNamespace::Artifacts => &manifest.artifacts,
        SessionResourceNamespace::Temp => &manifest.temp_files,
        SessionResourceNamespace::Checkpoints => &manifest.checkpoints,
        SessionResourceNamespace::Files => &manifest.files,
        SessionResourceNamespace::Plan | SessionResourceNamespace::Workspace => return true,
    };
    candidates
        .iter()
        .any(|entry| entry.storage_path == storage_path && entry.available)
}

fn resource_events_from_manifest(
    thread_id: ThreadId,
    start_seq: u64,
    manifest: &SessionResourceManifest,
) -> Vec<AgentRuntimeEvent> {
    let mut seq = start_seq;
    manifest
        .plan
        .iter()
        .chain(manifest.workspace.iter())
        .chain(
            manifest
                .artifacts
                .iter()
                .chain(manifest.temp_files.iter())
                .chain(manifest.checkpoints.iter())
                .chain(manifest.files.iter()),
        )
        .map(|resource| {
            let snapshot = SessionResourceSnapshot {
                namespace: resource.namespace,
                path: resource.logical_path.clone(),
                len: resource.len,
                updated_at: resource.updated_at.unwrap_or_else(Utc::now),
                metadata: serde_json::Value::Null,
            };
            let payload = match resource.namespace {
                SessionResourceNamespace::Artifacts => {
                    AgentRuntimeEventPayload::ArtifactCreated { resource: snapshot }
                }
                SessionResourceNamespace::Temp => {
                    AgentRuntimeEventPayload::TempFileWritten { resource: snapshot }
                }
                SessionResourceNamespace::Checkpoints => {
                    AgentRuntimeEventPayload::CheckpointCreated { resource: snapshot }
                }
                SessionResourceNamespace::Files => {
                    AgentRuntimeEventPayload::SessionFileWritten { resource: snapshot }
                }
                SessionResourceNamespace::Plan => {
                    AgentRuntimeEventPayload::PlanWritten { resource: snapshot }
                }
                SessionResourceNamespace::Workspace => {
                    AgentRuntimeEventPayload::WorkspaceUpdated { resource: snapshot }
                }
            };
            let event = AgentRuntimeEvent {
                schema_version: AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
                seq,
                thread_id,
                turn_id: None,
                timestamp: Utc::now(),
                payload,
            };
            seq += 1;
            event
        })
        .collect()
}
