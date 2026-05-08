# Provider Attachment Payload Assertions

## Task

`tsq-r0c1att8.4` adds provider payload assertions for attachment-derived model content.

## Context

Roci attachment V1 has two provider-visible content shapes:

- Text attachments render as bounded `ContentPart::Text`.
- Image attachments render as existing `ContentPart::Image`.

V1 does not add `ContentPart::File` or provider-native file upload. Native files remain future provider-specific work.

Pi and Codex use the same broad architecture: app-facing inputs can mention files/images, but provider-facing payloads stay normalized to text and image parts. Both also use tolerant text markers when image/media content cannot be sent.

## Design

Provider tests should assert exact JSON payload shape for current serializers:

- OpenAI Chat maps text to `{ "type": "text" }` and images to `{ "type": "image_url", "image_url": { "url": "data:<mime>;base64,<data>" } }`.
- OpenAI Responses maps text to `{ "type": "input_text" }` and images to `{ "type": "input_image", "image_url": "data:<mime>;base64,<data>" }`.
- Anthropic maps text to `{ "type": "text" }` and images to `{ "type": "image", "source": { "type": "base64", "media_type": <mime>, "data": <data> } }`.
- Google maps text to `{ "text": ... }` and images to `{ "inlineData": { "mimeType": <mime>, "data": <data> } }`.

Unsupported media should degrade into model-visible bounded text rather than silently dropping content or uploading opaque bytes:

```text
User attached unsupported media: <name> (<mime>, <size> bytes). Content omitted.
```

This marker lets the agent explain the limitation to the user. It preserves app responsibility for first-class UX while keeping SDK behavior tolerant.

This includes image attachments when the selected model cannot accept images, or when the image MIME type is unsupported by that model. Oversized images and count-limit violations still fail because they are resource-limit errors, not representable content-limitations.

## Error Handling

Unsupported media fallback is allowed only after safe metadata extraction. The compiler must still error for unsafe or unbounded cases:

- unreadable attachment path
- attachment count limit exceeded
- byte size limit exceeded
- image count limit exceeded
- image byte limit exceeded
- malformed blob metadata that cannot produce a bounded marker

Metadata stored in session/chat history remains sanitized. Raw host paths must not be persisted.

## Tests

Add focused provider tests under `roci-providers` for the four provider families. Tests should construct `ProviderRequest` values with text, image, and fallback marker content, then inspect request JSON without making network calls.

Run targeted command:

```bash
cargo test -p roci-providers provider_attachment_payload
```

Full verification remains in `.6`: fmt, clippy, workspace tests, live tmux
text plus unsupported-media smoke, and vision smoke when a vision-capable
provider/model is loaded or configured.

## Acceptance

- Provider payload assertions prove text and image shapes for OpenAI Chat, OpenAI Responses, Anthropic, and Google.
- Tests prove unsupported-media marker reaches each provider as text.
- Tests prove image attachments for non-vision models become unsupported-media marker text instead of failing.
- No native file provider payload exists in V1.
- No raw host path appears in persisted metadata or provider payload fallback text.
