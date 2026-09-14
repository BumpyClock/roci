# Dead code and test cleanup — 2026-09-14

This cleanup used three parallel reviewers plus a coordinating reviewer, tracked
under Tasque `tsq-1`. Production removals were limited to demonstrably unused or
test-maintained scaffolding. Test edits preserve supported contracts and replace
fixture-only assertions with calls to the actual implementation.

## Inventory and inspection limits

[The complete inventory](test-cleanup-inventory.tsv) lists all 180 Rust files with
test declarations, including embedded suites and integration targets. Counts are
source declarations, including feature-gated and ignored tests; each table loop
counts once. There are no generated test frameworks in the inspected suites.
Doctests and subprocess invocations are excluded from the declaration inventory.

| Area | Test-bearing files | Before | After |
| --- | ---: | ---: | ---: |
| Core runtime and agent loop | 58 | 574 | 565 |
| Core services | 76 | 627 | 621 |
| Providers | 27 | 221 | 212 |
| CLI | 16 | 143 | 143 |
| Tools | 2 | 49 | 47 |
| Meta crate | 1 | 20 | 13 |
| Total | 180 | 1634 | 1601 |

All Rust areas were inventoried and screened for unused declarations, dead-code
allowances, test-only configurations, legacy markers, callers, and weak tests.
Deep review concentrated on the changed suites and their retained coverage,
runtime key/prompt/state routing, context recovery/budgets, session construction,
credential stores, provider registration/overflow, CLI rendering, and tool
contracts. This is **not an assertion-by-assertion review of all 1,634 tests**.
Unmodified session recovery/persistence, subagent orchestration/routing,
projection, retry/tool execution, MCP transports, attachments, audio, security,
and skills suites received candidate screening or sampled review. Their remaining
tests are retained. Standalone scripts and historical design documents were not
audited for deletion. Cargo compiled the examples and exercised existing doctests.

## Production removals

- `agent/runtime/chat/event.rs`: removed 39 `AgentRuntimeEventPayload::*_name()`
  literal getters. Repository-wide searches found 24 used solely by a literal-name
  test and 15 with no callers. Serde never called them; changing a real wire tag
  could leave their test green. Every actual event variant remains.
- `context/budget.rs`: removed `BudgetDecision` and its reexport. Its only users
  were two construction/derived-equality tests. Its own documentation said the
  producer was not wired; the working runner instead uses `BudgetSnapshot` and
  `RecoveryDecision`. All budget arithmetic and enforcement remain.
- `context/recovery.rs`: removed private configurable policy fields and
  `with_test_config`. Production could only construct the documented fixed
  two-attempt/500-token policy. Its getters and decisions now use those constants
  directly, preserving every reachable decision.
- `session/snapshot.rs`: removed unused `SessionResumeState::new`, which had an
  unconditional dead-code allowance. The real store still constructs the complete
  state with its exclusive lease. Narrowed the remaining allowance and gated
  `SessionConfig::canonicalize_root` with its sole agent-feature consumer.
- `agent/runtime/state.rs`: inlined `publish_runtime_event_to` into its sole
  caller without changing event publication or error handling.
- `roci-cli/src/chat/runtime_events.rs`: unified identical test and production
  `spawn_with_prompt_fns` implementations. Existing interaction/approval tests now
  run the same implementation shipped by the CLI.
- `roci-providers/Cargo.toml`: removed unused `pretty_assertions` and the direct
  production Tokio dependency. All direct Tokio uses are test attributes; the dev
  dependency and runtime features required by core remain.

The removed budget placeholder and event-name methods are intentional SDK API
removals under the requested dead-code cleanup. No CLI callers needed migration.

## Test verdicts and retained coverage

Paths below are relative to the owning crate's `src/`, unless stated otherwise.

| Verdict | Candidates and defect | Contract retained or improved |
| --- | --- | --- |
| Delete | Core `context/budget.rs`: two `BudgetDecision` construction/equality tests | No implemented contract belonged to this unproduced enum. Budget snapshot arithmetic and runner budget tests remain. |
| Delete | Core `context/recovery.rs`: three alternate-policy configuration tests | These exercised impossible production configurations. Retained 500/499 progress boundaries, attempt exhaustion, output reduction ordering, and complete episode traces exercise the fixed ladder. |
| Delete | `input_overflow_skips_output_reduction` | Same policy, signal, and initial state as retained `input_overflow_first_action_is_compact`; its exact action/reason assertions imply the removed negative assertion. |
| Delete | Core `runtime_tests/value_types.rs`: four derive/debug-only cases | No custom behavior or stable debug-output contract. Existing `snapshot.rs` and lifecycle tests cover actual state, snapshots, and notifications. |
| Delete | Core `runtime_tests/api_key.rs::get_api_key_error_propagates` | It called a locally defined closure. Retained `prompt_get_api_key_error_restores_idle_state` invokes the runtime and checks the authentication error, idle restoration, snapshot error, and stopped streaming. |
| Rewrite | Six runtime key tests consolidated into two | Real prompts now cover five config/override/callback combinations and three consecutive rotating keys. A recording provider observes requests, completed runs, and callback counts. Provider transport tests retain actual HTTP authentication coverage. |
| Rewrite | `semantic_payload_set_matches_target_contract` | Serialize and deserialize 17 actual event variants against independent wire tags; retained `resource_event_payloads_have_stable_names` covers the other seven original inputs. Existing subagent serialization covers eleven additional variants. |
| Rewrite | Runtime system-prompt, watch-state, and two queue tests | Invoke actual prompt/state transitions; assert provider message roles/order/text, watch notification, FIFO order, remaining lengths, and complete exhaustion. Old prompt/watch cases manually recreated the desired state. |
| Rewrite | Core config default-store debug assertion | Save/read a credential in one default store and require an independent store to lack it. Detects no-op storage and unintended shared persistence; debug redaction coverage remains. |
| Rewrite | Providers `tests/factory_registration.rs`: nine presence/count cases | One exact feature-aware key table retains every original key and checks all enabled providers and disabled-key absence. Two synchronous alias cases retain their assertions without unnecessary Tokio runtimes. |
| Delete | Providers `overflow.rs::builtin_overflow_detector_fn_creates_composite` | Retained `tests/overflow_surface.rs::builtin_overflow_detector_is_reachable_from_crate_surface` uses the same public helper and typed OpenAI error, with stronger exact `InputOverflow` and public-accessibility checks. Different message text does not affect the typed path. |
| Rewrite | Tools `all_tools_returns_six_tools` / `all_tools_contains_expected_names` | One exact six-name assertion also detects missing `ask_user`, duplicates, and substitutions that the count plus five-name checks missed. |
| Rewrite | Tools `shell_times_out_on_long_running_command` | Replaced a separately implemented fake tool with the actual shell tool. Virtual time verifies its 30-second timeout; a temporary release file prevents host scheduling from ending the process early, and a 31-second outer deadline catches a missing tool timeout. TempDir cleanup releases the child even on panic. |
| Delete | Tools `read_file_keeps_host_absolute_paths_without_session_context` | Retained and renamed `read_file_returns_host_absolute_file_contents_without_session_context` already uses an absolute tempfile path and checks contents plus byte count/truncation metadata. Equivalent environment; fixture text has no semantic difference. |
| Rewrite | Tools missing-file fixture and three `ask_user` validation assertions | Missing-file test now owns a temporary directory. Invalid prompt cases require the specific `InvalidArgument` reason, so a missing callback or disabled feature cannot falsely satisfy them. Original input cases remain. |
| Delete | Root `tests/meta_crate_integration.rs`: three duplicate default-provider feature cases and lower-bound count | Retained default OpenAI, Anthropic, Google, and Codex checks use the same public registry, keys, and hermetic environment. Optional Grok/Groq feature cases and reexport compile checks remain. |
| Rewrite | Root four auth backend presence/count cases | Exact enabled-backend names, including dependency feature unification, replace unconditional assumptions about Copilot. Provider tests independently retain backend metadata and alias contracts. |

No test file was removed. The net reduction is 33 Rust test declarations,
including consolidations; reducing test count was not an acceptance criterion.
Independent cross-review found no lost supported inputs or weakened assertions.

## Retained and unresolved candidates

Public SDK conveniences, OAuth endpoint overrides/token accessors, and MCP
compatibility aliases remain: absent workspace callers do not establish that
their capabilities are obsolete. `abort_legacy` remains because the current
cancellation path calls it during preflight/error windows. Optional provider
helpers with dead-code allowances have genuine feature-gated consumers.

MCP mock constructors, credential-store fault injection, child-process lock
probes, compile-time/object-safety checks, and security/persistence cases remain.
Their fixture hooks and assertion-free bodies can protect real contracts.

## Verification

The initial installed compiler (1.93.1) failed the workspace's MSRV requirement.
Rust 1.96.1 was installed; the full baseline passed before source edits.

- Baseline: `cargo +1.96.1 test --workspace --all-features`: 1,636 passed,
  four existing ignored cases including doctests.
- Final: the same command: **1,603 passed, four unchanged ignored**. These totals
  include doctests and count nested subprocess lock probes only through their
  outer tests. The source inventory differs because it includes every conditional
  declaration and excludes doctests.
- `cargo +1.96.1 clippy --workspace --all-targets --all-features -- -D warnings`:
  passed.
- `cargo +1.96.1 fmt --all -- --check` and `git diff --check`: passed.
- `cargo +1.96.1 check -p roci-core --no-default-features`: passed.
- `cargo +1.96.1 test -p roci --test meta_crate_integration`: 11 passed with
  default features, in addition to 13 with all features.
- Provider registration without default features: two passed after cleanup;
  before cleanup, five cases failed because they assumed default providers.
- Focused runtime, context, credential-store, provider, and tool suites passed.
- Updated CLI built successfully with all provider features.

Live verification ran through the real CLI in interactive tmux session
`roci-cleanup-live`; attach with `tmux attach -t roci-cleanup-live`.

```sh
target/debug/roci-agent chat --no-skills --no-tools \
  --model google:gemini-2.5-flash-lite --temperature 0 --max-tokens 32 \
  --max-retry-attempts 1 'Reply exactly: roci-cleanup-live-ok'
```

Google's default endpoint returned **`roci-cleanup-live-ok`**, exit **0**. The
existing Gemini credential was inherited without printing it. The first tmux
window lacked that environment; a new window inherited the configured credential
and succeeded. This exercises core runtime, Google transport, and shared CLI
renderer startup.

Additional target: OpenAI-compatible `http://framed:4001/v1`, model `gemma-4-e4b`,
configured dummy key, same bounded prompt: HTTP 500, CLI exit 1. Its authenticated
model-list request also returned HTTP 500. Local `127.0.0.1:1234` refused the
connection. OpenAI, Codex, Claude, and Copilot had no configured credentials;
their live authentication/calls were not verified. These are recorded limits,
not successful provider checks.
