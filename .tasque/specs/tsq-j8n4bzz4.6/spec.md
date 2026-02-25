# Task Spec: OpenAI Whisper AudioProvider

## Deliverables
- Concrete `AudioProvider` implementation for Whisper transcription.
- Multipart upload with language hint/options.
- Robust error mapping and timeout/retry behavior.

## Acceptance Criteria
- Returns `TranscriptionResult` for valid audio payload.
- Handles invalid mime/content and provider errors.
- No panics on malformed responses.

## Tests
- Wiremock happy path and common error paths.
- Input validation unit tests.
