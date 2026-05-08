use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use crate::attachments::resolver::{
    safe_attachment_name, safe_unsupported_media_mime, unsupported_media_marker,
    UNSUPPORTED_MEDIA_MIME,
};
use crate::attachments::{
    preflight_resolved_attachments, render_prompt_input_text, AttachmentMetadata,
    AttachmentPreflightError, AttachmentResolveOptions, AttachmentResolver, AttachmentSource,
    DefaultAttachmentResolver, PromptInput, ResolvedAttachment,
};
use crate::error::RociError;
use crate::models::ModelCapabilities;
use crate::types::{ContentPart, ImageContent, ModelMessage, ModelMessageMetadata, Role};

/// Provider-ready prompt plus sanitized attachment display metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledPromptInput {
    pub message: ModelMessage,
    pub attachments: Vec<AttachmentDisplayMetadata>,
}

/// Sanitized attachment origin safe for persisted chat metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentSourceKind {
    File,
    Blob,
    Selection,
}

/// Sanitized attachment content type safe for persisted chat metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentContentKind {
    Text,
    Image,
}

/// Attachment metadata safe to persist in chat snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentDisplayMetadata {
    pub source_kind: AttachmentSourceKind,
    pub content_kind: AttachmentContentKind,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: usize,
}

/// Compiles host prompt input into a provider-facing user message.
///
/// # Errors
///
/// Returns [`RociError::InvalidState`] when attachment resolution or capability
/// preflight rejects the input.
pub fn compile_prompt_input(
    input: &PromptInput,
    capabilities: &ModelCapabilities,
) -> Result<CompiledPromptInput, RociError> {
    let resolver = DefaultAttachmentResolver;
    let resolved = resolver
        .resolve_prompt_input(input, &AttachmentResolveOptions::default())
        .map_err(|err| RociError::InvalidState(err.to_string()))?;
    let resolved = match preflight_resolved_attachments(&resolved, capabilities) {
        Ok(_) => resolved,
        Err(
            AttachmentPreflightError::ImageUnsupported
            | AttachmentPreflightError::ImageMimeUnsupported { .. },
        ) => {
            let downgraded = downgrade_unsupported_images(resolved, capabilities);
            preflight_resolved_attachments(&downgraded, capabilities)
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
            downgraded
        }
        Err(err) => return Err(RociError::InvalidState(err.to_string())),
    };

    let attachments = resolved.iter().map(display_metadata).collect::<Vec<_>>();
    let mut content = vec![ContentPart::Text {
        text: render_prompt_input_text(input, &resolved),
    }];

    for attachment in &resolved {
        let ResolvedAttachment::Image { data, metadata } = attachment else {
            continue;
        };
        let mime_type = metadata
            .mime_type
            .as_deref()
            .map(normalize_mime_type)
            .unwrap_or_else(|| "application/octet-stream".to_string());
        content.push(ContentPart::Image(ImageContent {
            data: BASE64_STANDARD.encode(data),
            mime_type,
        }));
    }

    let metadata = (!attachments.is_empty()).then(|| ModelMessageMetadata {
        attachments: attachments.clone(),
    });

    Ok(CompiledPromptInput {
        message: ModelMessage {
            role: Role::User,
            content,
            name: None,
            timestamp: Some(chrono::Utc::now()),
            metadata,
        },
        attachments,
    })
}

fn display_metadata(attachment: &ResolvedAttachment) -> AttachmentDisplayMetadata {
    let (metadata, content_kind) = match attachment {
        ResolvedAttachment::Text { metadata, .. } => (metadata, AttachmentContentKind::Text),
        ResolvedAttachment::Image { metadata, .. } => (metadata, AttachmentContentKind::Image),
    };

    AttachmentDisplayMetadata {
        source_kind: match &metadata.source {
            AttachmentSource::File { .. } => AttachmentSourceKind::File,
            AttachmentSource::Blob => AttachmentSourceKind::Blob,
            AttachmentSource::Selection => AttachmentSourceKind::Selection,
        },
        content_kind,
        name: metadata.name.clone(),
        mime_type: metadata.mime_type.as_deref().map(normalize_mime_type),
        size_bytes: metadata.size_bytes,
    }
}

fn downgrade_unsupported_images(
    resolved: Vec<ResolvedAttachment>,
    capabilities: &ModelCapabilities,
) -> Vec<ResolvedAttachment> {
    resolved
        .into_iter()
        .map(|attachment| match attachment {
            ResolvedAttachment::Image { data, metadata }
                if image_content_unsupported(&metadata, capabilities) =>
            {
                unsupported_image_marker(data.len(), metadata)
            }
            attachment => attachment,
        })
        .collect()
}

fn image_content_unsupported(
    metadata: &AttachmentMetadata,
    capabilities: &ModelCapabilities,
) -> bool {
    let Some(image_caps) = capabilities.input.image.as_ref() else {
        return true;
    };
    let mime_type = metadata
        .mime_type
        .as_deref()
        .map(normalize_mime_type)
        .unwrap_or_else(|| UNSUPPORTED_MEDIA_MIME.to_string());

    !image_caps
        .supported_mime_types
        .iter()
        .any(|supported| normalize_mime_type(supported) == mime_type)
}

fn unsupported_image_marker(
    size_bytes: usize,
    mut metadata: AttachmentMetadata,
) -> ResolvedAttachment {
    let name = safe_attachment_name(&metadata);
    let mime_type = safe_unsupported_media_mime(metadata.mime_type.as_deref())
        .unwrap_or_else(|| UNSUPPORTED_MEDIA_MIME.to_string());
    metadata.name = Some(name.clone());
    metadata.mime_type = Some(mime_type.clone());
    metadata.size_bytes = size_bytes;

    ResolvedAttachment::Text {
        text: unsupported_media_marker(&name, &mime_type, size_bytes),
        metadata,
    }
}

fn normalize_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
}
