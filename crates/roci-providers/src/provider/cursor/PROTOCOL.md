# Cursor native protocol

`agent_descriptor.bin` is the serialized `FileDescriptorProto` embedded as
`agentDescriptorB64` in CLIProxyAPIPlus's
`internal/auth/cursor/proto/descriptor.go`. That source attributes the descriptor
to `alma-plugins/plugins/cursor-auth/proto/agent_pb.ts`. It describes Cursor's
`agent.v1` protocol and is retained as data so field shapes are checked rather
than guessed. Source checkout: `~/Projects/references/CLIProxyAPIPlus`.

The transport uses HTTP/2 and Connect envelopes, with bidirectional blob/context
replies. With `ProviderRequest.session_id`, completed upstream checkpoints and
their blob state persist under `~/.roci/cursor/sessions`. Hosts can inject a
`CursorSessionStore`; no session ID means an ephemeral request. State keys hash
account identity, model, endpoint, and SDK session ID. Unix directory/file modes
are 0700/0600, commits use a synced temporary file and atomic rename, and an
exclusive file lock spans the request/response lifetime. Concurrent processes
using the same session fail instead of overwriting each other's state.

A checkpoint is compatible when the saved input history still matches, the
previous assistant text matches, and the next message is a new text user turn.
The provider sends that latest message plus the upstream checkpoint; it retains
the checkpoint's conversation ID and serves its persisted blobs. Edits,
compaction, model/account changes, interrupted turns, and missing checkpoints
fall back to a new conversation containing the complete SDK history. A failed
state read/write produces an error; it does not silently discard durable state.

Tool calls return to the SDK. Cursor's execution IDs address callbacks on the
live HTTP/2 stream. The reference resumes tool execution by writing an MCP
result on that same parked stream. `ResumeAction` contains request context but
has no field for supplying tool results on a newly opened stream; no verified
protocol rebinding of execution IDs across streams is available. Consequently,
SDK tool-result continuation uses the complete-history fallback, retaining the
exact call ID, name, arguments, result, and error flag. If Cursor reissues an
already completed call ID with identical arguments, the provider replies with
the stored SDK result instead of executing the tool again. Changed arguments
under the same completed ID are rejected. Checkpoints received around tool
boundaries are saved but marked incompatible with a new text action.

Upstream filesystem, shell, and network execution requests are rejected:
execution belongs to the host's SDK tools. Unsupported interaction requests,
images, structured output, and sampling settings fail explicitly. Connect
compression is not requested and compressed frames are rejected. Frames and
blob storage are bounded. No live credential-bearing connection survives a
request, and credential tokens are not included in the persisted state envelope.
