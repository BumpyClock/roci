## Overview
Add provider payload assertions for attachment-derived model content. V1 stays text/image only: text attachments render as bounded `ContentPart::Text`, image attachments render as existing `ContentPart::Image`, and no native `ContentPart::File` or provider file upload is introduced.

## Provider Payload Assertions
- OpenAI Chat: text -> `{ "type": "text" }`; image -> `{ "type": "image_url", "image_url": { "url": "data:<mime>;base64,<data>" } }`.
- OpenAI Responses: text -> `{ "type": "input_text" }`; image -> `{ "type": "input_image", "image_url": "data:<mime>;base64,<data>" }`.
- Anthropic: text -> `{ "type": "text" }`; image -> `{ "type": "image", "source": { "type": "base64", "media_type": <mime>, "data": <data> } }`.
- Google: text -> `{ "text": ... }`; image -> `{ "inlineData": { "mimeType": <mime>, "data": <data> } }`.

## Unsupported Media Fallback
Unsupported media should degrade into bounded model-visible text instead of silent drop, opaque upload, or hard preflight error when safe metadata can be extracted:

`User attached unsupported media: <name> (<mime>, <size> bytes). Content omitted.`

This lets the agent communicate the limitation to the user while preserving app responsibility for richer UX.

## Errors That Still Fail
- unreadable attachment path
- attachment count limit exceeded
- byte size limit exceeded
- malformed blob metadata that cannot produce a bounded marker

## Acceptance
- Provider JSON assertions prove text and image shapes for OpenAI Chat, OpenAI Responses, Anthropic, and Google.
- Tests prove unsupported-media fallback text reaches each provider as normal text.
- No native file provider payload exists in V1.
- Raw host paths do not appear in fallback text, persisted metadata, or provider payloads.

## Test Plan
- `cargo test -p roci-providers provider_attachment_payload`
- Full fmt/clippy/workspace/live verification remains in `tsq-r0c1att8.6`.
