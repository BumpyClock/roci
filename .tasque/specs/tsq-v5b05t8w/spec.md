## Plan
- Enable the roci-cli crate to compile with roci audio feature support.
- Add only two honest CLI entry points now: file transcription and text-to-speech file output.
- Defer realtime CLI until roci-core exposes a public send/input API beyond connect/bootstrap/read/close.

## Acceptance
- roci-cli builds with roci audio feature enabled.
- CLI has stable subcommands for transcribe and speak with explicit file IO.
- Tests cover clap parsing and one end-to-end path per command with mocked OpenAI HTTP APIs.
- Realtime is either absent from help output or clearly marked unsupported.