//! Host-facing attachment contract and V1 resolver.

mod preflight;
mod renderer;
mod resolver;
mod types;

pub use preflight::{
    preflight_resolved_attachments, AttachmentPreflightError, AttachmentPreflightReport,
};
pub use renderer::{render_prompt_input_text, render_resolved_text, AttachmentTextRenderer};
pub use resolver::{AttachmentResolveError, DefaultAttachmentResolver};
pub use types::{
    Attachment, AttachmentMetadata, AttachmentResolveOptions, AttachmentResolver, AttachmentSource,
    BlobAttachment, FileAttachment, PromptInput, ResolvedAttachment, SelectionAttachment,
};

#[cfg(test)]
mod tests;
