## Overview
Add provider payload assertions for attachment-derived model content. V1 stays text/image only: text attachments render as bounded `ContentPart::Text`, image attachments render as existing `ContentPart::Image`, and no native `ContentPart::File` or provider file upload is introduced.

## Provider Payload Assertions
Assert exact current serializer behavior:
- OpenAI Chat: single text may serialize as string; multipart text+image maps text -> `{ "type": "text" }`; image -> `{ "type": "image_url", "image_url": { "url": "data:<mime>;base64,<data>" } }`.
- OpenAI Responses: single text may serialize as string; multipart text+image maps text -> `{ "type": "input_text" }`; image -> `{ "type": "input_image", "image_url": "data:<mime>;base64,<data>" }`.
- Anthropic: single text serializes as string; multipart text+image maps text -> `{ "type": "text" }`; image -> `{ "type": "image", "source": { "type": "base64", "media_type": <mime>, "data": <data> } }`.
- Google: text -> `{ "text": ... }`; image -> `{ "inlineData": { "mimeType": <mime>, "data": <data> } }`.

## Unsupported Media Fallback
Unsupported media should degrade into bounded model-visible text instead of silent drop, opaque upload, or hard preflight error when safe metadata can be extracted:

`User attached unsupported media: <name> (<mime>, <size> bytes). Content omitted.`

`<name>` is explicit display name or basename only; never raw absolute path. This fallback also applies when a selected model cannot accept images or cannot accept the image MIME type. Oversized images/count-limit violations still fail because they are resource-limit errors.

## Errors That Still Fail
- unreadable attachment path
- attachment count limit exceeded
- byte size limit exceeded
- image count limit exceeded
- image byte limit exceeded
- malformed blob metadata that cannot produce a bounded marker

## Acceptance
- Provider JSON assertions prove text and image shapes for OpenAI Chat, OpenAI Responses, Anthropic, and Google.
- Tests prove unsupported-media fallback text reaches each provider as normal text.
- Tests prove image attachments for non-vision models become unsupported-media marker text instead of failing.
- No native file provider payload exists in V1; tests assert no `file`, `file_id`, `input_file`, or `document` payload for attachments.
- Raw host paths do not appear in fallback text, persisted metadata, or provider payloads.

## Test Plan
- `cargo test -p roci-core --features agent unsupported_media`
- `cargo test -p roci-providers provider_attachment_payload`
- Live tmux smoke with current `roci-agent` binary and unsupported attachment proves marker reaches model/provider payload.
- Full fmt/clippy/workspace/live verification remains in `tsq-r0c1att8.6`.
