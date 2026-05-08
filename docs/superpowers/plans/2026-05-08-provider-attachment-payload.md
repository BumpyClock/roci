# Provider Attachment Payload Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove attachment-derived text/image provider payloads and make unsupported media degrade into a bounded model-visible marker.

**Architecture:** Keep V1 provider-facing content normalized to `ContentPart::Text` and `ContentPart::Image`. Add unsupported media fallback in the attachment compiler/preflight seam, before provider serialization. Add provider tests around existing request builders; do not add native file payloads or network calls.

**Tech Stack:** Rust, roci-core attachments, roci-providers request builders, cargo test, clippy, rustfmt, tmux/live roci-cli verification.

---

## File Structure

- Modify `crates/roci-core/src/attachments/resolver.rs`: convert unsupported non-text/non-image resolved bytes into bounded marker text when size/count checks passed.
- Modify `crates/roci-core/src/attachments/compiler.rs`: convert image attachments that current model cannot accept into bounded marker text rather than erroring on content support.
- Modify `crates/roci-core/src/attachments/tests.rs`: replace unsupported-media hard-fail tests with marker fallback tests and raw-path assertions.
- Modify `crates/roci-providers/src/provider/openai.rs`: add OpenAI Chat payload assertion tests.
- Modify `crates/roci-providers/src/provider/openai_responses/mod.rs`: add OpenAI Responses payload assertion tests.
- Modify `crates/roci-providers/src/provider/anthropic.rs`: add Anthropic payload assertion tests.
- Modify `crates/roci-providers/src/provider/google.rs`: add Google payload assertion tests.
- No CLI code change expected. Use existing `roci-agent chat --attach` live path to prove unsupported media marker behavior.
- Multi-agent ownership: Task 1 owns core attachment files and must land first. Tasks 2 and 3 can run in parallel after checking `git status --short`; they own disjoint provider files. Task 4 owns final integration/verification only.

---

## Task 1: Core Unsupported Media Fallback

**Files:**
- Modify: `crates/roci-core/src/attachments/resolver.rs`
- Modify: `crates/roci-core/src/attachments/compiler.rs`
- Modify: `crates/roci-core/src/attachments/tests.rs`

- [ ] **Step 1: Update failing tests for fallback behavior**

In `crates/roci-core/src/attachments/tests.rs`, replace `opaque_blob_fails_preflight` and `non_text_non_image_mime_fails_even_when_utf8` with tests shaped like:

```rust
#[test]
fn opaque_blob_resolves_to_unsupported_media_marker() {
    let blob = BlobAttachment::new([0, 159, 146, 150]).with_name("opaque.bin");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("opaque blob should resolve as marker text");

    let text = resolved[0].as_text().expect("marker should be text");
    assert!(text.contains("User attached unsupported media: opaque.bin"));
    assert!(text.contains("application/octet-stream"));
    assert!(text.contains("4 bytes"));
    assert!(text.contains("Content omitted."));
}

#[test]
fn non_text_non_image_mime_resolves_to_unsupported_media_marker() {
    let blob = BlobAttachment::new("pdf-ish")
        .with_name("doc.pdf")
        .with_mime_type("application/pdf");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("unsupported MIME should resolve as marker text");

    let text = resolved[0].as_text().expect("marker should be text");
    assert_eq!(
        text,
        "User attached unsupported media: doc.pdf (application/pdf, 7 bytes). Content omitted."
    );
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("application/pdf")
    );
}
```

Add a file-path privacy test:

```rust
#[test]
fn unsupported_file_marker_uses_file_name_not_raw_path() {
    let dir = tempdir().expect("tempdir should be created");
    let path = dir.path().join("private.pdf");
    fs::write(&path, [0, 159, 146, 150]).expect("fixture should be written");
    let input = PromptInput::new("Inspect").with_attachment(Attachment::file(&path));

    let compiled =
        compile_prompt_input(&input, &ModelCapabilities::default()).expect("marker should compile");

    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: private.pdf"));
    assert!(!text.contains(dir.path().to_string_lossy().as_ref()));
}
```

Add non-vision image fallback test:

```rust
#[test]
fn unsupported_media_image_converts_to_marker_for_non_vision_model() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([137, 80, 78, 71])
            .with_name("pixel.png")
            .with_mime_type("image/png"),
    ));

    let compiled =
        compile_prompt_input(&input, &ModelCapabilities::default()).expect("marker should compile");

    assert_eq!(compiled.message.content.len(), 1);
    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: pixel.png"));
    assert!(text.contains("image/png"));
    assert!(text.contains("Content omitted."));
    let metadata = compiled.message.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.attachments[0].content_kind,
        AttachmentContentKind::Text
    );
}
```

Add unsupported image MIME and resource-limit tests:

```rust
#[test]
fn unsupported_media_image_mime_converts_to_marker_for_vision_model() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([71, 73, 70, 56])
            .with_name("motion.gif")
            .with_mime_type("image/gif"),
    ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities::from_vision_support(true),
        ..ModelCapabilities::default()
    };

    let compiled = compile_prompt_input(&input, &caps).expect("marker should compile");

    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: motion.gif"));
    assert!(text.contains("image/gif"));
}

#[test]
fn unsupported_media_oversized_image_still_errors() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([71, 73, 70, 56])
            .with_name("large.gif")
            .with_mime_type("image/gif"),
    ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities {
            image: Some(crate::models::ImageInputCapabilities {
                max_images: 8,
                max_image_bytes: Some(2),
                max_total_image_bytes: Some(10),
                supported_mime_types: vec!["image/png".to_string()],
                image_token_estimate: 85,
            }),
            ..ModelInputCapabilities::default()
        },
        ..ModelCapabilities::default()
    };

    let err = compile_prompt_input(&input, &caps).expect_err("oversized image should fail");

    assert!(err.to_string().contains("too large"));
}
```

- [ ] **Step 2: Run tests and verify they fail before implementation**

Run:

```bash
cargo test -p roci-core --features agent unsupported_media
```

Expected before implementation: tests fail because resolver still returns `UnsupportedMime` / `UnsupportedBinary` and compiler rejects image input for non-vision models.

- [ ] **Step 3: Implement marker fallback**

In `crates/roci-core/src/attachments/resolver.rs`, add helpers near MIME helpers:

```rust
const UNSUPPORTED_MEDIA_MIME: &str = "application/octet-stream";

fn unsupported_media_marker(name: &str, mime_type: &str, size_bytes: usize) -> String {
    format!(
        "User attached unsupported media: {name} ({mime_type}, {size_bytes} bytes). Content omitted."
    )
}

fn unsupported_media_text(
    metadata: AttachmentMetadata,
    name: &str,
    mime_type: String,
) -> ResolvedAttachment {
    ResolvedAttachment::Text {
        text: unsupported_media_marker(name, &mime_type, metadata.size_bytes),
        metadata: AttachmentMetadata {
            mime_type: Some(mime_type),
            ..metadata
        },
    }
}
```

Update `resolve_data` unsupported branches to return marker text after size/count validation:

```rust
if !allows_utf8_fallback(&mime_type) {
    return unsupported_media_text(metadata, name, Some(mime_type));
}
```

And for binary fallback:

```rust
Err(_) => {
    let mime_type = metadata.mime_type.clone();
    unsupported_media_text(metadata, name, mime_type)
}
```

Keep `InvalidUtf8` for declared text MIME because caller said bytes are text and they are malformed. `unsupported_media_text` must sanitize and bound the display name, validate and bound the MIME base type, and return `UnsupportedMime` for path-like, control-character, empty, or overlong MIME metadata.

- [ ] **Step 4: Convert only content-unsupported images at compiler/preflight seam**

In `crates/roci-core/src/attachments/compiler.rs`, classify `preflight_resolved_attachments` errors. Preserve resource-limit failures. Only downgrade `ImageUnsupported` and `ImageMimeUnsupported` into marker text, then run final text preflight.

Implementation shape:

```rust
let resolved = match preflight_resolved_attachments(&resolved, capabilities) {
    Ok(_) => resolved,
    Err(AttachmentPreflightError::ImageUnsupported | AttachmentPreflightError::ImageMimeUnsupported { .. }) => {
        let downgraded = downgrade_unsupported_images(resolved, capabilities);
        preflight_resolved_attachments(&downgraded, capabilities)
            .map_err(|err| RociError::InvalidState(err.to_string()))?;
        downgraded
    }
    Err(err) => return Err(RociError::InvalidState(err.to_string())),
};
```

Add helper:

```rust
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
    let mime_type = normalize_mime_type(
        metadata
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream"),
    );
    !image_caps
        .supported_mime_types
        .iter()
        .any(|supported| normalize_mime_type(supported) == mime_type)
}
```

`unsupported_image_marker` should produce `ResolvedAttachment::Text` using explicit display name or basename only. It must not use `AttachmentSource::File { path }.display()` in marker text. Do not downgrade image count, image byte, or image total byte violations.

- [ ] **Step 5: Verify core fallback tests pass**

Run:

```bash
cargo test -p roci-core --features agent unsupported_media
```

Expected: new unsupported-media tests pass.

---

## Task 2: OpenAI Provider Payload Assertions

**Files:**
- Modify: `crates/roci-providers/src/provider/openai.rs`
- Modify: `crates/roci-providers/src/provider/openai_responses/mod.rs`

- [ ] **Step 1: Add shared fixtures inside each test module**

Use existing private `build_request_body` tests. Add local helpers where needed:

```rust
fn attachment_message() -> ModelMessage {
    ModelMessage {
        role: Role::User,
        content: vec![
            ContentPart::Text {
                text: "Inspect attachment".to_string(),
            },
            ContentPart::Image(ImageContent {
                mime_type: "image/png".to_string(),
                data: "aW1hZ2U=".to_string(),
            }),
        ],
        name: None,
        timestamp: None,
        metadata: None,
    }
}

fn fallback_marker_message() -> ModelMessage {
    ModelMessage::user(
        "Inspect\n\n--- Attachment: doc.pdf (application/pdf) ---\n\
         User attached unsupported media: doc.pdf (application/pdf, 7 bytes). Content omitted.\n\
         --- End attachment ---",
    )
}
```

- [ ] **Step 2: Add OpenAI Chat assertions**

In `openai.rs` tests, add:

```rust
#[test]
fn provider_attachment_payload_openai_chat_maps_text_and_image_parts() {
    let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![attachment_message()],
        settings: settings(None, None, None, None, None),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(
        body["messages"][0]["content"][0]["text"],
        "Inspect attachment"
    );
    assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aW1hZ2U="
    );
    assert!(body["messages"][0]["content"][1].get("file").is_none());
    assert!(body["messages"][0]["content"][1].get("file_id").is_none());
    assert!(body["messages"][0]["content"][1].get("input_file").is_none());
}

#[test]
fn provider_attachment_payload_openai_chat_preserves_unsupported_media_marker_text() {
    let provider = OpenAiProvider::new(OpenAiModel::Gpt4o, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![fallback_marker_message()],
        settings: settings(None, None, None, None, None),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert!(body["messages"][0]["content"]
        .as_str()
        .expect("single text content")
        .contains("User attached unsupported media: doc.pdf"));
    assert!(!body["messages"][0]["content"].to_string().contains("/tmp/"));
}
```

- [ ] **Step 3: Add OpenAI Responses assertions**

In `openai_responses/mod.rs` tests, add matching tests using `OpenAiResponsesProvider::new(OpenAiModel::Gpt4o, ...)`. Assert:

```rust
assert_eq!(body["input"][0]["role"], "user");
assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
assert_eq!(body["input"][0]["content"][0]["text"], "Inspect attachment");
assert_eq!(body["input"][0]["content"][1]["type"], "input_image");
assert_eq!(
    body["input"][0]["content"][1]["image_url"],
    "data:image/png;base64,aW1hZ2U="
);
```

For fallback marker, assert `body["input"][0]["content"]` contains the marker. If OpenAI Responses collapses a single text part to a string, assert the string; if it keeps an array, assert first text part. Do not change serializer behavior only to satisfy the test. Add negative assertions that payload text does not contain `/tmp/` and image part has no `file`, `file_id`, or `input_file` key.

- [ ] **Step 4: Verify OpenAI provider tests pass**

Run:

```bash
cargo test -p roci-providers provider_attachment_payload_openai
```

Expected: all OpenAI attachment payload tests pass without network calls.

---

## Task 3: Anthropic and Google Provider Payload Assertions

**Files:**
- Modify: `crates/roci-providers/src/provider/anthropic.rs`
- Modify: `crates/roci-providers/src/provider/google.rs`

- [ ] **Step 1: Add Anthropic assertions**

In `anthropic.rs` tests, add `attachment_message()` and `fallback_marker_message()` helpers equivalent to Task 2. Add:

```rust
#[test]
fn provider_attachment_payload_anthropic_maps_text_and_image_parts() {
    let provider =
        AnthropicProvider::new(AnthropicModel::ClaudeSonnet4, "test-key".to_string(), None);
    let request = ProviderRequest {
        messages: vec![attachment_message()],
        settings: GenerationSettings::default(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["type"], "text");
    assert_eq!(body["messages"][0]["content"][0]["text"], "Inspect attachment");
    assert_eq!(body["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["type"],
        "base64"
    );
    assert_eq!(
        body["messages"][0]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(body["messages"][0]["content"][1]["source"]["data"], "aW1hZ2U=");
}
```

Add fallback marker test. Anthropic collapses single text to a string, so assert string contains marker.
Also assert the image part has no native-file keys: `file`, `file_id`, `input_file`, `document`.

- [ ] **Step 2: Add Google assertions**

In `google.rs` tests, add matching helpers and tests. Assert:

```rust
assert_eq!(body["contents"][0]["role"], "user");
assert_eq!(body["contents"][0]["parts"][0]["text"], "Inspect attachment");
assert_eq!(
    body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
    "image/png"
);
assert_eq!(
    body["contents"][0]["parts"][1]["inlineData"]["data"],
    "aW1hZ2U="
);
```

For fallback marker, assert `body["contents"][0]["parts"][0]["text"]` contains `User attached unsupported media: doc.pdf`.
Also assert payload text does not contain `/tmp/` and image part has no native-file keys: `file`, `file_id`, `input_file`, `document`.

- [ ] **Step 3: Verify Anthropic/Google provider tests pass**

```bash
cargo test -p roci-providers provider_attachment_payload_anthropic
cargo test -p roci-providers provider_attachment_payload_google
```

Expected: tests pass without network calls.

---

## Task 4: Integration Verification and Live Smoke

**Files:**
- Modify only if needed: `docs/testing.md`
- No CLI source change expected unless live smoke proves `roci-agent chat --attach` cannot exercise unsupported marker behavior.

- [ ] **Step 1: Run focused automated gates**

Run:

```bash
cargo test -p roci-core --features agent unsupported_media
cargo test -p roci-providers provider_attachment_payload
```

Expected: pass.

- [ ] **Step 2: Run formatting and lint gates**

Run:

```bash
cargo fmt --all -- --check
cargo clippy -p roci-core --features agent --all-targets -- -D warnings
cargo clippy -p roci-providers --all-targets -- -D warnings
```

Expected: pass. If `cargo fmt --check` fails, run `cargo fmt --all`, inspect diff, then rerun check.

- [ ] **Step 3: Build current roci-cli binary**

Run:

```bash
cargo build -p roci-cli --features roci/lmstudio
```

Expected: `target/debug/roci-agent` exists.

- [ ] **Step 4: Run live tmux unsupported-media attachment smoke**

Create unsupported fixture outside project cwd:

```bash
printf '\\000\\237\\222\\226' > /tmp/roci-unsupported-media.pdf
```

Start tmux:

```bash
rm -rf /tmp/roci-attach-smoke
LMSTUDIO_BASE_URL=http://127.0.0.1:1234 tmux new-session -d -s roci-attach-unsupported \
  './target/debug/roci-agent chat --no-skills --model "lmstudio:<loaded-model-id>" --session-root /tmp/roci-attach-smoke --session-id unsupported-media --attach /tmp/roci-unsupported-media.pdf "Repeat exactly any unsupported attachment notice you see."; roci_status=$?; printf "\n[roci-agent chat --attach exit=%s]\n" "$roci_status"; exec zsh'
```

Tell user attach command:

```bash
tmux attach -t roci-attach-unsupported
```

Expected: CLI sends request through current binary and model can see unsupported-media marker. Evidence requires provider response plus printed exit code `0`. If local provider at `http://127.0.0.1:1234` is unavailable or no loaded model ID is available, report that live provider verification is blocked and keep task open.

- [ ] **Step 5: Inspect session/payload privacy if session flag used**

Inspect session files and assert raw source path and session root path do not appear:

```bash
rg -n '/tmp/roci-unsupported-media.pdf|/tmp/roci-attach-smoke' /tmp/roci-attach-smoke/unsupported-media
```

Expected: exit code `1` and no matches. Marker should contain only `roci-unsupported-media.pdf`.

- [ ] **Step 6: Full workspace gate before final done**

Run:

```bash
cargo test --workspace --all-targets
```

Expected: pass. If too slow or blocked, report exact blocker and leave task open.

---

## Self-Review

- Spec coverage: provider multipart text/image JSON assertions covered in Tasks 2-3; unsupported marker covered in resolver and compiler/preflight seam in Task 1; no native file payload preserved by using `ContentPart::Text` marker only; raw path privacy covered in Task 1, provider negative assertions, and live smoke.
- Placeholder scan: no TBD/TODO/fill-in steps remain.
- Type consistency: plan uses existing `ModelMessage`, `ContentPart`, `ImageContent`, `ProviderRequest`, and private provider `build_request_body` tests.
