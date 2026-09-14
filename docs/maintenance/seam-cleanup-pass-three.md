# Seam cleanup, third pass — 2026-09-14

Baseline: `53c40b5`, after two committed cleanup passes. The baseline full
workspace/all-features run passed 1,596 tests with five ignored cases. This pass
used successive native parallel agent tasks for independent seams, followed by
two independent cross-reviews of the combined changes. Tasque was skipped as
requested. Applied programming, rust-skills, and test-cleanup guidance.

## Scope and inventory

[Declaration inventory](seam-cleanup-pass-three-inventory.tsv): 180 test-bearing
Rust files, 1,595 → 1,589 declarations. Counts include ignored and feature-gated
`#[test]`/`#[tokio::test(...)]` attributes; loops count once, doctests and nested
subprocess executions are excluded. A smaller count was not an acceptance
criterion: useful contracts were strengthened, and a runtime mutation contract
was added. Historical inventories retain prior-pass evidence.

| Seam | Inspection this pass |
| --- | --- |
| Generation | All generation modules, results, stop conditions, Agent execution/stream callers, examples and documentation. |
| Runtime lifecycle | Construction, validated candidates, mutations, queue/terminal transitions, abort/reset, summary/compaction; lifecycle, snapshot, hooks, dynamic-tool and mutation tests. Other chat/subagent matrices were screened and exercised. |
| Context and runner budgets | Token accounting, compaction helpers/artifacts, overflow/recovery, actual consumers; 13 test-bearing files, 205 → 200 declarations. |
| Human interaction | Coordinator, typed schemas, waiters, cancellation and user-input types; 14 owned declarations plus runtime/MCP/CLI consumer tracing. |
| Security/approval | All security modules and approval matching/grants/requests; 54 declarations inspected. |
| CLI commands | Entrypoint, argument definitions, auth/session/skills/profile/resource/MCP parsing and wiring; nine selected files, 113 declarations. |
| Provider streams | Responses state/dispatch/tests, Chat decoder/reasoning tests; Anthropic/Google decoder bodies and adjacent tests. Wrappers, factories and registry consumers screened. |
| Audio | All ten source files: transcription/speech, realtime bootstrap, heartbeat/reconnect/event delivery; four CLI subprocess contracts retained. |
| Config/auth/resources/skills/MCP/facade | Full config production paths; auth host and selected manager/store tests; resource/skill production paths and all seven skill-manager tests. Remaining public declarations and example/registry chains screened. |
| Dependencies/features | Manifest/source reference scan, remaining dead-code allowances, minimal/default feature builds, session metadata/lock feature gates. |

Not fully re-audited assertion by assertion this pass: built-in tool suites,
model catalog/health/retry, session storage/recovery matrices, full projector
subscriptions, CLI renderer/model smoke implementations, full provider request,
auth and catalog suites. Many received targeted prior-pass review. Scripts and
historical prose were not exhaustively audited. This is evidence of the reviewed
seams, not a proof about every downstream SDK usage or feature combination.

## Production removals

- Generation helpers no longer accept a tools parameter that could only reject
  nonempty input. Removed `stream_text_with_tools`, unattached `StreamTextResult`,
  and single-element `GenerationStep` scaffolding. Preserved the distinct
  provider tool-call data directly in `GenerateTextResult.tool_calls`; Agent tool
  execution still uses the actual loop runner. Migrated CLI-adjacent callers,
  example and provider documentation. These are intentional development API
  changes under the user's broader dead-code cleanup authorization.
- Runtime stores validated `ModelCandidates`, eliminating repeated validation
  and impossible empty-candidate failures. Removed the duplicate always-success
  `active_candidate` API. A private terminal outcome enum replaces invalid
  status/error combinations; queue transitions drop an impossible error arm.
- Human request registration is infallible; removed its alias and impossible
  error/canceled-success branches. Removed enum-impossible schema validation,
  folded a private conversion wrapper into `From`, and removed impossible JSON
  value serialization recovery. Actual validation/cancellation stays at its
  producing boundary.
- Removed unused command-classifier interface/input/context scaffolding; kept
  the real shell classifier and insights. Removed never-produced policy-change
  suggestions and specificity variants, and unimplemented rule grants that
  could never match and were always dropped. Exact grants retain their wire
  shape and real policy matching remains.
- Audio validates MIME types through its existing 13-alias extension mapping.
  Removed the duplicate allowlist, second unreachable validation failure and an
  unused private Clone implementation; early error ordering remains.
- Removed a cfg-selected single-variant credential-store tag/getter. Production
  constructs the same backend directly. Resource prompt resolution accepts the
  already-validated JSON object, removing an impossible non-object success.
- Removed a forwarding API-key helper, unused CLI base64 dependency edge, unused
  generic profile-hint widths, duplicate MCP redaction state and no-op bindings.
  Session/provider helpers now compile only with their consumers' features.
  The remaining session lease dead-code allowance preserves lock ownership and
  drop lifetime when agent support is disabled; it is not a disposable readless
  field.
- Responses flushing drops an emitted-call check at a cursor that has already
  advanced past every emitted entry. Finalization deduplication stays intact.

## Test decisions and retained contracts

| Verdict/group | Evidence and coverage retained or strengthened |
| --- | --- |
| Delete duplicate budget test | Misnamed accumulation test made one identical provider call as `no_budget_configured_preserves_existing_behavior`. Moved its total-token assertion there. Real two-call accumulation remains in `exact_anchor_allows_request_that_full_heuristic_would_reject`. |
| Delete four constructability/fixture tests | Recovery enum construction is enforced by actual runtime producers. Artifact fields are covered by real assembly/span/file operations. Null/always-overflow fixtures only asserted their own constants; real provider classifiers and object-safety coverage remain. Removed now-unused always-overflow fixture. |
| Delete duplicate idle abort | Identical first abort remains in repeated-idempotency test with the same empty runtime and additional state assertions. |
| Delete platform-tag tautology | Test compared the cfg-selected tag with itself. Actual Unix default backend is exercised by fresh CLI configure/status/providers processes under isolated HOME; concrete OS-store Debug coverage remains gated for non-Unix. |
| Replace retired generation rejection tests | New provider-boundary tests check exact messages, nondefault settings, response format, usage, finish and tool-call preservation. Streaming tests exercise no-stop and early-stop rows, reset ordering, accumulated text and event metadata. All three code-fence inputs retained. |
| Strengthen runtime behavior | Reset starts from a real completed turn; dynamic tools are resolved by name before/after replacement and clearing. Independent subscribers must receive changes. Reset observers first see dirty state. New candidate-mutator test protects empty/busy rejection, order/dedup and preservation of previous state. |
| Strengthen compaction/budgets | Exact summary content and split/no-split token accounting; injected counter verifies summarized span; boundary case actually reaches tool-result adjustment. Exact truncation length/identity/error. Cancellation waits for recorded dispatch rather than a scheduler sleep. |
| Strengthen human/wire contracts | Independent expected JSON shapes retain original prompts/results/IDs. Errors retain non-nil request IDs. Cancellation exercises both cancel_all and explicit canceled response, including pending-state removal. |
| Strengthen security corpora | Full expected redacted output for every original input, retaining negative cases; UTF-8 test now places multibyte text before the secret to check byte offsets. |
| Strengthen CLI/auth | Exact resource composition and public profile formatter Unicode/boundary behavior. Auth queues fail on unexpected calls. Virtual-time slowdown proves interval update; budget defaults retain explicit input plus omitted reserve. Secret checks now exercise actual stored sentinel credentials in all claimed output formats across four processes. |
| Strengthen host DTOs | Exact PKCE Debug and fixed-time DeviceCode JSON replace absence assertions against already-secret-free values. Secret-bearing manager and CLI tests retain actual filtering coverage. |
| Strengthen provider streaming | Dedup/final-response fallback crosses mocked HTTP SSE, preserving earlier emission before an intervening text event; exact call order/name/arguments and usage. Reasoning deltas must contain no visible text. |

No failing test was deleted or relaxed. Focused seam baselines/checks passed.
The independent reviews found no lost supported input or regression in the
combined runtime/generation/human and budget/stream/CLI/security changes.

## Retained uncertainties and separate defects

Useful public APIs remain even where workspace callers are sparse: raw MCP and
schema builders, host auth/status/override APIs, provider-neutral elicitation
DTOs/capabilities, realtime audio, stop conditions, context utilities and manual
summary operations. No evidence establishes their retirement. Unwired Codex
refresh is a missing integration, not permission to delete refresh behavior.

Separate pre-existing findings, not fixed or hidden through test deletion:

- Subagent wait status check precedes completion subscription and can miss a race.
- Managed MCP connect does not consume raw transport timeout configuration.
- Native Codex import can retain an expired token while the request path bypasses
  refresh; prior direct live verification failed despite signed-in status.
- Four stream decoders decode lossy UTF-8 per transport chunk; split characters
  can corrupt. Chat drops standard trailing usage-only chunks with empty choices.
- Duplicate human request IDs can let an old waiter remove a newer request;
  dropping an unawaited pending request can retain its record until cancellation.
- Realtime heartbeat interval zero can panic. No new live realtime/audio-provider
  verification or exhaustive Google/Anthropic streaming mapping test was added.
- Micro-compaction typed-marker idempotence has unclear semantics; guards remain
  reachable for caller-supplied typed markers, so were retained.

## Verification

All commands use Rust 1.96.1. Final results and live evidence are recorded below.

- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test --workspace --all-features`: **1,590 passed, 5 ignored**;
  doctests included, three nested subprocess summaries excluded.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed
  again after final feature-gate changes.
- `cargo test -p roci-core --no-default-features`: 491 passed, one ignored.
- `cargo test -p roci`: 11 passed.
- Final `cargo test -p roci-providers --all-features`: 216 passed, including
  doctests. Provider no-default tests: 58 passed. Strict provider Clippy with
  all targets passed for no-default, default, all-features and each independent
  base feature (OpenAI, Anthropic, Google). The initial no-default check exposed
  nine unused helper/import warnings; correct consumer feature gates removed
  them without suppressions or test deletions.
- Final CLI and multi-provider example rebuilt after those gates:
  `cargo build -p roci-cli --all-features --features roci/all-providers` and
  `cargo build --example multi_provider`.

Live verification used interactive tmux session `roci-cleanup-live`:
`tmux attach -t roci-cleanup-live`. The user supplied proxy
`http://127.0.0.1:8317/` with dummy key `sk-dummy`; its `/v1/models` returned
HTTP 200. Final main script used an isolated temporary cwd and the final rebuilt
CLI. Credentials remained in the environment/auth store.

| Path | Command/settings and observed result |
| --- | --- |
| Proxy OpenAI catalog | `models list --provider openai`, JSON and table: exit 0. |
| Proxy Chat/delegation | `chat --no-skills --agent smoke --model openai:gpt-4o --temperature 0 --max-tokens 512 --max-retry-attempts 1 --approval always --session-root <temp>/sessions --session-id delegation`: real child started/completed, `roci-pass3-child-ok`, exit 0. |
| Proxy durable resume | Second process without `--agent`, same session: child started/completed and `roci-pass3-resumed-ok`, exit 0. Persisted metadata contains `agent_profile: smoke`. |
| Proxy attachment/export | `chat --no-skills --no-tools --model openai:gpt-4o --temperature 0 --max-tokens 64 --max-retry-attempts 1 --attach <temp>/notes.txt` with durable session: `roci-pass3-attachment-ok`, exit 0. Export passed; persisted files/export contain no original attachment host path. |
| Proxy runtime switch | `models switch-chat-smoke --from openai:gpt-4o-mini --to openai:gpt-4o --prompt 'Reply exactly: roci-pass3-switch-ok' --expect roci-pass3-switch-ok --json`: correct previous/current models, Completed and expected response, exit 0. |
| Generation helper | `multi_provider` example: actual OpenAI `gpt-4o` through proxy and Google `gemini-2.5-flash` direct returned prose and token usage, exit 0. Its Anthropic entry reported missing credentials; it is not counted as verified by this example. |
| Proxy Responses | `chat --no-skills --no-tools --model openai:gpt-5-mini --max-tokens 512 --max-retry-attempts 1`: `roci-pass3-responses-ok`, exit 0. |
| Proxy Anthropic | `ANTHROPIC_BASE_URL=http://127.0.0.1:8317/v1`, dummy key, `chat --no-skills --no-tools --model anthropic:claude-haiku-4.5 --max-tokens 128 --max-retry-attempts 1`: `roci-pass3-anthropic-ok`, exit 0. |
| Proxy Codex adapter | `OPENAI_CODEX_BASE_URL=http://127.0.0.1:8317/v1`, dummy token, `chat --no-skills --no-tools --model codex:gpt-5-mini --max-tokens 512 --max-retry-attempts 1`: `roci-pass3-codex-proxy-ok`, exit 0. This does not verify native Codex OAuth renewal. |
| Direct Copilot | Saved authentication, `chat --no-skills --no-tools --model github-copilot:gpt-5-mini --temperature 0 --max-tokens 512 --max-retry-attempts 1`: `roci-pass3-copilot-ok`, exit 0. |
| Interactive human request | Proxy `openai:gpt-4o`, tools enabled, `--approval always`, max 512 tokens: model called ask_user, CLI displayed question, tmux input `roci-pass3-human-ok` resolved request, model returned same marker, exit 0. |
| Original local/Framed | Port 1234 refused connection; Framed `/v1/models` returned HTTP 500. Replaced with user-supplied working proxy. |

The first human-input smoke used plain `timeout`, which moved the CLI into a
background terminal process group and stopped raw-mode initialization. Process
state/foreground-group inspection established a harness issue. Rerunning with
`timeout --foreground` passed; no production/test behavior was weakened. The
working command and current single-profile TOML shape are in `docs/testing.md`.

Direct OpenAI/Anthropic authentication remains unconfigured and native Codex
renewal is not newly verified; successful proxy transport calls above are not
claims about those OAuth flows. Realtime audio and live external MCP were not
exercised. Automated audio HTTP subprocess/protocol tests remain intact.

Temporary execution reports/logs are under `/tmp/roci-pass3-*`, including
workspace, feature matrices, focused tests, two cross-reviews, final build,
final-live and human-live evidence. This audit and declaration inventory are the
committed record. All confirmed removal candidates found in the inspected seams
were addressed; functional public capabilities with uncertain retirement
remained explicitly retained.
