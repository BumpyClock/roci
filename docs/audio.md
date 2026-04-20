---
summary: "Audio CLI support in roci-agent"
read_when: "Working on roci-cli audio commands, OpenAI audio wiring, or audio feature flags"
---

# Audio CLI Support

`roci-agent` exposes file-based audio commands behind the `audio` feature.

Current surface:

- `roci-agent audio transcribe`
- `roci-agent audio speak`

## Scope

This CLI wiring is intentionally **file-based only**.

- Supported now:
  - OpenAI transcription via `/audio/transcriptions`
  - OpenAI text-to-speech via `/audio/speech`
- Not exposed yet:
  - realtime audio sessions
  - microphone capture
  - speaker playback
  - conversational audio turns

`roci-core` has a realtime WebSocket session type, but it does not yet expose a
public client-send API for audio/text turns. The CLI should not promise
realtime audio chat until that exists.

## Credentials and config

The audio commands use the OpenAI provider config already loaded by
`RociConfig::from_env()`:

| Variable | Purpose |
|---|---|
| `OPENAI_API_KEY` | Required for both transcription and TTS |
| `OPENAI_BASE_URL` | Optional override for OpenAI-compatible testing/proxies |

## Commands

### Transcribe audio

```bash
roci-agent audio transcribe --input sample.wav
roci-agent audio transcribe --input sample.mp3 --language en --json
cat sample.wav | roci-agent audio transcribe --input - --mime-type audio/wav
```

Behavior:

- Reads audio bytes from `--input`
- `--input -` reads from stdin
- Infers MIME type from file extension when possible
- `--mime-type` overrides inference and is required for stdin
- Prints transcript text to stdout by default
- `--json` prints the full `TranscriptionResult`

Supported inferred extensions:

- `.mp3` -> `audio/mpeg`
- `.mp4`, `.m4a` -> `audio/mp4`
- `.wav`, `.wave` -> `audio/wav`
- `.webm` -> `audio/webm`
- `.ogg`, `.oga` -> `audio/ogg`
- `.flac` -> `audio/flac`

### Generate speech

```bash
roci-agent audio speak --output out.mp3 "hello world"
roci-agent audio speak --output out.wav --voice nova --format wav "read this aloud"
roci-agent audio speak --output - --format mp3 "stream bytes to stdout" > out.mp3
```

Behavior:

- Sends text to OpenAI TTS
- Writes binary audio to `--output`
- `--output -` writes raw audio bytes to stdout
- When `--output` is a file, prints the written path to stdout
- Default voice: `alloy`
- Default format: `mp3`
- Default model: `tts-1`

## Validation

Recommended checks when touching this area:

```bash
cargo test -p roci-cli parse_audio_
cargo test -p roci-cli audio_cmd::tests::
```

The CLI also has mocked handler tests that exercise the actual OpenAI audio HTTP
paths end-to-end without requiring real network calls.
