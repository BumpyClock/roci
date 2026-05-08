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
pub use crate::session::{
    LocalSessionFs, LocalSessionResources, LogicalPath, PathConventions, PathNamespace,
    SessionConfig, SessionDirEntry, SessionError, SessionFileKind, SessionFileMetadata, SessionFs,
    SessionId, SessionMetadata, SessionResourceMetadata, SessionResourceNamespace, SessionResult,
};
pub use crate::tools::{AgentTool, AgentToolParameters, Tool, ToolArguments};
pub use crate::types::{
    ContentPart, FinishReason, GenerateTextResult, GenerationSettings, ModelMessage, Role,
    StreamEventType, StreamTextResult, TextStreamDelta, Usage,
};
