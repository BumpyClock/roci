# Attachment Capabilities and Preflight Design

## Context

Task `tsq-r0c1att8.2` finalizes the model-facing attachment capability shape, token accounting, and shared preflight validation needed by later attachment runtime/provider work and the model catalog.

Current roci state:

- `roci-core::attachments` resolves host attachments into `ResolvedAttachment::Text` or `ResolvedAttachment::Image`.
- `ModelCapabilities` has only `supports_vision` for media support.
- Context accounting already exposes `TokenCount`, `estimate_text_tokens`, and budget preflight utilities.
- Provider payload mapping for existing `ContentPart::Image` exists, but later task `tsq-r0c1att8.4` owns provider attachment assertions.
- V1 has no native `ContentPart::File`; files are resolver inputs that become text/image or fail.

Pi and Codex both keep user input separate from provider content and expose coarse text/image model modalities. Both degrade unsupported images into model-visible text. roci should keep the boundary, fail resource and malformed-metadata errors during preflight, and degrade safe unsupported media into bounded marker text so the agent can explain the limitation.

## Goals

- Add structured model input capabilities for text attachments, image attachments, and reserved file support.
- Keep capability data serializable and stable enough for the model catalog task.
- Add shared preflight over resolved attachments and model capabilities.
- Add attachment token estimates that match existing `TokenCount` conventions.
- Wire CLI chat attachments through the shared resolver/preflight path.
- Keep V1 file behavior explicit: file inputs resolve to text/image; native file payloads remain unsupported.

## Non-Goals

- No native `ContentPart::File` in this task.
- No provider payload rewrites in this task.
- No image resizing/transcoding pipeline in this task.
- No provider-specific exact tokenizer integration in this task.

## Capability Shape

Extend `ModelCapabilities` with a structured input capability field:

```rust
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
```

Keep `supports_vision` for compatibility with existing provider code, but make `input.image` the richer source for attachment validation. Provider constructors should set both from the same local value to avoid drift.

Proposed supporting types:

```rust
pub struct ModelInputCapabilities {
    pub text: TextInputCapabilities,
    pub image: Option<ImageInputCapabilities>,
    pub file: FileInputCapabilities,
}

pub struct TextInputCapabilities {
    pub max_text_bytes: Option<usize>,
    pub max_text_tokens: Option<usize>,
}

pub struct ImageInputCapabilities {
    pub max_images: usize,
    pub max_image_bytes: Option<usize>,
    pub max_total_image_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
    pub image_token_estimate: usize,
}

pub struct FileInputCapabilities {
    pub native_file_input: bool,
    pub max_files: usize,
    pub max_file_bytes: Option<usize>,
    pub max_total_file_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
}
```

Defaults:

- Text enabled with no model-specific byte/token cap.
- Image disabled.
- Native file input disabled.
- Vision-capable provider models use common image defaults:
  - MIME: `image/png`, `image/jpeg`, `image/webp`, `image/gif`
  - max images: `20`
  - max per image bytes: `20 MiB`
  - max total image bytes: `50 MiB`
  - image token estimate: `1200`

## Preflight API

Add shared core preflight for resolved attachments:

```rust
pub fn preflight_resolved_attachments(
    attachments: &[ResolvedAttachment],
    capabilities: &ModelCapabilities,
) -> Result<AttachmentPreflightReport, AttachmentPreflightError>;
```

Report:

```rust
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
```

Errors:

```rust
pub enum AttachmentPreflightError {
    ImageUnsupported,
    ImageCountExceeded { count: usize, max: usize },
    ImageBytesExceeded { name: String, size: usize, max: usize },
    ImageTotalBytesExceeded { size: usize, max: usize },
    ImageMimeUnsupported { name: String, mime_type: String },
    TextBytesExceeded { name: String, size: usize, max: usize },
    TextTokensExceeded { tokens: usize, max: usize },
}
```

The function validates only resolved text/image attachments. Resolver errors still own filesystem, MIME detection, UTF-8, generic byte/count limits, and malformed metadata that cannot produce a bounded unsupported-media marker. Safe unsupported media resolves to bounded marker text.

## Token Accounting

Text tokens use existing `estimate_text_tokens`.

Image tokens use `ImageInputCapabilities::image_token_estimate` per image. This is intentionally a heuristic because exact image accounting is provider/model-specific. Provider usage remains the source of truth after response.

The preflight report returns `TokenCount` values so later runtime budget code can add prompt text plus attachment estimates without inventing another accounting type.

## Data Flow

Host app:

1. Builds `PromptInput` with prompt text and attachments.
2. Resolver converts attachments into `ResolvedAttachment`.
3. Runtime calls shared preflight with provider `ModelCapabilities`.
4. Runtime degrades safe unsupported media and unsupported image inputs into bounded marker text.
5. Text attachments are rendered into the model prompt; supported image attachments are forwarded to provider mapping.

CLI chat:

1. Accepts repeatable `--attach PATH`.
2. Resolves file attachments before creating the runtime turn.
3. Calls shared preflight with the selected provider capabilities.
4. Renders text attachments into the user prompt.
5. Encodes image attachments as `ContentPart::Image` for providers whose capabilities allow images.
6. Degrades safe unsupported media and unsupported image inputs into bounded marker text.
7. Fails before provider execution for unreadable paths, invalid text UTF-8, malformed marker metadata, and resource limits.

Provider/catalog:

1. Provider model constructors populate `ModelCapabilities::input`.
2. Model catalog DTO can expose the same shape without inventing separate media fields.

## Testing

Core tests should cover:

- `ModelCapabilities::default()` has text enabled, image disabled, native file disabled.
- Vision helper/default populates image support and allowed MIME types.
- Non-vision model degrades image attachments into bounded marker text.
- Vision model accepts allowed image MIME.
- Image count, per-image bytes, total image bytes, and MIME failures.
- Text byte cap failure.
- Text token estimate included in report.
- Image token estimate included in report.
- Serde round trip for the capability and preflight report types.

Provider model tests should cover:

- Vision-capable provider models set `supports_vision == true` and `input.image.is_some()`.
- Text-only provider models set `supports_vision == false` and `input.image.is_none()`.

CLI/runtime tests should cover:

- Repeatable `--attach` parsing.
- Text file attachment rendering into the model-visible prompt.
- Unsupported image fallback for text-only model capabilities.
- Image encoding into `ContentPart::Image` for vision-capable model capabilities.
- Runtime `prompt_message` preserving multipart image content through to provider requests.

Live verification should cover:

- `roci-agent chat --attach <text-file>` against a local LM Studio model and assert the model sees attached text.
- `roci-agent chat --attach <image-file>` against the same text-only local model and assert image content degrades into an unsupported-media marker unless resource limits fail.

## Acceptance

- `roci-core` exposes the structured capability and preflight API.
- Provider model capability constructors compile with the new field.
- `roci-agent chat --attach` uses the shared resolver/preflight path.
- Attachment preflight tests pass.
- Existing attachment resolver tests keep passing.
- Live CLI attachment smoke proves text attachments reach the provider and unsupported media markers reach the provider when content is unsupported.
- No native file content part is introduced.
- Later tasks `tsq-r0c1att8.3`, `tsq-r0c1att8.4`, and `tsq-r0c1m0d5.1` can build on this shape.
