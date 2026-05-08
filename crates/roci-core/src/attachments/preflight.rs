use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::attachments::{AttachmentMetadata, AttachmentSource, ResolvedAttachment};
use crate::context::{estimate_text_tokens, TokenCount};
use crate::models::ModelCapabilities;

/// Byte and token accounting for resolved attachments accepted by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPreflightReport {
    pub total_attachments: usize,
    pub text_attachments: usize,
    pub image_attachments: usize,
    pub total_bytes: usize,
    pub text_bytes: usize,
    pub image_bytes: usize,
    pub estimated_tokens: TokenCount,
    pub text_tokens: TokenCount,
    pub image_tokens: TokenCount,
}

impl Default for AttachmentPreflightReport {
    fn default() -> Self {
        Self {
            total_attachments: 0,
            text_attachments: 0,
            image_attachments: 0,
            total_bytes: 0,
            text_bytes: 0,
            image_bytes: 0,
            estimated_tokens: TokenCount::zero(),
            text_tokens: TokenCount::zero(),
            image_tokens: TokenCount::zero(),
        }
    }
}

/// Attachment capability failure detected before provider calls.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentPreflightError {
    #[error("model does not support image attachments")]
    ImageUnsupported,
    #[error("too many image attachments: {count} exceeds limit {max}")]
    ImageCountExceeded { count: usize, max: usize },
    #[error("image attachment '{name}' is too large: {size} bytes exceeds limit {max}")]
    ImageBytesExceeded {
        name: String,
        size: usize,
        max: usize,
    },
    #[error("image attachments are too large: {size} bytes exceeds total limit {max}")]
    ImageTotalBytesExceeded { size: usize, max: usize },
    #[error("unsupported image MIME type '{mime_type}' for '{name}'")]
    ImageMimeUnsupported { name: String, mime_type: String },
    #[error("text attachment '{name}' is too large: {size} bytes exceeds limit {max}")]
    TextBytesExceeded {
        name: String,
        size: usize,
        max: usize,
    },
    #[error("text attachment tokens {tokens} exceed limit {max}")]
    TextTokensExceeded { tokens: usize, max: usize },
}

/// Validates resolved attachments against model input capabilities.
///
/// # Errors
///
/// Returns [`AttachmentPreflightError`] when a resolved attachment exceeds
/// text/image limits or requires unsupported image input.
pub fn preflight_resolved_attachments(
    attachments: &[ResolvedAttachment],
    capabilities: &ModelCapabilities,
) -> Result<AttachmentPreflightReport, AttachmentPreflightError> {
    let mut report = AttachmentPreflightReport::default();

    for attachment in attachments {
        match attachment {
            ResolvedAttachment::Text { text, metadata } => {
                preflight_text_attachment(text, metadata, capabilities, &mut report)?;
            }
            ResolvedAttachment::Image { data, metadata } => {
                preflight_image_attachment(data, metadata, capabilities, &mut report)?;
            }
        }
    }

    report.estimated_tokens = report.text_tokens + report.image_tokens;
    Ok(report)
}

fn preflight_text_attachment(
    text: &str,
    metadata: &AttachmentMetadata,
    capabilities: &ModelCapabilities,
    report: &mut AttachmentPreflightReport,
) -> Result<(), AttachmentPreflightError> {
    let size = text.len();
    if let Some(max) = capabilities.input.text.max_text_bytes {
        if size > max {
            return Err(AttachmentPreflightError::TextBytesExceeded {
                name: attachment_name(metadata),
                size,
                max,
            });
        }
    }

    let tokens = estimate_text_tokens(text);
    let text_tokens = report.text_tokens + TokenCount::heuristic(tokens);
    if let Some(max) = capabilities.input.text.max_text_tokens {
        if text_tokens.tokens > max {
            return Err(AttachmentPreflightError::TextTokensExceeded {
                tokens: text_tokens.tokens,
                max,
            });
        }
    }

    report.total_attachments += 1;
    report.text_attachments += 1;
    report.total_bytes += size;
    report.text_bytes += size;
    report.text_tokens = text_tokens;

    Ok(())
}

fn preflight_image_attachment(
    data: &[u8],
    metadata: &AttachmentMetadata,
    capabilities: &ModelCapabilities,
    report: &mut AttachmentPreflightReport,
) -> Result<(), AttachmentPreflightError> {
    let image_caps = capabilities
        .input
        .image
        .as_ref()
        .ok_or(AttachmentPreflightError::ImageUnsupported)?;

    let image_count = report.image_attachments + 1;
    if image_count > image_caps.max_images {
        return Err(AttachmentPreflightError::ImageCountExceeded {
            count: image_count,
            max: image_caps.max_images,
        });
    }

    let name = attachment_name(metadata);
    let size = data.len();
    if let Some(max) = image_caps.max_image_bytes {
        if size > max {
            return Err(AttachmentPreflightError::ImageBytesExceeded { name, size, max });
        }
    }

    let image_total_bytes = report.image_bytes + size;
    if let Some(max) = image_caps.max_total_image_bytes {
        if image_total_bytes > max {
            return Err(AttachmentPreflightError::ImageTotalBytesExceeded {
                size: image_total_bytes,
                max,
            });
        }
    }

    let mime_type = normalized_mime_type(metadata.mime_type.as_deref());
    let mime_allowed = image_caps
        .supported_mime_types
        .iter()
        .map(|mime_type| normalized_mime_type(Some(mime_type.as_str())))
        .any(|supported_mime_type| supported_mime_type == mime_type);

    if !mime_allowed {
        return Err(AttachmentPreflightError::ImageMimeUnsupported { name, mime_type });
    }

    report.total_attachments += 1;
    report.image_attachments = image_count;
    report.total_bytes += size;
    report.image_bytes = image_total_bytes;
    report.image_tokens += TokenCount::heuristic(image_caps.image_token_estimate);

    Ok(())
}

fn attachment_name(metadata: &AttachmentMetadata) -> String {
    metadata
        .name
        .clone()
        .unwrap_or_else(|| match &metadata.source {
            AttachmentSource::File { path } => path.display().to_string(),
            AttachmentSource::Blob => "blob".to_string(),
            AttachmentSource::Selection => "selection".to_string(),
        })
}

fn normalized_mime_type(mime_type: Option<&str>) -> String {
    mime_type
        .and_then(|mime_type| mime_type.split(';').next())
        .map(str::trim)
        .filter(|mime_type| !mime_type.is_empty())
        .unwrap_or("application/octet-stream")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::models::{ImageInputCapabilities, ModelInputCapabilities, TextInputCapabilities};

    fn text_attachment(text: impl Into<String>) -> ResolvedAttachment {
        ResolvedAttachment::Text {
            text: text.into(),
            metadata: AttachmentMetadata {
                source: AttachmentSource::Selection,
                name: Some("selection".to_string()),
                mime_type: Some("text/plain".to_string()),
                size_bytes: 0,
            },
        }
    }

    fn image_attachment(data: Vec<u8>, mime_type: impl Into<String>) -> ResolvedAttachment {
        named_image_attachment(data, mime_type, Some("image.bin"))
    }

    fn named_image_attachment(
        data: Vec<u8>,
        mime_type: impl Into<String>,
        name: Option<&str>,
    ) -> ResolvedAttachment {
        ResolvedAttachment::Image {
            data,
            metadata: AttachmentMetadata {
                source: AttachmentSource::File {
                    path: PathBuf::from("image.bin"),
                },
                name: name.map(str::to_string),
                mime_type: Some(mime_type.into()),
                size_bytes: 0,
            },
        }
    }

    fn text_caps(
        max_text_bytes: Option<usize>,
        max_text_tokens: Option<usize>,
    ) -> ModelCapabilities {
        ModelCapabilities {
            input: ModelInputCapabilities {
                text: TextInputCapabilities {
                    max_text_bytes,
                    max_text_tokens,
                },
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        }
    }

    fn vision_caps(image_caps: ImageInputCapabilities) -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities {
                image: Some(image_caps),
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        }
    }

    fn default_image_caps() -> ImageInputCapabilities {
        ImageInputCapabilities {
            max_images: 20,
            max_image_bytes: Some(20 * 1024 * 1024),
            max_total_image_bytes: Some(50 * 1024 * 1024),
            supported_mime_types: vec!["image/png".to_string()],
            image_token_estimate: 1200,
        }
    }

    #[test]
    fn text_attachment_reports_bytes_and_tokens() {
        let attachments = vec![text_attachment("abcdefgh")];

        let report = preflight_resolved_attachments(&attachments, &ModelCapabilities::default())
            .expect("text attachment should pass");

        assert_eq!(report.total_attachments, 1);
        assert_eq!(report.text_attachments, 1);
        assert_eq!(report.image_attachments, 0);
        assert_eq!(report.total_bytes, 8);
        assert_eq!(report.text_bytes, 8);
        assert_eq!(report.image_bytes, 0);
        assert_eq!(report.text_tokens, TokenCount::heuristic(2));
        assert_eq!(report.image_tokens, TokenCount::zero());
        assert_eq!(report.estimated_tokens, TokenCount::heuristic(2));
    }

    #[test]
    fn non_vision_model_rejects_image() {
        let attachments = vec![image_attachment(vec![1, 2, 3, 4], "image/png")];

        let err = preflight_resolved_attachments(&attachments, &ModelCapabilities::default())
            .expect_err("image should fail");

        assert_eq!(err, AttachmentPreflightError::ImageUnsupported);
    }

    #[test]
    fn vision_model_accepts_allowed_image_and_counts_tokens() {
        let caps = vision_caps(default_image_caps());
        let attachments = vec![image_attachment(vec![1, 2, 3, 4], "image/png")];

        let report =
            preflight_resolved_attachments(&attachments, &caps).expect("image should pass");

        assert_eq!(report.total_attachments, 1);
        assert_eq!(report.text_attachments, 0);
        assert_eq!(report.image_attachments, 1);
        assert_eq!(report.total_bytes, 4);
        assert_eq!(report.image_bytes, 4);
        assert_eq!(report.text_bytes, 0);
        assert_eq!(report.image_tokens, TokenCount::heuristic(1200));
        assert_eq!(report.text_tokens, TokenCount::zero());
        assert_eq!(report.estimated_tokens, TokenCount::heuristic(1200));
    }

    #[test]
    fn image_mime_allowlist_is_case_insensitive_and_ignores_params() {
        let caps = vision_caps(default_image_caps());
        let attachments = vec![image_attachment(
            vec![1, 2, 3, 4],
            "IMAGE/PNG; charset=binary",
        )];

        let report =
            preflight_resolved_attachments(&attachments, &caps).expect("image should pass");

        assert_eq!(report.image_attachments, 1);
        assert_eq!(report.image_tokens, TokenCount::heuristic(1200));
    }

    #[test]
    fn image_mime_allowlist_is_enforced() {
        let caps = vision_caps(default_image_caps());
        let attachments = vec![named_image_attachment(vec![1, 2, 3, 4], "image/bmp", None)];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("unsupported MIME should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageMimeUnsupported {
                name: "image.bin".to_string(),
                mime_type: "image/bmp".to_string(),
            }
        );
    }

    #[test]
    fn image_without_mime_type_defaults_to_octet_stream_and_is_rejected() {
        let caps = vision_caps(default_image_caps());
        let attachments = vec![ResolvedAttachment::Image {
            data: vec![1, 2, 3, 4],
            metadata: AttachmentMetadata {
                source: AttachmentSource::File {
                    path: PathBuf::from("image.bin"),
                },
                name: Some("image.bin".to_string()),
                mime_type: None,
                size_bytes: 0,
            },
        }];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("missing MIME should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageMimeUnsupported {
                name: "image.bin".to_string(),
                mime_type: "application/octet-stream".to_string(),
            }
        );
    }

    #[test]
    fn image_count_limit_is_enforced() {
        let caps = vision_caps(ImageInputCapabilities {
            max_images: 1,
            ..default_image_caps()
        });
        let attachments = vec![
            image_attachment(vec![1, 2, 3, 4], "image/png"),
            image_attachment(vec![1, 2, 3, 4], "image/png"),
        ];

        let err =
            preflight_resolved_attachments(&attachments, &caps).expect_err("count should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageCountExceeded { count: 2, max: 1 }
        );
    }

    #[test]
    fn image_byte_limits_are_enforced() {
        let caps = vision_caps(ImageInputCapabilities {
            max_image_bytes: Some(3),
            ..default_image_caps()
        });
        let attachments = vec![image_attachment(vec![1, 2, 3, 4], "image/png")];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("image bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageBytesExceeded {
                name: "image.bin".to_string(),
                size: 4,
                max: 3,
            }
        );
    }

    #[test]
    fn image_byte_limit_uses_actual_payload_length() {
        let caps = vision_caps(ImageInputCapabilities {
            max_image_bytes: Some(3),
            ..default_image_caps()
        });
        let attachments = vec![ResolvedAttachment::Image {
            data: vec![1, 2, 3, 4],
            metadata: AttachmentMetadata {
                source: AttachmentSource::Blob,
                name: Some("pixel.png".to_string()),
                mime_type: Some("image/png".to_string()),
                size_bytes: 1,
            },
        }];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("actual image bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageBytesExceeded {
                name: "pixel.png".to_string(),
                size: 4,
                max: 3,
            }
        );
    }

    #[test]
    fn image_total_byte_limit_is_enforced() {
        let caps = vision_caps(ImageInputCapabilities {
            max_total_image_bytes: Some(7),
            ..default_image_caps()
        });
        let attachments = vec![
            image_attachment(vec![1, 2, 3, 4], "image/png"),
            image_attachment(vec![1, 2, 3, 4], "image/png"),
        ];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("total image bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageTotalBytesExceeded { size: 8, max: 7 }
        );
    }

    #[test]
    fn text_byte_limit_uses_actual_text_length() {
        let caps = text_caps(Some(3), None);
        let attachments = vec![ResolvedAttachment::Text {
            text: "abcd".to_string(),
            metadata: AttachmentMetadata {
                source: AttachmentSource::Selection,
                name: Some("selection".to_string()),
                mime_type: Some("text/plain".to_string()),
                size_bytes: 1,
            },
        }];

        let err = preflight_resolved_attachments(&attachments, &caps)
            .expect_err("actual text bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::TextBytesExceeded {
                name: "selection".to_string(),
                size: 4,
                max: 3,
            }
        );
    }

    #[test]
    fn text_token_limit_is_enforced() {
        let caps = text_caps(None, Some(1));
        let attachments = vec![text_attachment("abcdefgh")];

        let err =
            preflight_resolved_attachments(&attachments, &caps).expect_err("tokens should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::TextTokensExceeded { tokens: 2, max: 1 }
        );
    }

    #[test]
    fn preflight_report_round_trips_through_json() {
        let report = AttachmentPreflightReport {
            total_attachments: 2,
            text_attachments: 1,
            image_attachments: 1,
            total_bytes: 12,
            text_bytes: 8,
            image_bytes: 4,
            estimated_tokens: TokenCount::heuristic(1202),
            text_tokens: TokenCount::heuristic(2),
            image_tokens: TokenCount::heuristic(1200),
        };

        let json = serde_json::to_string(&report).expect("serialize report");
        let decoded: AttachmentPreflightReport =
            serde_json::from_str(&json).expect("deserialize report");

        assert_eq!(decoded, report);
    }
}
