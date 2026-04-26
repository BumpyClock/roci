# Task Spec: OpenAI TTS SpeechProvider

## Deliverables
- Concrete `SpeechProvider` implementation using OpenAI TTS API.
- Format/voice/speed option mapping.
- Binary audio response handling and error mapping.

## Acceptance Criteria
- Valid request returns non-empty bytes in expected format.
- Invalid options/provider failures map to stable `RociError` values.

## Tests
- Wiremock happy path.
- Error status + malformed body tests.
