use std::{fs, path::Path};

use thiserror::Error;

use super::types::{
    Attachment, AttachmentMetadata, AttachmentResolveOptions, AttachmentResolver, AttachmentSource,
    BlobAttachment, FileAttachment, ResolvedAttachment, SelectionAttachment,
};

const FALLBACK_TEXT_MIME: &str = "text/plain; charset=utf-8";
pub(super) const UNSUPPORTED_MEDIA_MIME: &str = "application/octet-stream";
const MAX_ATTACHMENT_DISPLAY_NAME_CHARS: usize = 128;
const MAX_UNSUPPORTED_MEDIA_MIME_CHARS: usize = 128;

/// Default synchronous V1 attachment resolver.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultAttachmentResolver;

impl AttachmentResolver for DefaultAttachmentResolver {
    type Error = AttachmentResolveError;

    fn resolve_attachments(
        &self,
        attachments: &[Attachment],
        options: &AttachmentResolveOptions,
    ) -> Result<Vec<ResolvedAttachment>, Self::Error> {
        validate_count(attachments.len(), options.max_attachments)?;

        let mut resolved = Vec::with_capacity(attachments.len());
        let mut total_bytes = 0usize;

        for attachment in attachments {
            let item = match attachment {
                Attachment::File(file) => resolve_file(file, options)?,
                Attachment::Blob(blob) => resolve_blob(blob, options)?,
                Attachment::Selection(selection) => resolve_selection(selection, options)?,
            };

            total_bytes = total_bytes.checked_add(item.metadata().size_bytes).ok_or(
                AttachmentResolveError::TotalLimitExceeded {
                    size: usize::MAX,
                    max: options.max_total_bytes,
                },
            )?;
            validate_total_size(total_bytes, options.max_total_bytes)?;
            resolved.push(item);
        }

        Ok(resolved)
    }
}

/// Attachment resolution failure.
#[derive(Debug, Error)]
pub enum AttachmentResolveError {
    #[error("too many attachments: {count} exceeds limit {max}")]
    CountLimitExceeded { count: usize, max: usize },
    #[error("attachment '{name}' is too large: {size} bytes exceeds limit {max}")]
    AttachmentLimitExceeded {
        name: String,
        size: usize,
        max: usize,
    },
    #[error("attachments are too large: {size} bytes exceeds total limit {max}")]
    TotalLimitExceeded { size: usize, max: usize },
    #[error("attachment path '{path}' is not a file")]
    NotAFile { path: String },
    #[error("failed to read attachment metadata for '{path}': {source}")]
    Metadata {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read attachment '{path}': {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("text attachment '{name}' is not valid UTF-8")]
    InvalidUtf8 { name: String },
    #[error("unsupported attachment MIME type '{mime_type}' for '{name}'")]
    UnsupportedMime { name: String, mime_type: String },
    #[error("unsupported binary attachment '{name}' without text or image MIME type")]
    UnsupportedBinary { name: String },
}

fn validate_count(count: usize, max: usize) -> Result<(), AttachmentResolveError> {
    if count > max {
        return Err(AttachmentResolveError::CountLimitExceeded { count, max });
    }

    Ok(())
}

fn validate_attachment_size(
    name: &str,
    size: usize,
    max: usize,
) -> Result<(), AttachmentResolveError> {
    if size > max {
        return Err(AttachmentResolveError::AttachmentLimitExceeded {
            name: name.to_string(),
            size,
            max,
        });
    }

    Ok(())
}

fn validate_total_size(size: usize, max: usize) -> Result<(), AttachmentResolveError> {
    if size > max {
        return Err(AttachmentResolveError::TotalLimitExceeded { size, max });
    }

    Ok(())
}

fn resolve_file(
    file: &FileAttachment,
    options: &AttachmentResolveOptions,
) -> Result<ResolvedAttachment, AttachmentResolveError> {
    let path = file.path.as_path();
    let name = file_name(file);
    let metadata = fs::metadata(path).map_err(|source| AttachmentResolveError::Metadata {
        path: display_path(path),
        source,
    })?;

    if !metadata.is_file() {
        return Err(AttachmentResolveError::NotAFile {
            path: display_path(path),
        });
    }

    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    validate_attachment_size(&name, size, options.max_attachment_bytes)?;

    let data = fs::read(path).map_err(|source| AttachmentResolveError::Read {
        path: display_path(path),
        source,
    })?;
    let mime_type = normalize_mime(file.mime_type.as_deref())
        .or_else(|| mime_guess::from_path(path).first_raw().map(str::to_owned));
    let metadata = AttachmentMetadata {
        source: AttachmentSource::File {
            path: path.to_path_buf(),
        },
        name: Some(name.clone()),
        mime_type,
        size_bytes: data.len(),
    };

    resolve_data(data, metadata, &name)
}

fn resolve_blob(
    blob: &BlobAttachment,
    options: &AttachmentResolveOptions,
) -> Result<ResolvedAttachment, AttachmentResolveError> {
    let name = blob
        .name
        .as_deref()
        .map(sanitize_display_name)
        .unwrap_or_else(|| "blob".to_string());
    validate_attachment_size(&name, blob.data.len(), options.max_attachment_bytes)?;
    let metadata = AttachmentMetadata {
        source: AttachmentSource::Blob,
        name: blob.name.as_deref().map(sanitize_display_name),
        mime_type: normalize_mime(blob.mime_type.as_deref()),
        size_bytes: blob.data.len(),
    };

    resolve_data(blob.data.clone(), metadata, &name)
}

fn resolve_selection(
    selection: &SelectionAttachment,
    options: &AttachmentResolveOptions,
) -> Result<ResolvedAttachment, AttachmentResolveError> {
    let name = selection
        .name
        .as_deref()
        .map(sanitize_display_name)
        .unwrap_or_else(|| "selection".to_string());
    validate_attachment_size(&name, selection.text.len(), options.max_attachment_bytes)?;

    Ok(ResolvedAttachment::Text {
        text: selection.text.clone(),
        metadata: AttachmentMetadata {
            source: AttachmentSource::Selection,
            name: selection.name.as_deref().map(sanitize_display_name),
            mime_type: Some(FALLBACK_TEXT_MIME.to_string()),
            size_bytes: selection.text.len(),
        },
    })
}

fn resolve_data(
    data: Vec<u8>,
    mut metadata: AttachmentMetadata,
    name: &str,
) -> Result<ResolvedAttachment, AttachmentResolveError> {
    if let Some(mime_type) = metadata.mime_type.clone() {
        if is_image_mime(&mime_type) {
            return Ok(ResolvedAttachment::Image { data, metadata });
        }

        if is_text_mime(&mime_type) {
            let text =
                String::from_utf8(data).map_err(|_| AttachmentResolveError::InvalidUtf8 {
                    name: name.to_string(),
                })?;
            return Ok(ResolvedAttachment::Text { text, metadata });
        }

        if !allows_utf8_fallback(&mime_type) {
            return unsupported_media_text(metadata, name, Some(mime_type));
        }
    }

    match String::from_utf8(data) {
        Ok(text) => {
            metadata.mime_type = Some(FALLBACK_TEXT_MIME.to_string());
            Ok(ResolvedAttachment::Text { text, metadata })
        }
        Err(_) => {
            let mime_type = metadata.mime_type.clone();
            unsupported_media_text(metadata, name, mime_type)
        }
    }
}

pub(super) fn unsupported_media_marker(name: &str, mime_type: &str, size_bytes: usize) -> String {
    format!(
        "User attached unsupported media: {name} ({mime_type}, {size_bytes} bytes). Content omitted."
    )
}

fn unsupported_media_text(
    metadata: AttachmentMetadata,
    name: &str,
    mime_type: Option<String>,
) -> Result<ResolvedAttachment, AttachmentResolveError> {
    let safe_name = sanitize_display_name(name);
    let Some(mime_type) = safe_unsupported_media_mime(mime_type.as_deref()) else {
        return Err(AttachmentResolveError::UnsupportedMime {
            name: safe_name,
            mime_type: mime_type.unwrap_or_default(),
        });
    };

    Ok(ResolvedAttachment::Text {
        text: unsupported_media_marker(&safe_name, &mime_type, metadata.size_bytes),
        metadata: AttachmentMetadata {
            name: Some(safe_name),
            mime_type: Some(mime_type),
            ..metadata
        },
    })
}

fn normalize_mime(mime_type: Option<&str>) -> Option<String> {
    mime_type
        .map(str::trim)
        .filter(|mime_type| !mime_type.is_empty())
        .map(str::to_owned)
}

fn is_image_mime(mime_type: &str) -> bool {
    mime_type
        .split_once(';')
        .map_or(mime_type, |(base, _)| base)
        .trim()
        .starts_with("image/")
}

fn is_text_mime(mime_type: &str) -> bool {
    let base = mime_type
        .split_once(';')
        .map_or(mime_type, |(base, _)| base)
        .trim();

    base.starts_with("text/")
        || matches!(
            base,
            "application/json"
                | "application/javascript"
                | "application/typescript"
                | "application/xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/x-toml"
                | "application/x-sh"
                | "application/x-shellscript"
                | "image/svg+xml"
        )
        || base.ends_with("+json")
        || base.ends_with("+xml")
}

fn allows_utf8_fallback(mime_type: &str) -> bool {
    let base = mime_type
        .split_once(';')
        .map_or(mime_type, |(base, _)| base)
        .trim();

    base == "application/octet-stream"
}

fn file_name(file: &FileAttachment) -> String {
    file.name
        .as_deref()
        .map(sanitize_display_name)
        .unwrap_or_else(|| {
            file.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file")
                .to_string()
        })
}

pub(super) fn safe_attachment_name(metadata: &AttachmentMetadata) -> String {
    metadata
        .name
        .as_deref()
        .map(sanitize_display_name)
        .unwrap_or_else(|| match &metadata.source {
            AttachmentSource::File { path } => path
                .file_name()
                .and_then(|name| name.to_str())
                .map(sanitize_display_name)
                .unwrap_or_else(|| "file".to_string()),
            AttachmentSource::Blob => "blob".to_string(),
            AttachmentSource::Selection => "selection".to_string(),
        })
}

pub(super) fn safe_unsupported_media_mime(mime_type: Option<&str>) -> Option<String> {
    let mime_type = mime_type.unwrap_or(UNSUPPORTED_MEDIA_MIME);
    let base = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase();
    if base.is_empty()
        || base.len() > MAX_UNSUPPORTED_MEDIA_MIME_CHARS
        || base.chars().any(|c| c.is_control() || c.is_whitespace())
        || base.split('/').count() != 2
    {
        return None;
    }

    let (kind, subtype) = base.split_once('/')?;
    let valid_token = |token: &str| {
        !token.is_empty()
            && token.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '!' | '#' | '$' | '&' | '^' | '_' | '.' | '+' | '-')
            })
    };
    valid_token(kind).then_some(())?;
    valid_token(subtype).then_some(())?;

    Some(base)
}

fn sanitize_display_name(name: &str) -> String {
    let basename = name
        .trim()
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("attachment");
    let sanitized = basename
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_ATTACHMENT_DISPLAY_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    if sanitized.is_empty() {
        "attachment".to_string()
    } else {
        sanitized
    }
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
