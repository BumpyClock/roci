# Attachment Capabilities and Preflight Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add structured model input capabilities, attachment token accounting, shared resolved-attachment preflight, and CLI chat wiring for `tsq-r0c1att8.2`.

**Architecture:** Keep host attachments separate from provider payloads. `ModelCapabilities` owns model-facing input limits, while `attachments::preflight` validates already-resolved text/image attachments and returns a token/byte report. Provider model constructors populate the new input capability field so later runtime, provider, and model catalog tasks share one contract.

`roci-agent chat --attach` resolves files through the same core attachment resolver, preflights against the selected provider capabilities, renders text attachments into the prompt, and forwards image attachments as multipart `ContentPart::Image` only when capabilities allow images.

**Tech Stack:** Rust 2021, `serde`, `thiserror`, existing `roci-core::context::TokenCount`, existing `roci-core::attachments`.

---

## File Structure

- Modify: `crates/roci-core/src/context/tokens.rs`
  - Add `Serialize`/`Deserialize` derives to token count metadata types used in the preflight report.
- Modify: `crates/roci-core/src/models/capabilities.rs`
  - Add `ModelInputCapabilities`, `TextInputCapabilities`, `ImageInputCapabilities`, `FileInputCapabilities`.
  - Add default/helper constructors and unit tests.
  - Add `input: ModelInputCapabilities` to `ModelCapabilities`.
- Modify: `crates/roci-core/src/models/mod.rs`
  - Re-export new capability types.
- Create: `crates/roci-core/src/attachments/preflight.rs`
  - Add `preflight_resolved_attachments`, `AttachmentPreflightReport`, `AttachmentPreflightError`.
  - Add focused unit tests for text/image success and failures.
- Modify: `crates/roci-core/src/attachments/mod.rs`
  - Register and re-export preflight API/types.
- Modify: `crates/roci-core/src/prelude.rs`
  - Re-export preflight API/types for SDK users.
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs`
  - Add `prompt_message(ModelMessage)` so CLI and SDK hosts can submit multipart user messages after preflight.
- Modify: `crates/roci-core/src/agent/runtime_tests/chat_runtime.rs`
  - Add provider-request capture coverage proving multipart image content survives runtime submission.
- Modify: `crates/roci-cli/src/cli/mod.rs`
  - Add repeatable `chat --attach PATH`.
- Modify: `crates/roci-cli/src/chat.rs`
  - Resolve and preflight attachments before `AgentRuntime` execution.
  - Render text attachments into prompt text.
  - Encode image attachments as base64 `ContentPart::Image` when model capabilities support images.
- Modify: `crates/roci-cli/Cargo.toml`
  - Add `base64` for image payload encoding.
- Modify provider model files:
  - `crates/roci-providers/src/models/openai.rs`
  - `crates/roci-providers/src/models/anthropic.rs`
  - `crates/roci-providers/src/models/google.rs`
  - `crates/roci-providers/src/models/mistral.rs`
  - `crates/roci-providers/src/models/grok.rs`
  - `crates/roci-providers/src/models/groq.rs`
  - `crates/roci-providers/src/models/ollama.rs`
  - `crates/roci-providers/src/models/lmstudio.rs`
- Modify: `examples/custom_provider.rs`
  - Convert static `ModelCapabilities` literal to `OnceLock<ModelCapabilities>` if needed.
- Modify test/support files that construct `ModelCapabilities` literals:
  - Use `ModelInputCapabilities::default()` for text-only test caps or `ModelInputCapabilities::from_vision_support(true)` for vision tests.

---

### Task 1: Make Token Count Types Serializable

**Files:**
- Modify: `crates/roci-core/src/context/tokens.rs`

- [ ] **Step 1: Write the failing serde test**

Add this test inside the existing `#[cfg(test)] mod tests` in `crates/roci-core/src/context/tokens.rs`:

```rust
#[test]
fn token_count_round_trips_through_json() {
    let count = TokenCount::heuristic(42);

    let json = serde_json::to_string(&count).expect("serialize token count");
    let decoded: TokenCount = serde_json::from_str(&json).expect("deserialize token count");

    assert_eq!(decoded, count);
    assert_eq!(decoded.accuracy, CountAccuracy::Estimated);
    assert_eq!(decoded.source, TokenCountSource::Heuristic);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p roci-core token_count_round_trips_through_json
```

Expected: compile failure saying `TokenCount` does not implement `Serialize` or `Deserialize`.

- [ ] **Step 3: Add serde derives**

Change the serde import and derives in `crates/roci-core/src/context/tokens.rs`:

```rust
use serde::{Deserialize, Serialize};
use crate::types::{ContentPart, ModelMessage, Role};
```

Update derives:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CountAccuracy {
    Exact,
    Estimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenCountSource {
    Heuristic,
    ExactTokenizer,
    ProviderUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount {
    pub tokens: usize,
    pub accuracy: CountAccuracy,
    pub source: TokenCountSource,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p roci-core token_count_round_trips_through_json
```

Expected: PASS.

---

### Task 2: Add Structured Model Input Capabilities

**Files:**
- Modify: `crates/roci-core/src/models/capabilities.rs`
- Modify: `crates/roci-core/src/models/mod.rs`

- [ ] **Step 1: Write failing capability tests**

Add these tests at bottom of `crates/roci-core/src/models/capabilities.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_have_text_only_input() {
        let caps = ModelCapabilities::default();

        assert!(!caps.supports_vision);
        assert_eq!(caps.input, ModelInputCapabilities::default());
        assert!(caps.input.image.is_none());
        assert!(!caps.input.file.native_file_input);
    }

    #[test]
    fn vision_input_defaults_are_catalog_ready() {
        let input = ModelInputCapabilities::from_vision_support(true);
        let image = input.image.expect("vision input");

        assert_eq!(image.max_images, 20);
        assert_eq!(image.max_image_bytes, Some(20 * 1024 * 1024));
        assert_eq!(image.max_total_image_bytes, Some(50 * 1024 * 1024));
        assert_eq!(image.image_token_estimate, 1200);
        assert_eq!(
            image.supported_mime_types,
            vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/webp".to_string(),
                "image/gif".to_string(),
            ]
        );
    }

    #[test]
    fn input_capabilities_round_trip_through_json() {
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities::from_vision_support(true),
            ..ModelCapabilities::default()
        };

        let json = serde_json::to_string(&caps).expect("serialize capabilities");
        let decoded: ModelCapabilities =
            serde_json::from_str(&json).expect("deserialize capabilities");

        assert_eq!(decoded, caps);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p roci-core capabilities
```

Expected: compile failure for missing `ModelInputCapabilities` and `input` field.

- [ ] **Step 3: Add capability types and helpers**

Replace the type section in `crates/roci-core/src/models/capabilities.rs` with this shape while preserving the tests added in Step 1:

```rust
//! Model capabilities descriptor.

use serde::{Deserialize, Serialize};

const DEFAULT_IMAGE_MAX_IMAGES: usize = 20;
const DEFAULT_IMAGE_MAX_BYTES: usize = 20 * 1024 * 1024;
const DEFAULT_IMAGE_MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_IMAGE_TOKEN_ESTIMATE: usize = 1200;

/// Describes what a model can do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub supports_json_mode: bool,
    pub supports_json_schema: bool,
    pub supports_reasoning: bool,
    pub supports_system_messages: bool,
    pub context_length: usize,
    pub max_output_tokens: Option<usize>,
    pub input: ModelInputCapabilities,
}

/// Provider-independent model input limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInputCapabilities {
    pub text: TextInputCapabilities,
    pub image: Option<ImageInputCapabilities>,
    pub file: FileInputCapabilities,
}

impl ModelInputCapabilities {
    pub fn from_vision_support(supports_vision: bool) -> Self {
        Self {
            image: supports_vision.then(ImageInputCapabilities::default),
            ..Self::default()
        }
    }
}

impl Default for ModelInputCapabilities {
    fn default() -> Self {
        Self {
            text: TextInputCapabilities::default(),
            image: None,
            file: FileInputCapabilities::default(),
        }
    }
}

/// Text input limits after attachment resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TextInputCapabilities {
    pub max_text_bytes: Option<usize>,
    pub max_text_tokens: Option<usize>,
}

/// Image input limits after attachment resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageInputCapabilities {
    pub max_images: usize,
    pub max_image_bytes: Option<usize>,
    pub max_total_image_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
    pub image_token_estimate: usize,
}

impl Default for ImageInputCapabilities {
    fn default() -> Self {
        Self {
            max_images: DEFAULT_IMAGE_MAX_IMAGES,
            max_image_bytes: Some(DEFAULT_IMAGE_MAX_BYTES),
            max_total_image_bytes: Some(DEFAULT_IMAGE_MAX_TOTAL_BYTES),
            supported_mime_types: vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/webp".to_string(),
                "image/gif".to_string(),
            ],
            image_token_estimate: DEFAULT_IMAGE_TOKEN_ESTIMATE,
        }
    }
}

/// Reserved V1 file input limits. Native file payloads are disabled by default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileInputCapabilities {
    pub native_file_input: bool,
    pub max_files: usize,
    pub max_file_bytes: Option<usize>,
    pub max_total_file_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            supports_vision: false,
            supports_tools: false,
            supports_streaming: true,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 4096,
            max_output_tokens: None,
            input: ModelInputCapabilities::default(),
        }
    }
}
```

- [ ] **Step 4: Re-export capability types**

Update `crates/roci-core/src/models/mod.rs`:

```rust
pub use capabilities::{
    FileInputCapabilities, ImageInputCapabilities, ModelCapabilities, ModelInputCapabilities,
    TextInputCapabilities,
};
```

- [ ] **Step 5: Run capability tests**

Run:

```bash
cargo test -p roci-core capabilities
```

Expected: capability tests pass or only unrelated `ModelCapabilities` literal compile errors remain for later task.

---

### Task 3: Add Resolved Attachment Preflight API

**Files:**
- Create: `crates/roci-core/src/attachments/preflight.rs`
- Modify: `crates/roci-core/src/attachments/mod.rs`
- Modify: `crates/roci-core/src/prelude.rs`

- [ ] **Step 1: Write failing preflight tests**

Create `crates/roci-core/src/attachments/preflight.rs` with the module skeleton and tests first:

```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::TokenCount;
use crate::models::ModelCapabilities;

use super::types::ResolvedAttachment;

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

pub fn preflight_resolved_attachments(
    _attachments: &[ResolvedAttachment],
    _capabilities: &ModelCapabilities,
) -> Result<AttachmentPreflightReport, AttachmentPreflightError> {
    Ok(AttachmentPreflightReport {
        total_attachments: 0,
        text_attachments: 0,
        image_attachments: 0,
        total_bytes: 0,
        text_bytes: 0,
        image_bytes: 0,
        estimated_tokens: TokenCount::zero(),
        text_tokens: TokenCount::zero(),
        image_tokens: TokenCount::zero(),
    })
}

#[cfg(test)]
mod tests {
    use crate::attachments::{
        AttachmentMetadata, AttachmentSource, ResolvedAttachment,
    };
    use crate::models::{
        ImageInputCapabilities, ModelCapabilities, ModelInputCapabilities, TextInputCapabilities,
    };

    use super::*;

    fn text_attachment(text: &str) -> ResolvedAttachment {
        ResolvedAttachment::Text {
            text: text.to_string(),
            metadata: AttachmentMetadata {
                source: AttachmentSource::Selection,
                name: Some("note.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                size_bytes: text.len(),
            },
        }
    }

    fn image_attachment(mime_type: &str, size_bytes: usize) -> ResolvedAttachment {
        ResolvedAttachment::Image {
            data: vec![0; size_bytes],
            metadata: AttachmentMetadata {
                source: AttachmentSource::Blob,
                name: Some("image.bin".to_string()),
                mime_type: Some(mime_type.to_string()),
                size_bytes,
            },
        }
    }

    fn vision_caps() -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities::from_vision_support(true),
            ..ModelCapabilities::default()
        }
    }

    #[test]
    fn text_attachment_reports_bytes_and_tokens() {
        let report =
            preflight_resolved_attachments(&[text_attachment("abcdefgh")], &ModelCapabilities::default())
                .expect("preflight");

        assert_eq!(report.total_attachments, 1);
        assert_eq!(report.text_attachments, 1);
        assert_eq!(report.image_attachments, 0);
        assert_eq!(report.text_bytes, 8);
        assert_eq!(report.text_tokens.tokens, 2);
        assert_eq!(report.estimated_tokens.tokens, 2);
    }

    #[test]
    fn non_vision_model_rejects_image() {
        let err = preflight_resolved_attachments(
            &[image_attachment("image/png", 4)],
            &ModelCapabilities::default(),
        )
        .expect_err("image should fail");

        assert_eq!(err, AttachmentPreflightError::ImageUnsupported);
    }

    #[test]
    fn vision_model_accepts_allowed_image_and_counts_tokens() {
        let report =
            preflight_resolved_attachments(&[image_attachment("image/png", 4)], &vision_caps())
                .expect("preflight");

        assert_eq!(report.image_attachments, 1);
        assert_eq!(report.image_bytes, 4);
        assert_eq!(report.image_tokens.tokens, 1200);
        assert_eq!(report.estimated_tokens.tokens, 1200);
    }

    #[test]
    fn image_mime_allowlist_is_enforced() {
        let err =
            preflight_resolved_attachments(&[image_attachment("image/bmp", 4)], &vision_caps())
                .expect_err("mime should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageMimeUnsupported {
                name: "image.bin".to_string(),
                mime_type: "image/bmp".to_string(),
            }
        );
    }

    #[test]
    fn image_count_limit_is_enforced() {
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities {
                image: Some(ImageInputCapabilities {
                    max_images: 1,
                    ..ImageInputCapabilities::default()
                }),
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };

        let err = preflight_resolved_attachments(
            &[image_attachment("image/png", 4), image_attachment("image/png", 4)],
            &caps,
        )
        .expect_err("count should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageCountExceeded { count: 2, max: 1 }
        );
    }

    #[test]
    fn image_byte_limits_are_enforced() {
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities {
                image: Some(ImageInputCapabilities {
                    max_image_bytes: Some(3),
                    ..ImageInputCapabilities::default()
                }),
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };

        let err =
            preflight_resolved_attachments(&[image_attachment("image/png", 4)], &caps)
                .expect_err("bytes should fail");

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
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities {
                image: Some(ImageInputCapabilities {
                    max_image_bytes: Some(3),
                    ..ImageInputCapabilities::default()
                }),
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };
        let image = ResolvedAttachment::Image {
            data: vec![0; 4],
            metadata: AttachmentMetadata {
                source: AttachmentSource::Blob,
                name: Some("image.bin".to_string()),
                mime_type: Some("image/png".to_string()),
                size_bytes: 1,
            },
        };

        let err = preflight_resolved_attachments(&[image], &caps)
            .expect_err("actual payload bytes should fail");

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
    fn image_total_byte_limit_is_enforced() {
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities {
                image: Some(ImageInputCapabilities {
                    max_total_image_bytes: Some(7),
                    ..ImageInputCapabilities::default()
                }),
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };

        let err = preflight_resolved_attachments(
            &[image_attachment("image/png", 4), image_attachment("image/png", 4)],
            &caps,
        )
        .expect_err("total bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::ImageTotalBytesExceeded { size: 8, max: 7 }
        );
    }

    #[test]
    fn text_byte_limit_uses_actual_text_length() {
        let caps = ModelCapabilities {
            input: ModelInputCapabilities {
                text: TextInputCapabilities {
                    max_text_bytes: Some(3),
                    max_text_tokens: None,
                },
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };

        let err =
            preflight_resolved_attachments(&[text_attachment("abcd")], &caps)
                .expect_err("bytes should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::TextBytesExceeded {
                name: "note.txt".to_string(),
                size: 4,
                max: 3,
            }
        );
    }

    #[test]
    fn text_token_limit_is_enforced() {
        let caps = ModelCapabilities {
            input: ModelInputCapabilities {
                text: TextInputCapabilities {
                    max_text_bytes: None,
                    max_text_tokens: Some(1),
                },
                ..ModelInputCapabilities::default()
            },
            ..ModelCapabilities::default()
        };

        let err =
            preflight_resolved_attachments(&[text_attachment("abcdefgh")], &caps)
                .expect_err("tokens should fail");

        assert_eq!(
            err,
            AttachmentPreflightError::TextTokensExceeded { tokens: 2, max: 1 }
        );
    }

    #[test]
    fn preflight_report_round_trips_through_json() {
        let report =
            preflight_resolved_attachments(&[text_attachment("abcd")], &ModelCapabilities::default())
                .expect("preflight");

        let json = serde_json::to_string(&report).expect("serialize report");
        let decoded: AttachmentPreflightReport =
            serde_json::from_str(&json).expect("deserialize report");

        assert_eq!(decoded, report);
    }
}
```

- [ ] **Step 2: Register module so tests compile**

Update `crates/roci-core/src/attachments/mod.rs`:

```rust
mod preflight;
mod renderer;
mod resolver;
mod types;

pub use preflight::{
    preflight_resolved_attachments, AttachmentPreflightError, AttachmentPreflightReport,
};
```

Keep existing exports below this block.

- [ ] **Step 3: Run tests to verify dummy implementation fails assertions**

Run:

```bash
cargo test -p roci-core attachments::preflight
```

Expected: tests compile and fail because the dummy report does not count text/image attachments and does not reject unsupported images.

- [ ] **Step 4: Implement preflight**

Replace the dummy `preflight_resolved_attachments` function in `crates/roci-core/src/attachments/preflight.rs` with:

```rust
pub fn preflight_resolved_attachments(
    attachments: &[ResolvedAttachment],
    capabilities: &ModelCapabilities,
) -> Result<AttachmentPreflightReport, AttachmentPreflightError> {
    let mut report = AttachmentPreflightReport {
        total_attachments: attachments.len(),
        text_attachments: 0,
        image_attachments: 0,
        total_bytes: 0,
        text_bytes: 0,
        image_bytes: 0,
        estimated_tokens: TokenCount::zero(),
        text_tokens: TokenCount::zero(),
        image_tokens: TokenCount::zero(),
    };

    for attachment in attachments {
        match attachment {
            ResolvedAttachment::Text { text, metadata } => {
                let name = attachment_name(metadata);
                let size = text.len();
                if let Some(max) = capabilities.input.text.max_text_bytes {
                    if size > max {
                        return Err(AttachmentPreflightError::TextBytesExceeded {
                            name,
                            size,
                            max,
                        });
                    }
                }

                report.text_attachments += 1;
                report.text_bytes += size;
                report.total_bytes += size;
                report.text_tokens += TokenCount::heuristic(crate::context::estimate_text_tokens(text));
            }
            ResolvedAttachment::Image { data, metadata } => {
                let image_caps = capabilities
                    .input
                    .image
                    .as_ref()
                    .ok_or(AttachmentPreflightError::ImageUnsupported)?;
                let name = attachment_name(metadata);
                let size = data.len();

                if let Some(max) = image_caps.max_image_bytes {
                    if size > max {
                        return Err(AttachmentPreflightError::ImageBytesExceeded {
                            name,
                            size,
                            max,
                        });
                    }
                }

                let mime_type = metadata
                    .mime_type
                    .clone()
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                if !mime_matches(&mime_type, &image_caps.supported_mime_types) {
                    return Err(AttachmentPreflightError::ImageMimeUnsupported {
                        name,
                        mime_type,
                    });
                }

                report.image_attachments += 1;
                report.image_bytes += size;
                report.total_bytes += size;
                report.image_tokens += TokenCount::heuristic(image_caps.image_token_estimate);
            }
        }
    }

    if let Some(image_caps) = capabilities.input.image.as_ref() {
        if report.image_attachments > image_caps.max_images {
            return Err(AttachmentPreflightError::ImageCountExceeded {
                count: report.image_attachments,
                max: image_caps.max_images,
            });
        }
        if let Some(max) = image_caps.max_total_image_bytes {
            if report.image_bytes > max {
                return Err(AttachmentPreflightError::ImageTotalBytesExceeded {
                    size: report.image_bytes,
                    max,
                });
            }
        }
    }

    if let Some(max) = capabilities.input.text.max_text_tokens {
        if report.text_tokens.tokens > max {
            return Err(AttachmentPreflightError::TextTokensExceeded {
                tokens: report.text_tokens.tokens,
                max,
            });
        }
    }

    report.estimated_tokens = report.text_tokens + report.image_tokens;
    Ok(report)
}

fn attachment_name(metadata: &super::types::AttachmentMetadata) -> String {
    metadata.name.clone().unwrap_or_else(|| match &metadata.source {
        super::types::AttachmentSource::File { path } => path.display().to_string(),
        super::types::AttachmentSource::Blob => "blob".to_string(),
        super::types::AttachmentSource::Selection => "selection".to_string(),
    })
}

fn mime_matches(actual: &str, allowed: &[String]) -> bool {
    let actual_base = actual
        .split_once(';')
        .map_or(actual, |(base, _)| base)
        .trim()
        .to_ascii_lowercase();
    allowed
        .iter()
        .any(|mime| mime.trim().eq_ignore_ascii_case(&actual_base))
}
```

- [ ] **Step 5: Re-export from prelude**

Update `crates/roci-core/src/prelude.rs` attachment export block:

```rust
pub use crate::attachments::{
    preflight_resolved_attachments, render_prompt_input_text, render_resolved_text, Attachment,
    AttachmentMetadata, AttachmentPreflightError, AttachmentPreflightReport,
    AttachmentResolveOptions, AttachmentResolver, AttachmentSource, AttachmentTextRenderer,
    BlobAttachment, DefaultAttachmentResolver, FileAttachment, PromptInput, ResolvedAttachment,
    SelectionAttachment,
};
```

- [ ] **Step 6: Run preflight tests**

Run:

```bash
cargo test -p roci-core attachments::preflight
```

Expected: PASS.

---

### Task 4: Wire Provider Model Constructors to New Capability Shape

**Files:**
- Modify: `crates/roci-providers/src/models/openai.rs`
- Modify: `crates/roci-providers/src/models/anthropic.rs`
- Modify: `crates/roci-providers/src/models/google.rs`
- Modify: `crates/roci-providers/src/models/mistral.rs`
- Modify: `crates/roci-providers/src/models/grok.rs`
- Modify: `crates/roci-providers/src/models/groq.rs`
- Modify: `crates/roci-providers/src/models/ollama.rs`
- Modify: `crates/roci-providers/src/models/lmstudio.rs`

- [ ] **Step 1: Run provider compile check to expose missing `input` fields**

Run:

```bash
cargo check -p roci-providers
```

Expected: compile failures for `missing field input in initializer of ModelCapabilities`.

- [ ] **Step 2: Update imports**

In each provider model file, replace:

```rust
use roci_core::models::ModelCapabilities;
```

with:

```rust
use roci_core::models::{ModelCapabilities, ModelInputCapabilities};
```

- [ ] **Step 3: Add `input` to each constructor**

For constructors that already compute a local `vision` bool, add:

```rust
input: ModelInputCapabilities::from_vision_support(vision),
```

For always-vision constructors, add:

```rust
input: ModelInputCapabilities::from_vision_support(true),
```

For text-only constructors, add:

```rust
input: ModelInputCapabilities::default(),
```

Apply as follows:

- `openai.rs`: use existing `vision` tuple value.
- `mistral.rs`: assign `let vision = matches!(self, Self::MistralLarge);`, use it for `supports_vision` and `input`.
- `anthropic.rs`: always true.
- `google.rs`: always true.
- `grok.rs`: if current constructor is text-only, default; if it has a vision bool, use that bool.
- `groq.rs`: default.
- `ollama.rs`: default.
- `lmstudio.rs`: default.

- [ ] **Step 4: Run provider compile check**

Run:

```bash
cargo check -p roci-providers
```

Expected: provider model files compile or remaining failures point to test/support `ModelCapabilities` literals.

---

### Task 5: Update Core Test/Support Capability Literals

**Files:**
- Modify any files reported by workspace compile checks that construct `ModelCapabilities` with struct literals.
- Likely modify: `examples/custom_provider.rs` because it keeps a `static ModelCapabilities` literal.

- [ ] **Step 1: Find remaining literals**

Run:

```bash
rg -n "ModelCapabilities \\{" crates src examples tests
```

Expected: list of constructors/tests. Provider model constructors should now include `input`.

- [ ] **Step 2: Fix text-only test literals**

For test/support stubs that do not need media support, add this field:

```rust
input: ModelInputCapabilities::default(),
```

and import the type near existing `ModelCapabilities` imports:

```rust
use crate::models::{ModelCapabilities, ModelInputCapabilities};
```

or for external crate files:

```rust
use roci_core::models::{ModelCapabilities, ModelInputCapabilities};
```

- [ ] **Step 2b: Fix static example literals**

If `examples/custom_provider.rs` still uses `static CAPS: ModelCapabilities`, replace the static with a lazy value because `ModelInputCapabilities::default()` cannot be called inside a `static` initializer:

```rust
use std::sync::{Arc, OnceLock};
```

Then update `capabilities()`:

```rust
    fn capabilities(&self) -> &ModelCapabilities {
        static CAPS: OnceLock<ModelCapabilities> = OnceLock::new();
        CAPS.get_or_init(|| ModelCapabilities {
            supports_vision: false,
            supports_tools: false,
            supports_streaming: true,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 4096,
            max_output_tokens: None,
            input: ModelInputCapabilities::default(),
        })
    }
```

Also change the import to:

```rust
use roci::models::capabilities::{ModelCapabilities, ModelInputCapabilities};
```

- [ ] **Step 3: Fix vision test literals**

For tests that should model image support, add:

```rust
input: ModelInputCapabilities::from_vision_support(true),
```

and ensure `supports_vision: true`.

- [ ] **Step 4: Run all-target compile checks**

Run:

```bash
cargo check -p roci-core --all-targets
cargo check -p roci-providers --all-targets
cargo check --workspace --all-targets
```

Expected: PASS.

---

### Task 6: Add Provider Capability Regression Tests

**Files:**
- Modify: provider model files that already contain model tests, or add `#[cfg(test)] mod tests` to `crates/roci-providers/src/models/openai.rs`, `anthropic.rs`, and one text-only model file.

- [ ] **Step 1: Add OpenAI vision/text regression tests**

Add tests in `crates/roci-providers/src/models/openai.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt4o_has_vision_input_capabilities() {
        let caps = OpenAiModel::Gpt4o.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn gpt4_text_model_has_no_image_input_capabilities() {
        let caps = OpenAiModel::Gpt4.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }
}
```

- [ ] **Step 2: Add Anthropic regression test**

Add test in `crates/roci-providers/src/models/anthropic.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_models_have_vision_input_capabilities() {
        let caps = AnthropicModel::ClaudeSonnet4.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }
}
```

- [ ] **Step 3: Add text-only regression test**

Add test in `crates/roci-providers/src/models/groq.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groq_models_have_no_image_input_capabilities() {
        let caps = GroqModel::Llama3370bVersatile.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }
}
```

- [ ] **Step 4: Run provider tests**

Run:

```bash
cargo test -p roci-providers gpt4o_has_vision_input_capabilities
cargo test -p roci-providers anthropic_models_have_vision_input_capabilities
cargo test -p roci-providers groq_models_have_no_image_input_capabilities
```

Expected: PASS.

---

### Task 7: Final Verification

**Files:**
- No new files beyond prior tasks.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt
```

Expected: no output or formatted files only from this task.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test -p roci-core attachments::preflight
cargo test -p roci-core capabilities
cargo test -p roci-core token_count_round_trips_through_json
cargo test -p roci-core attachments::
cargo test -p roci-providers gpt4o_has_vision_input_capabilities
cargo test -p roci-providers anthropic_models_have_vision_input_capabilities
cargo test -p roci-providers groq_models_have_no_image_input_capabilities
```

Expected: PASS.

- [ ] **Step 3: Run package checks**

Run:

```bash
cargo clippy -p roci-core -p roci-providers --all-targets -- -D warnings
cargo test -p roci-core -p roci-providers
cargo check --workspace --all-targets
```

Expected: PASS.

- [ ] **Step 4: Inspect diff**

Run:

```bash
git diff --stat
git diff -- crates/roci-core/src/context/tokens.rs crates/roci-core/src/models/capabilities.rs crates/roci-core/src/models/mod.rs crates/roci-core/src/attachments/preflight.rs crates/roci-core/src/attachments/mod.rs crates/roci-core/src/prelude.rs crates/roci-providers/src/models examples/custom_provider.rs
```

Expected: diff limited to capability shape, preflight API, exports, provider model constructors/tests, and example capability literal cleanup.

---

## Self-Review

- Spec coverage: capability shape in Task 2; token accounting in Tasks 1 and 3; preflight in Task 3; provider defaults in Tasks 4 and 6; V1 no native file rule in Task 2 default file caps and no `ContentPart::File` changes.
- Placeholder scan: no deferred implementation steps. Each code-changing step includes concrete code or exact replacement pattern.
- Type consistency: `ModelInputCapabilities::from_vision_support`, `AttachmentPreflightReport`, `AttachmentPreflightError`, and `preflight_resolved_attachments` names match across tasks.
- Scope check: provider payload mapping, runtime prompt queue wiring, CLI `--attach`, docs/live provider verification stay in downstream tasks from the attachment epic.
- Plan-review fixes applied: `OpenAiModel` spelling fixed, workspace/example compile checks added, preflight byte limits use actual payload lengths, text byte/image total/serde tests added, and capability literal search widened to workspace paths.
