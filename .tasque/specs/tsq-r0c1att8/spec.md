## Overview
Add first-class host-facing attachments for prompt inputs and CLI chat. V1 supports text and images only: text files/selections render into model-visible text, images use existing `ContentPart::Image`, unsupported opaque blobs fail preflight.

## Constraints / Non-goals
- Active development: breaking API changes allowed; no compatibility shims.
- No native provider file upload / `ContentPart::File` in V1.
- Only text and image attachments for now.
- MIME detection uses `mime_guess` plus UTF-8 fallback; no deep content sniffing.
- Attachment text is model-visible; hosts must avoid attaching secrets.
- Sessioned attachments do not make host cwd into session workspace.

## Interfaces (CLI/API)
- New `roci_core::attachments` module.
- Types: `Attachment`, `FileAttachment`, `BlobAttachment`, `SelectionAttachment`, `ResolvedAttachment`, `AttachmentMetadata`, `AttachmentResolver`, `AttachmentResolveOptions`, `PromptInput`.
- `ModelCapabilities` gains final media/file limit shape used by both attachments and model catalog.
- `ModelMessage` gains attachment metadata.
- Runtime prompt APIs accept `impl Into<PromptInput>`; steer/follow_up return `Result<(), RociError>` because resolution can fail.
- CLI: `roci-agent chat --attach <path>` repeatable.

## Data model / schema changes
- Text attachments become bounded rendered `ContentPart::Text` with metadata.
- Image attachments become existing `ContentPart::Image` with MIME/data and metadata.
- Unsupported blobs/native files return preflight errors before provider call.
- Token estimation accounts for rendered text and conservative image placeholders.
- Provider payload mappings rely on existing image serialization for OpenAI Chat/Responses, Anthropic, and Google.
- In a durable session, `--attach <host_path>` is ephemeral prompt input with source metadata. Explicit import/copy into session `files/` is separate session/workspace API behavior.

## Acceptance criteria
- Resolver tests cover file/blob/selection, size/count limits, MIME, UTF-8 fallback, unsupported opaque blobs.
- Serde/token/preflight tests pass.
- Runtime queue tests prove PromptInput flows through prompt/continue/steer/follow-up and chat metadata.
- Provider JSON assertions prove image/text payload shape.
- CLI parse and chat wiring tests pass.
- Sessioned CLI test proves `--attach` never turns host cwd into session workspace implicitly.
- Docs and live tmux text+vision smoke complete or clearly report missing vision-capable provider/auth.

## Test plan
- `cargo test -p roci-core attachments`
- `cargo test -p roci-core --features agent "agent::runtime::tests::attachments"`
- `cargo test -p roci-providers provider_attachment_payload`
- `cargo test -p roci-cli attach`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --features full -- -D warnings`
- `cargo test`
- Live tmux text attachment smoke and vision attachment smoke per `docs/testing.md`.
