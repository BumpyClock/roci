# Runtime PromptInput Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move attachment-aware `PromptInput` handling into `AgentRuntime` so prompt, continue, steer, follow-up, chat metadata, and CLI all use one runtime-owned path.

**Architecture:** Add a core compile helper that converts `PromptInput` into `ModelMessage` plus sanitized attachment metadata before runtime state mutation. Runtime APIs accept `impl Into<PromptInput>`, queue compiled messages, and expose metadata through `ModelMessage.metadata`. CLI builds `PromptInput` and lets runtime resolve/preflight.

**Tech Stack:** Rust, Tokio, serde, roci-core attachments/runtime/chat, roci-cli, cargo test/clippy/fmt, tmux live verification.

---

## File Map

- Create `crates/roci-core/src/attachments/compiler.rs`
  - Own `CompiledPromptInput`, `AttachmentDisplayMetadata`, `AttachmentContentKind`, `AttachmentSourceKind`, and `compile_prompt_input`.
- Modify `crates/roci-core/src/attachments/mod.rs`
  - Export compiler API.
- Modify `crates/roci-core/src/prelude.rs`
  - Re-export public attachment compile types.
- Modify `crates/roci-core/src/types/message.rs`
  - Add optional `metadata: Option<ModelMessageMetadata>` to `ModelMessage`.
  - Add `ModelMessageMetadata { attachments: Vec<AttachmentDisplayMetadata> }`.
  - Update constructors.
- Modify `crates/roci-core/src/agent/runtime/lifecycle.rs`
  - Compile `PromptInput` in `prompt`, `continue_run`, `steer`, and `follow_up`.
  - Preserve `prompt_message` escape hatch.
  - Avoid mutation before compile/projection success where feasible.
- Modify `crates/roci-core/src/agent/runtime.rs`
  - Update public docs examples from text-only to `PromptInput`-compatible text.
- Modify runtime tests:
  - `crates/roci-core/src/agent/runtime_tests/chat_runtime.rs`
  - `crates/roci-core/src/agent/runtime_tests/queue_and_continue.rs`
  - `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs` if compile errors require API updates.
- Modify `crates/roci-cli/src/chat.rs`
  - Remove CLI-local provider creation/preflight/message compile path.
  - Build `PromptInput` with `Attachment::file`.
  - Call `agent.prompt(input).await`.
- Modify CLI tests in `crates/roci-cli/src/chat.rs`
  - Replace `prompt_message_from_resolved` tests with `PromptInput` construction or move message compile assertions to core.
- Modify docs:
  - `docs/agent-runtime-chat.md`
  - `docs/testing.md` if live verification command changed.

## Dependency Order

Run tasks in this order:

1. Task 1 first. It defines compile API and `ModelMessage` metadata.
2. Task 2 after Task 1. It depends on compile API and metadata.
3. Task 3 after Task 2. It depends on runtime `prompt(input)` accepting `PromptInput`.
4. Task 4 can run after Task 1, but final wording should be checked after Task 2/3.
5. Task 5 last.

Parallel execution is allowed only for non-overlapping work after dependency APIs are pinned. Safe split:

- Worker A owns Task 1.
- Worker B may start Task 2 tests using Task 1 names, then finish after Worker A lands.
- Worker C may prepare docs from Task 4.
- CLI Task 3 waits until Task 2 signatures compile.

Plan review fixes included:

- Use `Attachment::Blob(BlobAttachment::new(...).with_mime_type(...))` and `Attachment::Selection(SelectionAttachment::new(...).with_name(...))`; `Attachment::blob(...)` returns enum and has no builder methods.
- Import `AttachmentResolver` in compiler.
- `CompiledPromptInput` derives `PartialEq`, not `Eq`, because `ModelMessage` is not `Eq`.
- Match `&metadata.source` to avoid moving out of borrowed metadata.
- Run an explicit `rg "ModelMessage \\{"` pass. Preserve `metadata` in transforms; use `metadata: None` only for new synthetic/test messages.
- Runtime compile must happen under the idle/running gate for prompt/continue so busy calls fail before file reads/provider creation and model capability cannot race with another run.
- Chat projection/event publish failure needs rollback from cloned `ChatProjector` state before returning an error.

---

## Task 1: Core Compile Contract

**Files:**
- Create: `crates/roci-core/src/attachments/compiler.rs`
- Modify: `crates/roci-core/src/attachments/mod.rs`
- Modify: `crates/roci-core/src/prelude.rs`
- Modify: `crates/roci-core/src/types/message.rs`
- Test: `crates/roci-core/src/attachments/tests.rs`

- [ ] **Step 1: Add metadata and compile tests first**

Add tests that describe public behavior:

```rust
#[test]
fn compile_prompt_input_renders_text_and_sanitizes_metadata() {
    let input = PromptInput::new("Inspect")
        .with_attachment(Attachment::Selection(
            SelectionAttachment::new("selected text").with_name("Selection A"),
        ));
    let caps = ModelCapabilities::default();

    let compiled = compile_prompt_input(&input, &caps).expect("input should compile");

    assert_eq!(compiled.message.role, Role::User);
    assert!(compiled.message.text().contains("Inspect"));
    assert!(compiled.message.text().contains("selected text"));
    let metadata = compiled
        .message
        .metadata
        .as_ref()
        .expect("metadata should exist");
    assert_eq!(metadata.attachments.len(), 1);
    assert_eq!(metadata.attachments[0].source_kind, AttachmentSourceKind::Selection);
    assert_eq!(metadata.attachments[0].content_kind, AttachmentContentKind::Text);
}

#[test]
fn compile_prompt_input_encodes_images_when_model_supports_vision() {
    let input = PromptInput::new("Describe")
        .with_attachment(Attachment::Blob(
            BlobAttachment::new([137, 80, 78, 71]).with_mime_type("image/png"),
        ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities::from_vision_support(true),
        ..ModelCapabilities::default()
    };

    let compiled = compile_prompt_input(&input, &caps).expect("image should compile");

    assert!(matches!(
        &compiled.message.content[1],
        ContentPart::Image(image) if image.mime_type == "image/png" && !image.data.is_empty()
    ));
    let metadata = compiled.message.metadata.as_ref().expect("metadata");
    assert_eq!(metadata.attachments[0].source_kind, AttachmentSourceKind::Blob);
    assert_eq!(metadata.attachments[0].content_kind, AttachmentContentKind::Image);
}

#[test]
fn compile_prompt_input_rejects_image_for_non_vision_model() {
    let input = PromptInput::new("Describe")
        .with_attachment(Attachment::Blob(
            BlobAttachment::new([137, 80, 78, 71]).with_mime_type("image/png"),
        ));

    let err = compile_prompt_input(&input, &ModelCapabilities::default())
        .expect_err("non-vision model should reject image");

    assert!(err.to_string().contains("model does not support image attachments"));
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p roci-core attachments::tests::compile_prompt_input --features agent
```

Expected: fail because compile types/functions do not exist.

- [ ] **Step 3: Add message metadata types**

In `crates/roci-core/src/types/message.rs`, add:

```rust
use crate::attachments::AttachmentDisplayMetadata;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelMessageMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentDisplayMetadata>,
}
```

Extend `ModelMessage`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub metadata: Option<ModelMessageMetadata>,
```

Update every `ModelMessage` constructor to set `metadata: None`.

- [ ] **Step 4: Add compiler module**

Create `crates/roci-core/src/attachments/compiler.rs` with:

```rust
use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use crate::attachments::{
    preflight_resolved_attachments, render_prompt_input_text, AttachmentResolveOptions,
    AttachmentResolver, AttachmentSource, DefaultAttachmentResolver, PromptInput,
    ResolvedAttachment,
};
use crate::error::RociError;
use crate::models::ModelCapabilities;
use crate::types::{ContentPart, ImageContent, ModelMessage, ModelMessageMetadata, Role};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledPromptInput {
    pub message: ModelMessage,
    pub attachments: Vec<AttachmentDisplayMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentSourceKind {
    File,
    Blob,
    Selection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentContentKind {
    Text,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentDisplayMetadata {
    pub source_kind: AttachmentSourceKind,
    pub content_kind: AttachmentContentKind,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: usize,
}

pub fn compile_prompt_input(
    input: &PromptInput,
    capabilities: &ModelCapabilities,
) -> Result<CompiledPromptInput, RociError> {
    let resolver = DefaultAttachmentResolver;
    let resolved = resolver
        .resolve_prompt_input(input, &AttachmentResolveOptions::default())
        .map_err(|err| RociError::InvalidState(err.to_string()))?;
    preflight_resolved_attachments(&resolved, capabilities)
        .map_err(|err| RociError::InvalidState(err.to_string()))?;

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

fn normalize_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
}
```

If `base64` or `chrono` imports differ in crate scope, follow existing imports used by `crates/roci-cli/src/chat.rs`.

- [ ] **Step 5: Export compiler API**

In `attachments/mod.rs`, add:

```rust
mod compiler;
pub use compiler::{
    compile_prompt_input, AttachmentContentKind, AttachmentDisplayMetadata, AttachmentSourceKind,
    CompiledPromptInput,
};
```

In `prelude.rs`, re-export the same public symbols.

- [ ] **Step 6: Run core attachment tests**

Before running tests, update all direct `ModelMessage` literals:

```bash
rg -n "ModelMessage \\{" crates/roci-core/src crates/roci-cli/src
```

Rules:

- for new synthetic/test/provider-created messages, add `metadata: None`;
- for transforms that preserve a message, clone existing metadata;
- for compaction/truncation helpers, preserve `metadata: message.metadata.clone()`;
- do not drop metadata unless the function intentionally creates a different semantic message.

Run:

```bash
cargo test -p roci-core attachments --features agent
```

Expected: pass.

---

## Task 2: Runtime API Wiring

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/lifecycle.rs`
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/chat_runtime.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/queue_and_continue.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/state_lifecycle.rs`

- [ ] **Step 1: Add runtime tests first**

Add tests using existing test provider helpers:

```rust
#[tokio::test]
async fn prompt_input_preserves_attachment_metadata_in_chat_snapshot() {
    let agent = runtime_with_chat_provider();
    let input = PromptInput::new("Inspect")
        .with_attachment(Attachment::Selection(
            SelectionAttachment::new("selected text").with_name("Selection A"),
        ));

    agent.prompt(input).await.expect("prompt should run");

    let thread = agent.read_snapshot().await.threads[0].clone();
    let user = thread
        .messages
        .iter()
        .find(|message| message.payload.role == Role::User)
        .expect("user message");
    let metadata = user.payload.metadata.as_ref().expect("metadata");
    assert_eq!(metadata.attachments.len(), 1);
    assert_eq!(metadata.attachments[0].source_kind, AttachmentSourceKind::Selection);
    assert_eq!(metadata.attachments[0].name.as_deref(), Some("Selection A"));
}

#[tokio::test]
async fn continue_run_accepts_prompt_input_attachments() {
    let agent = runtime_with_chat_provider();
    agent.prompt("hello").await.expect("first run");

    let input = PromptInput::new("Continue")
        .with_attachment(Attachment::Selection(SelectionAttachment::new("extra context")));
    agent.continue_run(input).await.expect("continue should run");

    let user_messages = agent
        .messages()
        .await
        .into_iter()
        .filter(|message| message.role == Role::User)
        .collect::<Vec<_>>();
    assert!(user_messages.last().expect("last user").text().contains("extra context"));
}

#[tokio::test]
async fn steer_and_follow_up_accept_prompt_input_attachments() {
    let agent = AgentRuntime::new(test_registry(), test_config(), test_agent_config());

    agent
        .steer(PromptInput::new("steer").with_attachment(Attachment::Selection(
            SelectionAttachment::new("s"),
        )))
        .await
        .expect("steer should queue");
    agent
        .follow_up(PromptInput::new("follow").with_attachment(Attachment::Selection(
            SelectionAttachment::new("f"),
        )))
        .await
        .expect("follow-up should queue");

    assert!(agent.steering_queue.lock().await[0].text().contains("s"));
    assert!(agent.follow_up_queue.lock().await[0].text().contains("f"));
}
```

Add a failure atomicity test:

```rust
#[tokio::test]
async fn prompt_input_preflight_failure_does_not_mutate_messages_or_chat() {
    let agent = runtime_with_chat_provider();
    let input = PromptInput::new("Describe")
        .with_attachment(Attachment::Blob(
            BlobAttachment::new([137, 80, 78, 71]).with_mime_type("image/png"),
        ));

    let err = agent.prompt(input).await.expect_err("image should fail");

    assert!(err.to_string().contains("model does not support image attachments"));
    assert!(agent.messages().await.is_empty());
    let thread = agent.read_snapshot().await.threads[0].clone();
    assert!(thread.messages.is_empty());
    assert!(thread.turns.is_empty());
}
```

Add a runtime positive image path using the full-message recording provider and vision capabilities:

```rust
#[tokio::test]
async fn prompt_input_sends_image_parts_in_provider_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FullMessageRecordingFactory {
        provider_key: "stub",
        requests: requests.clone(),
    }));
    let mut config = test_agent_config();
    config.model = "stub:vision".parse().expect("stub model should parse");
    let agent = AgentRuntime::new(Arc::new(registry), test_config(), config);
    let input = PromptInput::new("describe image").with_attachment(Attachment::Blob(
        BlobAttachment::new([137, 80, 78, 71]).with_mime_type("image/png"),
    ));

    agent.prompt(input).await.expect("vision prompt should run");

    let recorded = requests.lock().expect("requests lock");
    let user_message = recorded[0]
        .iter()
        .find(|message| message.role == Role::User)
        .expect("provider request should include user message");
    assert!(matches!(&user_message.content[1], ContentPart::Image(image) if image.mime_type == "image/png"));
}
```

If the existing `FullMessageRecordingFactory` does not expose vision caps, extend only that test helper so `provider.capabilities()` returns `ModelInputCapabilities::from_vision_support(true)` for model id `vision`.

- [ ] **Step 2: Run failing runtime tests**

Run:

```bash
cargo test -p roci-core runtime_tests::chat_runtime::prompt_input_preserves_attachment_metadata_in_chat_snapshot --features agent
cargo test -p roci-core runtime_tests::queue_and_continue::steer_and_follow_up_accept_prompt_input_attachments --features agent
```

Expected: fail because runtime APIs are text-only or return `()`.

- [ ] **Step 3: Add `PromptInput` conversions**

In `attachments/types.rs`, add:

```rust
impl From<String> for PromptInput {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for PromptInput {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}
```

- [ ] **Step 4: Add runtime compile helper**

In `lifecycle.rs`, import:

```rust
use crate::attachments::{compile_prompt_input, PromptInput};
```

Add:

```rust
async fn compile_user_prompt_with_model(
    &self,
    input: impl Into<PromptInput>,
    model: &crate::models::LanguageModel,
) -> Result<ModelMessage, RociError> {
    let provider = self
        .registry
        .create_provider(model.provider_name(), model.model_id(), &self.roci_config)?;
    Ok(compile_prompt_input(&input.into(), provider.capabilities())?.message)
}
```

For prompt/continue, call `transition_to_running()` before compile, clone `model` immediately after transition, and restore idle on compile error. This avoids busy calls reading files or racing model capability changes.

For steer/follow-up, compile before enqueue because those APIs remain legal while idle/running and do not mutate run state.

- [ ] **Step 5: Update public runtime APIs**

Change signatures:

```rust
pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<RunResult, RociError>
pub async fn continue_run(&self, input: impl Into<PromptInput>) -> Result<RunResult, RociError>
pub async fn steer(&self, input: impl Into<PromptInput>) -> Result<(), RociError>
pub async fn follow_up(&self, input: impl Into<PromptInput>) -> Result<(), RociError>
```

Implementation:

```rust
pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<RunResult, RociError> {
    self.transition_to_running()?;
    let model = self.model.lock().await.clone();
    let user_message = match self.compile_user_prompt_with_model(input, &model).await {
        Ok(message) => message,
        Err(err) => {
            self.restore_idle_after_preflight_error().await;
            return Err(err);
        }
    };
    self.prompt_user_message(user_message).await
}

pub async fn continue_run(&self, input: impl Into<PromptInput>) -> Result<RunResult, RociError> {
    self.transition_to_running()?;
    let model = self.model.lock().await.clone();
    let user_message = match self.compile_user_prompt_with_model(input, &model).await {
        Ok(message) => message,
        Err(err) => {
            self.restore_idle_after_preflight_error().await;
            return Err(err);
        }
    };
    self.continue_with_user_message(user_message).await
}

pub async fn steer(&self, input: impl Into<PromptInput>) -> Result<(), RociError> {
    let model = self.model.lock().await.clone();
    let user_message = self.compile_user_prompt_with_model(input, &model).await?;
    self.steering_queue.lock().await.push(user_message);
    Ok(())
}

pub async fn follow_up(&self, input: impl Into<PromptInput>) -> Result<(), RociError> {
    let model = self.model.lock().await.clone();
    let user_message = self.compile_user_prompt_with_model(input, &model).await?;
    self.follow_up_queue.lock().await.push(user_message);
    Ok(())
}
```

Extract old `continue_run` body into:

```rust
async fn continue_with_user_message(
    &self,
    user_message: ModelMessage,
) -> Result<RunResult, RociError> { ... }
```

Also adjust `prompt_user_message` so it assumes state is already running. Do not call `transition_to_running()` twice.

Add transactional chat queue rollback:

```rust
async fn queue_chat_turn(&self, messages: Vec<ModelMessage>) -> Result<TurnId, RociError> {
    let (turn_id, events, previous_projector) = {
        let mut projector = self
            .chat_projector
            .lock()
            .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?;
        let previous_projector = projector.clone();
        let projection = projector.queue_turn(messages);
        (projection.turn_id, projection.events, previous_projector)
    };
    if let Err(err) = self.publish_runtime_events(events).await {
        if let Ok(mut projector) = self.chat_projector.lock() {
            *projector = previous_projector;
        }
        return Err(Self::map_chat_projection_error(err));
    }
    Ok(turn_id)
}
```

Add a unit test with a failing `AgentRuntimeEventStore` that proves queue/publish failure leaves messages and chat turns unchanged.

- [ ] **Step 6: Fix call sites for fallible steer/follow_up**

Update tests and runtime call sites from:

```rust
agent.steer("s1").await;
agent.follow_up("f1").await;
```

to:

```rust
agent.steer("s1").await.expect("steer should queue");
agent.follow_up("f1").await.expect("follow-up should queue");
```

Use meaningful expectations in tests only.

- [ ] **Step 7: Run runtime tests**

Run:

```bash
cargo test -p roci-core runtime_tests --features agent
```

Expected: pass.

---

## Task 3: CLI Runtime-Owned Attachment Path

**Files:**
- Modify: `crates/roci-cli/src/chat.rs`

- [ ] **Step 1: Replace CLI compile tests**

Remove tests that assert private CLI `prompt_message_from_resolved` behavior. Keep registry and preflight-oriented behavior only if it still applies through core tests.

Add a small unit test for path-to-input construction if helper exists:

```rust
#[test]
fn build_prompt_input_preserves_attachment_paths() {
    let input = build_prompt_input(
        "Inspect".to_string(),
        &[PathBuf::from("/tmp/notes.txt"), PathBuf::from("/tmp/image.png")],
    );

    assert_eq!(input.text, "Inspect");
    assert_eq!(input.attachments.len(), 2);
}
```

Add a CLI runtime-owned path test by extracting a provider factory seam:

```rust
#[test]
fn build_prompt_input_preserves_attachment_paths() {
    let input = build_prompt_input(
        "Inspect".to_string(),
        &[PathBuf::from("/tmp/notes.txt"), PathBuf::from("/tmp/image.png")],
    );

    assert_eq!(input.text, "Inspect");
    assert_eq!(input.attachments.len(), 2);
    assert!(matches!(&input.attachments[0], Attachment::File(file) if file.path == PathBuf::from("/tmp/notes.txt")));
}
```

Runtime-owned preflight is covered by core/runtime tests. CLI live verification in Task 5 is required evidence that `--attach` uses current runtime path end to end.

- [ ] **Step 2: Add helper**

In `chat.rs`, add:

```rust
fn build_prompt_input(prompt: String, attachment_paths: &[PathBuf]) -> PromptInput {
    PromptInput::new(prompt).with_attachments(
        attachment_paths
            .iter()
            .cloned()
            .map(Attachment::file),
    )
}
```

- [ ] **Step 3: Remove CLI-local provider/preflight/message compile**

Delete CLI imports no longer needed:

```rust
base64::prelude::{Engine as _, BASE64_STANDARD}
preflight_resolved_attachments
render_prompt_input_text
AttachmentResolveOptions
AttachmentResolver
DefaultAttachmentResolver
ResolvedAttachment
ModelCapabilities
ContentPart
ImageContent
ModelMessage
Role
```

Remove:

```rust
build_attachment_prompt_message
prompt_message_from_resolved
normalize_mime_type
```

- [ ] **Step 4: Call runtime prompt with PromptInput**

Replace:

```rust
let attachment_message = if attachments.is_empty() { ... };
...
let result = if let Some(message) = attachment_message {
    agent.prompt_message(message).await
} else {
    agent.prompt(prompt).await
};
```

with:

```rust
let prompt_input = build_prompt_input(prompt, &attachments);
...
let result = agent.prompt(prompt_input).await;
```

Do not create provider in CLI solely for attachment preflight.

- [ ] **Step 5: Run CLI tests**

Run:

```bash
cargo test -p roci-cli --all-targets --features roci/lmstudio chat
```

Expected: pass.

---

## Task 4: Docs and API Contract

**Files:**
- Modify: `docs/agent-runtime-chat.md`
- Modify: `docs/testing.md`
- Modify: `crates/roci-core/src/agent/runtime.rs`

- [ ] **Step 1: Update runtime docs**

In `docs/agent-runtime-chat.md`, add to public APIs:

```markdown
- `prompt(input: impl Into<PromptInput>) -> Result<RunResult, RociError>` (async)
  - Resolves and preflights attachments before mutating runtime state.
  - Stores sanitized attachment metadata on user `ModelMessage` payloads.
- `continue_run(input: impl Into<PromptInput>) -> Result<RunResult, RociError>` (async)
  - Same input handling as `prompt`, without prepending system prompt.
- `steer(input: impl Into<PromptInput>) -> Result<(), RociError>` (async)
  - Queues a compiled user message for the next steering drain.
- `follow_up(input: impl Into<PromptInput>) -> Result<(), RociError>` (async)
  - Queues a compiled user message for post-completion continuation.
```

Add metadata contract:

```markdown
Attachment metadata in chat snapshots is sanitized. It includes source kind,
content kind, display name, MIME type, and byte size. It does not persist raw
local file paths or attachment bytes.
```

- [ ] **Step 2: Update testing doc if needed**

In `docs/testing.md`, ensure attachment live verification mentions:

```markdown
For attachment changes, verify through `roci-agent chat --attach ...`, not only
core unit tests. Session-enabled checks must inspect persisted session files and
confirm message metadata does not contain raw host file paths.
```

- [ ] **Step 3: Update rustdoc**

In `crates/roci-core/src/agent/runtime.rs`, update top-level API list:

```rust
//! - [`Agent::prompt`] — start a new conversation from text or `PromptInput`
//! - [`Agent::continue_run`] — continue with text or `PromptInput`
//! - [`Agent::steer`] — queue fallible steering input
//! - [`Agent::follow_up`] — queue fallible follow-up input
```

- [ ] **Step 4: Run docs-adjacent checks**

Run:

```bash
cargo check -p roci-core --features agent
```

Expected: pass.

---

## Task 5: Integration, Automated Verification, Live Verification

**Files:**
- No primary ownership; integration owner checks full diff.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --check
```

If it fails, run `cargo fmt`, then rerun `cargo fmt --check`.

- [ ] **Step 2: Whitespace diff check**

Run:

```bash
git diff --check
```

Expected: no whitespace errors.

- [ ] **Step 3: Workspace compile/test**

Run:

```bash
cargo check --workspace --all-targets
cargo test --workspace --all-targets
```

Expected: both pass. If unrelated pre-existing lints/tests fail, record exact failures and still run targeted checks below.

- [ ] **Step 4: Targeted clippy**

Run:

```bash
cargo clippy -p roci-core --lib --features agent -- -D warnings
cargo clippy -p roci-cli --all-targets --features roci/lmstudio -- -D warnings
```

Expected: both pass.

- [ ] **Step 5: Live CLI text attachment verification**

Create a temp attachment outside repo:

```bash
mkdir -p /tmp/roci-attach-live
printf 'roci-runtime-promptinput-live-marker-5821\n' > /tmp/roci-attach-live/notes.txt
```

Run in tmux and show attach command:

```bash
curl -s http://127.0.0.1:1234/api/v0/models | head
tmux new-session -d -s roci-runtime-promptinput-live \
  'cd /Users/adityasharma/Projects/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model lmstudio:local-model --temperature 0 --max-tokens 64 \
   --attach /tmp/roci-attach-live/notes.txt \
   "Repeat the marker from the attachment exactly"; \
   status=$?; printf "\n[roci runtime promptinput text exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-runtime-promptinput-live"
```

Expected: model response includes `roci-runtime-promptinput-live-marker-5821` or equivalent evidence that attachment text reached model. Record exit code.

- [ ] **Step 6: Live CLI image preflight verification**

Create a tiny PNG-like file:

```bash
printf '\211PNG\r\n\032\n' > /tmp/roci-attach-live/image.png
```

Run:

```bash
tmux new-session -d -s roci-runtime-promptinput-image \
  'cd /Users/adityasharma/Projects/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model lmstudio:local-model --temperature 0 --max-tokens 64 \
   --attach /tmp/roci-attach-live/image.png \
   "Describe this image"; \
   status=$?; printf "\n[roci runtime promptinput image exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-runtime-promptinput-image"
```

Expected for non-vision local model: command exits nonzero with `model does not support image attachments` before provider request.

- [ ] **Step 7: Session metadata verification**

Run session-enabled text attachment:

```bash
SESSION_ROOT="$(mktemp -d /tmp/roci-session-root.XXXXXX)"
cargo run -p roci-cli --features roci/lmstudio -- chat \
  --model lmstudio:local-model \
  --session-root "$SESSION_ROOT" \
  --session-id runtime-promptinput-live \
  --attach /tmp/roci-attach-live/notes.txt \
  "Repeat the marker from the attachment exactly"
rg -n "/tmp/roci-attach-live|notes.txt|source_kind|attachments" "$SESSION_ROOT"
```

Expected:

- `notes.txt`, `source_kind`, or `attachments` may appear.
- `/tmp/roci-attach-live` must not appear in persisted files.

- [ ] **Step 8: Close tasque task**

After all required verification passes:

```bash
tsq close tsq-r0c1att8.3 --note "Implemented runtime-owned PromptInput compile path for prompt/continue/steer/follow-up with sanitized chat metadata and CLI --attach wiring. Verified with automated tests and live tmux CLI attachment checks."
```

Do not close task before live verification passes.
