# Seam cleanup, second pass — 2026-09-14

Baseline: `861d297` (first cleanup, committed and pushed). The baseline full
workspace/all-features run passed 1,603 tests with four existing ignored helpers.
This pass used successive native parallel agent tasks for distinct boundaries;
task tracking switched away from Tasque as requested. Applied the programming,
rust-skills, and test-cleanup workflows.

## Scope

| Seam | Inspection and retained boundaries |
| --- | --- |
| Profile → candidates → launcher → child routing | Registry resolution, supervisor orchestration/wait, handle and routing consumers; retain authorization, cancellation, cached results, ordering and config inheritance. |
| Authentication → pending claims → credential storage | Admission, expiry, claim lifetime, manager cancellation, file/OS stores; retain security/fault injection and process locking. |
| Provider request → HTTP → response/stream | OpenAI Chat/Responses callbacks, JSON tool response shapes, Anthropic thinking, shared-client fixtures; retain compatibility decoders and public audio/realtime capabilities. |
| MCP connection → delivery → elicitation/bridge | Managed and raw transports, reconnect, pending/progress delivery, coordinator response, aggregation and tool errors; retain distinct raw transport capability and fault injection. |
| Raw events → projection → stores → subscription → CLI | Projection/replay, strict versus recovery ledgers, catalog subprocess matrix, runtime recovery and renderer lifetime; retain recovery/security and terminal ordering contracts. |
| Catalog → model selection → retry/fallback → health | Core/provider model modules, CLI listing, candidate ordering, retry matrix and health observation; retain presets and functional public selection APIs. |
| Resources/skills/attachments → request/schema | Loading and precedence, skill materialization, attachment compilation and provider normalization. |
| Tool visibility → validation/approval → execution | Catalog aliases, model-visible and executable tools, approval floors, dispatch and built-in tool contracts. |

All Rust test areas remain inventoried. This is a deeper seam review, not a claim
that every assertion or every possible runtime path was exhaustively audited.
Standalone scripts, historical design documents and vendor model availability
were not audited for retirement. Detailed gaps appear below.

[The complete inventory](seam-cleanup-pass-two-inventory.tsv) compares all 180
test-bearing Rust files against `861d297`: 1,601 declarations before, 1,595 after.
Feature-gated and ignored declarations are included; table rows, subprocess
invocations and doctests are not separate declarations. No test file was removed.
The six-declaration reduction reflects the evidenced deletions/consolidations
below; reducing count was not an acceptance criterion.

## Production cleanup

- Removed the obsolete single-model resolver result/helper chain. Tests now use
  the actual supervisor candidate resolver; public first-viable resolution keeps
  its existing behavior. Removed the ignored internal launcher ID argument.
- Routing records now store their always-present child handle directly. Removed
  the redundant cached status, impossible missing-handle branches and always-zero
  future recursive-depth field/getter. Cached-result and live-status guards remain.
- Pending login capacity is the fixed production limit of 32, without a private
  field configurable only by tests. Deterministic expiry inputs remain.
- Removed the inert `HealthSignal::CandidateAdvanced` path: its handler only read
  and discarded snapshots. Removed the unused health retry index. Actual retry
  advancement events and their indices remain observable.
- CLI rendering directly owns its always-present terminal handle. Removed its
  single-use test-only forwarding wrapper. Optional subscription state remains
  because subscription need not have started.
- CLI model listing uses the catalog's already-enforced sort order directly.
- Removed dead MCP compatibility names `McpServerToolIdentity` and
  `McpServerListedTool`; the canonical types and functional transport APIs remain.
- Removed the unreachable retry error accumulator: every nonempty attempt loop
  already returns its result, and zero attempts still return `Timeout(0)`.
- Removed the unused core `pretty_assertions` dependency and its now-unreferenced
  lockfile dependencies `diff` and `yansi`.
- Removed the unused zero-state `AttachmentTextRenderer` facade and its reexports;
  the existing public rendering functions and compilation pipeline remain.
- Removed a repeated schema insertion already performed by recursion, preserving
  the distinct properties-map traversal. Removed redundant `ssh://`/`git://`
  checks already covered by the preceding `contains("://")` URL check.
- Merged a pure tool-result forwarding wrapper into its implementation, retaining
  the existing callable name, result events, failure counter and message order.
  Corrected alias documentation that described current dispatch as future work.

Removed public placeholder/compatibility names are intentional development-stage
API changes under the requested cleanup. Working CLI consumers were migrated in
the same tree. Lack of local consumers alone did not justify deleting functional
public SDK capabilities.

## Test decisions and coverage

| Verdict | Defect and retained/revised contract |
| --- | --- |
| Delete | `handle_accessors_compile` only compared two nil UUID aliases and never created or used a handle. Actual routing/supervisor handle tests remain. |
| Rewrite | Five profile tests used the obsolete single-candidate helper. They now check the real ordered candidate list while retaining API-key, header, callback, local-provider and reasoning inputs. |
| Rewrite | Six supervisor captures slept 50 ms although launch records synchronously before returning. Removed sleeps; kept every assertion and real scheduling/timeout tests. |
| Rewrite | Three pending-login tests now fill the real independently asserted 32-entry bound instead of unsupported capacities 1/2; expiry and claimed-capacity cases remain. |
| Rewrite | Snapshot resource serialization checked absence of strings never present in its input. Exact resources JSON now protects field placement, metadata and absence of extra payload fields. |
| Rewrite | Two provider callback tests manually invoked the callback. Actual generation and streaming requests now require one callback per HTTP request, equality to transmitted JSON and independent model/input/stream assertions. |
| Rewrite | Three Responses tool parsing cases now deserialize literal wire JSON instead of constructing private structs; IDs, arguments, text and finish reasons are asserted for all original shapes. |
| Rewrite | Anthropic's thinking test now supplies the temperature it expects to be suppressed. The disabled-thinking positive control remains. |
| Consolidate | Two CLI audio HTTP unit tests duplicated subprocess coverage and mutated global environment. Retained transcription plus both speech input rows in isolated subprocess tests, with exact request/output/auth checks. Five pure audio unit tests remain. |
| Rewrite | MCP elicitation acceptance now crosses the real initialized rmcp protocol dispatch before coordinator interaction; schema/source and response assertions remain. |
| Consolidate | Two raw MCP send-timeout tests become a virtual-time table retaining both original settings plus request-over-connect precedence. The old name incorrectly claimed connection timing despite injecting an already-connected transport. |
| Delete fixture scaffolding | Removed never-enabled MCP receive delay and unused generic bridge fake branches. Retained send delay and exact downstream tool-error propagation. |
| Delete | Duplicate runtime no-snapshot-payload subscription case had identical setup/input/assertion already included in the retained stronger semantic-ordering test. |
| Rewrite | Session lock helper is explicitly ignored at top level and invoked with `--ignored --exact` by its three parent process tests. Five child invocations still exercise real OS locks; it no longer appears to pass by doing nothing. |
| Rewrite | Recovery fixture uses fixed thread IDs ordered 2 > 1 instead of repeatedly drawing random UUIDs. Non-first default-thread selection assertions remain. |
| Delete/rewrite | Removed vacuous no-op health signal test. Retained fallback integration test now checks successful destination health as well as source failure and actual advancement event. |
| Rewrite | Catalog precedence checks both insertion orders; catalog and selector roundtrips compare full values rather than counts or one identifier. |
| Rewrite | Two shallow schema tests now assert exact recursive objects/arrays, explicit additional-properties settings and original boolean/root inputs. Image compilation checks exact content and base64 bytes instead of merely nonempty data. |
| Rewrite | Tool visibility checks complete descriptor and executable name lists for all original allow/exclude/no-tools inputs, detecting hidden-tool leakage. |
| Rewrite | `ask_user` checks complete delivered questions, labels, defaults and form fields for all five original prompt kinds instead of only enum variants. |

Provider HTTP fixtures exposed an intermittent `runtime dropped the dispatch task`
failure. The shared reqwest client persists across per-test Tokio runtimes while
`MockServer::start()` pools server endpoints. Eleven provider fixture sites now use
fresh `MockServer::builder().start()` listeners, preserving keep-alive without
retries, serialization or response-header workarounds. The initial OpenAI failure
printed the transport error; a subsequent Copilot assertion failure did not print
its error, so the same cause for that second failure is unconfirmed.

## Retained uncertainties and separate findings

- Supervisor `wait` and `wait_any` read child status before subscribing to
  completion broadcasts. Completion between those operations can be missed.
  This pre-existing concurrency defect remains a separate follow-up; wait tests
  were not deleted or relaxed.
- MCP managed `connect` does not consume the raw transport request/connect
  timeout settings. The corrected test accurately covers raw operation timeout
  selection; it does not claim to validate managed connection timeout behavior.
- Core audio realtime reconnect/heartbeat and malformed response behavior were
  source-reviewed, not newly exercised live. CLI transcription/speech use local
  HTTP fixtures. Callback stream tests consume `[DONE]`; they do not replace the
  retained stream-state tests or prove every SSE mapping.
- CLI stale-subscription replay and multi-thread snapshot fallback were read but
  have no newly added direct renderer test. Large projector/runtime integration
  matrices received bounded candidate review, not assertion-by-assertion review.
- Public raw MCP transports, auth override helpers, provider model presets,
  credential fault injection and usable embedding APIs remain. Their absence from
  some application paths is not evidence that their supported behavior is dead.
- Live Codex testing exposed a separate auth lifecycle defect: login imported an
  expired access token from the native Codex cache, assigned no expiry, and the
  actual request credential path bypassed the existing refresh helper. Local
  status therefore reported signed in while the provider rejected the token.
  Retained the refresh capability; it is not dead merely because this connection
  is missing. Native Codex renewal followed by reimport was provided to the user.

## Verification

All commands used Rust 1.96.1:

- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace --all-features`: **1,596 passed, 5 ignored**. Counts
  include doctests but exclude nested subprocess helper summaries. One additional
  ignored case is the catalog helper now explicitly invoked by passing parents.
- `cargo test -p roci-core --no-default-features`: 496 passed, one ignored helper.
- `cargo test -p roci`: 11 passed.
- Final CLI build: `cargo build -p roci-cli --all-features --features roci/all-providers`.

Focused seam checks also passed. Transient compilation errors from an in-flight
MCP test rewrite were corrected before its successful protocol test run. No
failing tests were deleted or relaxed to obtain these results.

Live verification used interactive tmux session `roci-cleanup-live` (attach with
`tmux attach -t roci-cleanup-live`). Secrets remained in the environment/auth store.

| Provider/path | Command/settings and observed result |
| --- | --- |
| Gemini final CLI, catalog | `models list --provider google`, both JSON and table: exit 0. |
| Gemini real delegation | `chat --no-skills --agent smoke --model google:gemini-2.5-flash-lite --temperature 0 --max-tokens 512 --max-retry-attempts 1 --approval always --session-root <temp>/sessions --session-id delegation`; real child call emitted started/completed and returned `roci-pass2-child-ok`, exit 0. |
| Gemini durable resume | Second process, same session, without `--agent`: real child returned `roci-pass2-resumed-ok`, exit 0. Metadata retained `agent_profile: smoke`. |
| Gemini attachment/export | `chat --no-skills --no-tools --model google:gemini-2.5-flash-lite --temperature 0 --max-tokens 64 --max-retry-attempts 1 --attach <temp>/notes.txt` with durable session: `roci-pass2-attachment-ok`, exit 0. Session export passed; persisted files and export contained no original attachment host path. |
| GitHub Copilot | `chat --no-skills --no-tools --model github-copilot:gpt-5-mini --temperature 0 --max-tokens 512 --max-retry-attempts 1`: `roci-pass2-copilot-ok`, exit 0. Authenticated dynamic catalog listing also succeeded. |
| Codex | Same bounded chat settings, `codex:gpt-5.2`: HTTP 401, “Could not parse your authentication token”, exit 1. Expired imported credential diagnosis above; not counted as verified. |
| OpenAI / Anthropic | Signed out after checking saved auth status; no successful live call claimed. |
| Local / Framed | `127.0.0.1:1234/api/v0/models`: connection refused. `http://framed:4001/v1/models` with dummy key: HTTP 500. Unavailable for live smoke. |

Execution evidence for this session lives under `/tmp/roci-pass2-`: workspace,
minimal/default test, Clippy, final build and final-live logs, plus individual
provider logs. Audit and inventory are committed records; temporary logs are not
repository artifacts. The final Gemini run used the rebuilt tree after all
production edits and covered the tool-result path through real delegation.
