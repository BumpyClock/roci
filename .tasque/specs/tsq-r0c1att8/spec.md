# First-Class Attachments API and CLI Support Implementation Plan

> **For agentic workers:** Execute task-by-task. Use subagent-driven development when available, otherwise run tasks inline with review checkpoints. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add host-facing attachment inputs, resolver, runtime prompt APIs, CLI `--attach`, provider preflight, provider payload assertions, docs, and live text/vision smoke.

**Architecture:** Keep provider transports unchanged where possible by using documented fallback for v1: text files/selections render into `ContentPart::Text`, images use existing `ContentPart::Image`, and opaque/native file blobs fail during resolver/preflight until roci chooses provider-native file upload semantics. Attachment metadata lives in core message/runtime snapshots so hosts can render attachment chips without scraping prompt text.

**Tech Stack:** Rust 2021, serde, tokio, base64, mime_guess, roci-core agent feature, roci-providers payload builders, clap CLI, cargo test/clippy/rustfmt, tmux live provider smoke.

---

## Current Architecture Facts

- `roci-core` owns provider-agnostic `ModelMessage`, `ContentPart`, `ModelCapabilities`, `AgentRuntime`, chat snapshots/events, and runner preflight.
- `ContentPart` currently supports `Text`, `Image`, tool calls/results, and thinking blocks. `ImageContent` is base64 data plus MIME type.
- OpenAI Chat, OpenAI Responses, Anthropic, and Google providers already serialize `ContentPart::Image` into their provider JSON shapes.
- `ModelCapabilities` has `supports_vision: bool` but no MIME/count/size limits and no native file-input contract.
- `AgentRuntime::prompt`, `continue_run`, `steer`, and `follow_up` accept text only and create `ModelMessage::user` directly.
- `EnqueueTurnRequest` accepts raw `Vec<ModelMessage>` and can remain the low-level escape hatch.
- `MessageSnapshot` contains `payload: ModelMessage`; adding attachment metadata to `ModelMessage` makes chat event metadata visible without changing event payload variants.
- Runner preflight validates transport and context budgets, not attachment caps.
- `roci-cli chat` owns clap parsing and calls `agent.prompt(prompt)`. No attachment flag exists.

## Proposed Public API / Types

Add `roci_core::attachments` and re-export from `roci_core::lib`, `types` or `prelude` as appropriate:

- `Attachment`: enum with `File(FileAttachment)`, `Blob(BlobAttachment)`, `Selection(SelectionAttachment)`.
- `FileAttachment`: `{ path: PathBuf, mime_type: Option<String>, display_name: Option<String> }`.
- `BlobAttachment`: `{ data_base64: String, mime_type: String, display_name: Option<String> }`, plus `from_bytes` constructor.
- `SelectionAttachment`: `{ text: String, label: Option<String>, source: Option<String> }`.
- `ResolvedAttachment`: `{ id: AttachmentId, metadata: AttachmentMetadata, content: ResolvedAttachmentContent }`.
- `ResolvedAttachmentContent`: `RenderedText(String)`, `ImageBase64 { data_base64: String }`.
- `AttachmentMetadata`: `{ id, name, mime_type, byte_len, kind, rendered_as }`.
- `AttachmentKind`: `File`, `Blob`, `Selection`; `AttachmentRenderMode`: `Text`, `Image`.
- `AttachmentResolver`: async resolver with `resolve_all(&[Attachment], AttachmentResolveOptions) -> Result<Vec<ResolvedAttachment>, RociError>`.
- `AttachmentResolveOptions`: max count, max bytes per attachment, max total bytes, max rendered text bytes; default conservative but large enough for CLI smoke.
- `PromptInput`: `{ text: String, attachments: Vec<Attachment> }` with `From<&str>`, `From<String>`, builder methods `attachment`, `attachments`.

Message/capability API:

- Add `attachment_metadata: Vec<AttachmentMetadata>` to `ModelMessage` with serde default + skip when empty.
- Do not add `ContentPart::File` in v1. Document fallback explicitly.
- Add `VisionLimits` and `FileInputLimits` to `ModelCapabilities`:
  - `VisionLimits { max_images_per_request: Option<usize>, max_image_bytes: Option<u64>, allowed_mime_types: Vec<String> }`.
  - `FileInputLimits { native_file_input: bool, max_files_per_request: Option<usize>, max_file_bytes: Option<u64>, max_total_file_bytes: Option<u64>, allowed_mime_types: Vec<String> }`.
- Existing providers set `supports_vision` and `vision_limits`; `native_file_input` remains false for all providers in v1.

Runtime API:

- Change `prompt` and `continue_run` to accept `impl Into<PromptInput>` while preserving text calls through `From<&str/String>`.
- Change `steer` and `follow_up` to return `Result<(), RociError>` and accept `impl Into<PromptInput>` because attachment resolution can fail.
- Add internal `prompt_input_to_user_message` helper in runtime lifecycle: resolve attachments, render content parts, attach metadata.
- Keep `continue_without_input` unchanged.

## File / Module Changes

- Create `crates/roci-core/src/attachments/mod.rs`: exports types, resolver, renderer.
- Create `crates/roci-core/src/attachments/types.rs`: public attachment structs/enums/newtypes.
- Create `crates/roci-core/src/attachments/resolver.rs`: file/blob/selection resolve, MIME guess, size/count checks.
- Create `crates/roci-core/src/attachments/render.rs`: stable text-file/selection renderer and content-part conversion.
- Modify `crates/roci-core/Cargo.toml`: add `mime_guess = "2"`.
- Modify `crates/roci-core/src/lib.rs` and `crates/roci-core/src/prelude.rs`: export attachments.
- Modify `crates/roci-core/src/types/message.rs`: add `attachment_metadata` to `ModelMessage`, constructor defaults.
- Modify `crates/roci-core/src/context/tokens.rs`: token tests for rendered text/image metadata path; no native file counting in v1.
- Modify `crates/roci-core/src/models/capabilities.rs`: add `VisionLimits`, `FileInputLimits`, defaults.
- Modify `crates/roci-providers/src/models/{openai,anthropic,google,grok,mistral}.rs`: fill vision limits where `supports_vision` is true.
- Modify `crates/roci-core/src/agent_loop/runner/engine/llm_phase.rs`: validate image MIME/count/byte limits before provider stream.
- Modify `crates/roci-core/src/generation/{text,stream}.rs`: same capability validation for direct generation helpers.
- Modify `crates/roci-core/src/agent/runtime/lifecycle.rs`: PromptInput conversion for prompt/continue/steer/follow-up.
- Modify `crates/roci-core/src/agent/runtime/chat/{domain,projector,event}.rs` only if snapshot metadata needs explicit helper methods; prefer `ModelMessage.attachment_metadata` inside `MessageSnapshot.payload`.
- Modify provider tests in `crates/roci-providers/src/provider/{openai.rs,anthropic.rs,google.rs,openai_responses/request.rs}` for image payload assertions.
- Modify `crates/roci-cli/src/cli/mod.rs`: add repeatable `--attach PATH` to `ChatArgs` and parse tests.
- Modify `crates/roci-cli/src/chat.rs`: build `PromptInput` from prompt + file attachments and call runtime prompt.
- Modify docs: create `docs/attachments.md`; update `docs/ARCHITECTURE.md`, `docs/agent-runtime-chat.md`, `docs/testing.md` if live command examples move there.

## Dependency Order / Child Tasks

1. `tsq-r0c1att8.1` Define attachment types resolver and text renderer.
2. `tsq-r0c1att8.2` Extend message content capabilities tokens and preflight validation. Blocks on .1.
3. `tsq-r0c1att8.3` Wire PromptInput through AgentRuntime queues and chat metadata. Blocks on .2.
4. `tsq-r0c1att8.4` Add provider attachment payload mappings and assertions. Blocks on .2.
5. `tsq-r0c1att8.5` Add roci-cli --attach parsing and chat wiring. Blocks on .3.
6. `tsq-r0c1att8.6` Update attachment docs and run automated plus live verification. Blocks on .4 and .5.

Parallel after .2: provider payload work (.4) and runtime API work (.3) can run together; CLI waits for runtime; docs/live waits for provider + CLI.

## Tests

- Resolver tests: text file, markdown/json MIME, image MIME/base64, blob text/image, selection, max count, max per-file bytes, max total bytes, unsupported/unknown binary error.
- Serde tests: `Attachment`, `ResolvedAttachment`, `AttachmentMetadata`, `ModelMessage` with metadata round trip; empty metadata skipped.
- Token tests: rendered text attachment counted via text content; image still counted through existing `ImageContent` heuristic.
- Preflight tests: image on `supports_vision=false` fails before provider stream; unsupported MIME fails; count/byte caps fail; text-rendered attachments pass on text-only model.
- Runtime queue tests: prompt with text file emits user message with rendered text + metadata; `continue_run`, `steer`, `follow_up` preserve attachment metadata and queue order; failed attachment resolve restores idle state.
- Provider JSON assertions: OpenAI Chat `image_url`, OpenAI Responses `input_image`, Anthropic `source.base64`, Google `inlineData`; text attachment appears as text, metadata not leaked as provider JSON fields.
- CLI parse tests: default empty `attach`, repeated `--attach`, prompt positional unaffected.

Focused commands:

- `cargo test -p roci-core attachments`
- `cargo test -p roci-core --features agent "agent::runtime::tests::"`
- `cargo test -p roci-providers openai_responses -- --nocapture`
- `cargo test -p roci-providers image`
- `cargo test -p roci-cli parse_chat`

Full gates:

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test`

## Docs / Live Verification

Docs:

- `docs/attachments.md`: public API examples, CLI examples, fallback policy, limits, error semantics.
- `docs/agent-runtime-chat.md`: attachment metadata in message snapshots/events.
- `docs/ARCHITECTURE.md`: `attachments` module owned by `roci-core`; CLI only maps files to PromptInput.
- `docs/testing.md`: text + vision tmux smoke examples.

Live tmux text smoke:

- Create small temp text file.
- Run `roci-agent chat --no-skills --no-tools --attach /tmp/roci-attach.txt ...` against loaded LM Studio model when available.
- Show `tmux attach -t roci-attach-text` before/while running.
- Expected: provider response references attached file content; exit code 0.

Live tmux vision smoke:

- Use a tiny PNG fixture and a configured vision-capable provider/model (`openai:gpt-4o`, `google:gemini-*`, or local vision model if LM Studio reports loaded vision model).
- Run `roci-agent chat --no-skills --no-tools --attach /tmp/roci-vision.png "Describe image in five words"`.
- Show `tmux attach -t roci-attach-vision`.
- Expected: visual description response; exit code 0. If no vision-capable provider/model/auth exists, report blocker, not completion.

## Risks / Open Questions

- Native provider file upload remains open. V1 chooses documented fallback to avoid guessing provider-specific file APIs.
- `steer`/`follow_up` return type becomes `Result`; breaking but correct because file IO/MIME/size checks can fail.
- MIME detection via `mime_guess` is extension-based plus UTF-8 fallback; content sniffing can be added later if false positives matter.
- Model catalogs may under-report local vision capability, especially LM Studio. Live vision smoke may require a provider/model override or catalog follow-up.
- Rendered text attachments are prompt-visible; hosts should not attach secrets unless intended for model context.