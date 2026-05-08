use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Host-provided attachment before resolution and preflight checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Attachment {
    /// File attachment loaded from local filesystem.
    File(FileAttachment),
    /// In-memory blob attachment supplied by host code.
    Blob(BlobAttachment),
    /// User-visible text selection supplied by host code.
    Selection(SelectionAttachment),
}

impl Attachment {
    /// Creates a file attachment.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File(FileAttachment::new(path))
    }

    /// Creates a blob attachment.
    pub fn blob(data: impl Into<Vec<u8>>) -> Self {
        Self::Blob(BlobAttachment::new(data))
    }

    /// Creates a selection attachment.
    pub fn selection(text: impl Into<String>) -> Self {
        Self::Selection(SelectionAttachment::new(text))
    }
}

/// File attachment supplied by host code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub path: PathBuf,
    pub name: Option<String>,
    pub mime_type: Option<String>,
}

impl FileAttachment {
    /// Creates a file attachment for `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            name: None,
            mime_type: None,
        }
    }

    /// Overrides display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Overrides MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

/// In-memory blob attachment supplied by host code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobAttachment {
    pub data: Vec<u8>,
    pub name: Option<String>,
    pub mime_type: Option<String>,
}

impl BlobAttachment {
    /// Creates a blob attachment.
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self {
            data: data.into(),
            name: None,
            mime_type: None,
        }
    }

    /// Overrides display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets MIME type.
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }
}

/// Text selection supplied by host code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionAttachment {
    pub text: String,
    pub name: Option<String>,
}

impl SelectionAttachment {
    /// Creates a text selection attachment.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            name: None,
        }
    }

    /// Overrides display name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Resolution metadata preserved for diagnostics, rendering, and providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub source: AttachmentSource,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: usize,
}

/// Origin of a resolved attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentSource {
    File { path: PathBuf },
    Blob,
    Selection,
}

/// Attachment after preflight and content resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolvedAttachment {
    /// UTF-8 text visible to model input.
    Text {
        text: String,
        metadata: AttachmentMetadata,
    },
    /// Image bytes preserved for provider image conversion.
    Image {
        data: Vec<u8>,
        metadata: AttachmentMetadata,
    },
}

impl ResolvedAttachment {
    /// Returns metadata for this resolved attachment.
    pub fn metadata(&self) -> &AttachmentMetadata {
        match self {
            Self::Text { metadata, .. } | Self::Image { metadata, .. } => metadata,
        }
    }

    /// Returns text content when this is a text attachment.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text.as_str()),
            Self::Image { .. } => None,
        }
    }

    /// Returns image bytes when this is an image attachment.
    pub fn as_image_data(&self) -> Option<&[u8]> {
        match self {
            Self::Image { data, .. } => Some(data.as_slice()),
            Self::Text { .. } => None,
        }
    }
}

/// Attachment preflight limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentResolveOptions {
    pub max_attachments: usize,
    pub max_attachment_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for AttachmentResolveOptions {
    fn default() -> Self {
        Self {
            max_attachments: 20,
            max_attachment_bytes: 20 * 1024 * 1024,
            max_total_bytes: 50 * 1024 * 1024,
        }
    }
}

/// Prompt text plus host attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptInput {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

impl PromptInput {
    /// Creates prompt input with no attachments.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    /// Adds one attachment.
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// Adds many attachments.
    pub fn with_attachments(mut self, attachments: impl IntoIterator<Item = Attachment>) -> Self {
        self.attachments.extend(attachments);
        self
    }
}

/// Resolves host attachments into model-safe V1 attachment data.
pub trait AttachmentResolver {
    type Error;

    /// Resolves attachments carried by prompt input.
    fn resolve_prompt_input(
        &self,
        input: &PromptInput,
        options: &AttachmentResolveOptions,
    ) -> Result<Vec<ResolvedAttachment>, Self::Error> {
        self.resolve_attachments(&input.attachments, options)
    }

    /// Resolves attachments with MIME, size, and count preflight checks.
    fn resolve_attachments(
        &self,
        attachments: &[Attachment],
        options: &AttachmentResolveOptions,
    ) -> Result<Vec<ResolvedAttachment>, Self::Error>;
}
