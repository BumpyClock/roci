//! Convenience re-exports for common use.

pub use crate::attachments::{
    render_prompt_input_text, render_resolved_text, Attachment, AttachmentMetadata,
    AttachmentResolveOptions, AttachmentResolver, AttachmentSource, AttachmentTextRenderer,
    BlobAttachment, DefaultAttachmentResolver, FileAttachment, PromptInput, ResolvedAttachment,
    SelectionAttachment,
};
pub use crate::config::RociConfig;
pub use crate::error::{Result, RociError};
pub use crate::models::LanguageModel;
pub use crate::provider::{ModelProvider, ProviderFactory, ProviderRegistry};
pub use crate::resource::{
    BranchSummarySettings, CompactionSettings, ContextFileResource, ContextPromptLoader,
    ContextPromptResources, LoadedPromptTemplates, PromptDiagnostic, PromptDiagnosticLevel,
    PromptTemplate, PromptTemplateLoader, ResourceBundle, ResourceDiagnostic, ResourceDirectories,
    ResourceLoader, ResourceSettings, ResourceSettingsLoader,
};
#[cfg(feature = "agent")]
pub use crate::session::LocalSessionStore;
pub use crate::session::{
    AgentRuntimeEvent, CreateSessionOptions, ImportPolicy, LocalProviderLedger, LocalSessionFs,
    LocalSessionResources, LogicalPath, PathConventions, PathNamespace, ProviderLedgerRecord,
    ProviderLedgerSnapshot, ProviderLedgerState, ProviderLedgerSummary, RuntimeCursor,
    RuntimeSnapshot, RuntimeSnapshotCache, SessionConfig, SessionDirEntry, SessionError,
    SessionFileKind, SessionFileMetadata, SessionFs, SessionId, SessionLease, SessionMetadata,
    SessionResourceManifest, SessionResourceMetadata, SessionResourceNamespace, SessionResourceRef,
    SessionResult, SessionResumeState, SessionSnapshot, ThreadId,
};
pub use crate::tools::{AgentTool, AgentToolParameters, Tool, ToolArguments};
pub use crate::types::{
    ContentPart, FinishReason, GenerateTextResult, GenerationSettings, ModelMessage, Role,
    StreamEventType, StreamTextResult, TextStreamDelta, Usage,
};
