# Runtime PromptInput Design

## Task

`tsq-r0c1att8.3` wires `PromptInput` through `AgentRuntime` prompt, continue, steer, follow-up queues, and chat metadata.

## Context

Roci already has attachment primitives:

- `PromptInput` holds prompt text plus host attachments.
- `DefaultAttachmentResolver` resolves files, blobs, and selections.
- Attachment preflight validates resolved text/image input against model capabilities.
- CLI currently resolves/preflights attachments and calls `prompt_message`.

That leaves runtime APIs inconsistent: `prompt_message` accepts multipart messages, but `continue_run`, `steer`, and `follow_up` are still text-only. Chat snapshots also preserve `ModelMessage` payloads but not attachment metadata.

Pi and Codex both avoid this bottleneck:

- Pi queues full rich messages and maps provider payloads at provider boundary.
- Codex keeps typed user input in history, then derives model payloads later.

For this task, Roci should use a smaller step: compile `PromptInput` at the runtime boundary, preserve sanitized metadata in chat state, and keep provider-facing `ModelMessage` flow intact.

## Decision

Use a runtime-owned compile gate:

```text
PromptInput
  -> resolve attachments
  -> preflight against current model capabilities
  -> render text attachments into prompt text
  -> encode image attachments into ContentPart::Image
  -> produce ModelMessage + sanitized attachment metadata
  -> queue/run
```

Do not persist raw local file paths in chat/session metadata. Persist only safe, portable fields:

- source kind: `file`, `blob`, `selection`
- display name
- MIME type
- byte size
- content kind: `text`, `image`

Raw paths may appear in immediate resolver/preflight error messages, but not in persisted runtime/chat metadata.

## Public API Shape

Runtime prompt APIs accept `impl Into<PromptInput>` where practical:

- `prompt(input) -> Result<RunResult, RociError>`
- `continue_run(input) -> Result<RunResult, RociError>`
- `steer(input) -> Result<(), RociError>`
- `follow_up(input) -> Result<(), RociError>`

Text-only callers remain ergonomic via `From<String>`, `From<&str>`, or equivalent conversions into `PromptInput`.

Keep `prompt_message(ModelMessage)` as an escape hatch for hosts that already produced a model-ready user message.

## Compile Step

Add a small core helper, owned by `roci-core`, that returns:

```text
CompiledPromptInput {
    message: ModelMessage,
    attachments: Vec<AttachmentDisplayMetadata>,
}
```

The helper should:

- use `DefaultAttachmentResolver` for default runtime behavior,
- use `AttachmentResolveOptions::default()` for V1,
- call `preflight_resolved_attachments`,
- render text attachments with existing `render_prompt_input_text`,
- append images as `ContentPart::Image` with normalized MIME,
- produce sanitized metadata from each `ResolvedAttachment`.

## Chat Metadata

Add attachment metadata to message-level chat projection, not turn-level state.

Recommended shape:

```text
ModelMessage {
    role,
    content,
    name,
    timestamp,
    metadata,
}

ModelMessageMetadata {
    attachments: Vec<AttachmentDisplayMetadata>,
}
```

`MessageSnapshot.payload` already carries `ModelMessage`, so runtime events and snapshots inherit metadata without a second parallel map.

Metadata must skip empty serialization to preserve compact snapshots and old history readability.

## Runtime Flow

`prompt` and `continue_run` should compile input before mutating runtime message history.

Atomicity requirement:

- if resolve fails, no message mutation,
- if preflight fails, no message mutation,
- if chat projection fails, no durable queued turn remains,
- `AgentState` returns to idle after prompt/continue failure.

`steer` and `follow_up` compile before queue insertion. Since compile can fail, both return `Result<(), RociError>`.

Queue storage remains `Vec<ModelMessage>` for this task.

## CLI Flow

CLI should stop compiling attachment `ModelMessage` itself.

CLI should:

1. build `PromptInput` from prompt text and `--attach` paths,
2. create the runtime,
3. call `agent.prompt(input).await`.

This keeps preflight and metadata behavior identical between CLI and SDK callers.

## Error Handling

Resolver and preflight errors return before provider call.

Errors should be normal `RociError` values with existing user-readable strings from attachment resolver/preflight. This task should not add retry behavior.

No raw local path should be persisted in metadata, but resolver/preflight errors may still mention paths for immediate diagnosis.

## Tests

Core tests should prove:

- text `PromptInput` still behaves like old text prompt,
- `prompt(PromptInput)` sends rendered text attachments to provider,
- `prompt(PromptInput)` sends image attachments as image content when model supports vision,
- `prompt(PromptInput)` sends safe unsupported media and unsupported image inputs as bounded marker text,
- `continue_run(PromptInput)` preserves attachment content and metadata,
- `steer(PromptInput)` and `follow_up(PromptInput)` compile and enqueue rich messages,
- `steer` and `follow_up` return `Result`,
- failed resolve/preflight does not mutate messages, queues, or chat turns,
- chat snapshots/events expose sanitized metadata and no raw file path.

CLI tests should prove:

- `--attach` builds `PromptInput` and calls runtime-owned attachment path,
- non-vision image input becomes an unsupported-media marker before provider request.

Live verification should prove:

- local LM Studio text attachment reaches model,
- local LM Studio unsupported media or unsupported image input reaches model as bounded marker text,
- session files, when enabled, do not store raw host attachment paths in message metadata.

## Out Of Scope

- Full Codex-style structured `UserInput` history.
- Image URL attachments.
- Native provider file uploads.
- New attachment storage/import into session workspace.
- Tolerant history repair loaders.

## Follow-Up

Track a later task for structured history:

- introduce typed chat input separate from provider `ModelMessage`,
- persist structured user input and derived provider payload separately,
- support replay/edit/resubmit without reconstructing attachments from model text,
- define explicit path redaction/import policy for local files.
