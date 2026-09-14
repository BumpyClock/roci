# Native auth validation

The implementation follows the authorization/protocol flows inspected in
`references/CLIProxyAPIPlus` (commit `e197b6c0`), with independent session
ownership, explicit imports, coordinated refresh, named accounts and durable
Cursor state added in roci.

## Live evidence (2026-09-14)

Interactive terminal: `tmux attach -t roci-auth-live`.

| Path | Target | Observable result |
| --- | --- | --- |
| OpenAI-compatible | `http://framed:8317/v1`, `gpt-4.1` | `roci-auth-smoke-ok`, exit 0 |
| OpenAI SDK transport | Same endpoint, `openai:gpt-4.1` | `roci-openai-path-ok`, exit 0 |
| Native GitHub Copilot | `github-copilot:gpt-4.1` | `roci-copilot-native-ok`, exit 0 |
| Native Copilot catalog | `models list --provider github-copilot --json` | 24 models, exit 0 |
| Gemini API key | `google:gemini-2.5-flash` | `roci-gemini-key-ok`, exit 0 |
| Responses fast speed through proxy | `codex:gpt-5.4`, base URL `http://framed:8317/v1`, `--reasoning-effort low --speed fast` | `roci-responses-fast-ok`, exit 0; verifies Responses transport, not native Codex OAuth |
| Initial native Codex session | `codex:gpt-5.4` | Old refresh rejected; later fresh `live-test` login verified below |
| Native Cursor catalog | `models --account live-test list --provider cursor --json` | 223 models, exit 0 |
| Native Cursor Sonnet | `cursor:claude-4.6-sonnet-medium`, `live-test` | `roci-cursor-sonnet-ok`, exit 0 |
| Native Cursor families | `models --account live-test list --provider cursor --json` | 36 picker entries from 223 upstream IDs, exit 0 |
| Native Cursor Sonnet family | `cursor:claude-4.6-sonnet`, `--reasoning-effort medium` | `roci-cursor-family-ok`, exit 0 |
| Native Cursor fast family | `cursor:gpt-5.6-terra`, `--reasoning-effort high --speed fast` | `roci-cursor-fast-ok`, exit 0 |
| Native Cursor process restart | Sonnet family, durable `sonnet-resume` session, second process inherits account | `SAVED.`, then `cursor-cobalt-7291`; both exit 0; same upstream conversation, advanced checkpoint, blobs 9 → 15, mode 0600 |
| Native Cursor SDK tool round trip | Sonnet family medium, `--tool read_file`, temporary marker file | `read_file` executed once; final `roci-cursor-tool-cobalt-9421`, exit 0 |
| Native xAI, Gemini OAuth, Claude OAuth | `live-test` account login windows | Awaiting browser authorization and successful model calls |

The configured local endpoint at `127.0.0.1:1234` refused connections.
`framed:4001/v1/models` returned HTTP 500. The user supplied the working
port-8317 endpoint and dummy key. Proxy success verifies the SDK's API-key
transport; it does not verify native Cursor/xAI authentication or execution.

The initial `gpt-5-mini` proxy smoke exited 0 without visible output at a
64-token cap; it was not accepted as provider-response evidence. The `gpt-4.1`
request above produced the required visible response.

Native Cursor completed-text restart reuse was verified against the live service,
comparing sanitized hashes and counts without printing checkpoint content.
Live tool-result continuation also passed using the SDK read_file tool and full
transcript continuation; this does not imply reconnecting an old upstream exec ID.
See [testing.md](testing.md) for the commands and acceptance requirements.

## Automated checks

`cargo +1.96.1 test --workspace --all-features --no-fail-fast`,
`cargo +1.96.1 clippy --workspace --all-features --all-targets -- -D warnings`,
and `cargo +1.96.1 fmt --all -- --check` passed. The Cursor-only and minimal
provider feature builds also compiled.

Coverage includes rotated-refresh races, logout/relogin conditional writes,
separate processes and named accounts, manager/store rebinding, PKCE state,
explicit import behavior, bounded HTTP 401 replay, cancellation, Anthropic
OAuth/API-key header selection, Copilot derived endpoints, native Gemini
onboarding/transport, and native Cursor HTTP/2 checkpoint/blob restoration.

The first native Cursor catalog request returned HTTP 415 because the request
contained both streaming and unary `Content-Type` values. The corrected unary
request sends exactly `application/proto` and no streaming Connect header; a
local HTTP/2 regression test checks the complete contract. Live discovery then
returned 223 models. The upstream Sonnet 4.6 ID is
`claude-4.6-sonnet-medium`; model-family support now accepts
`claude-4.6-sonnet` and selects its advertised variant from the active account.

The restart smoke initially only replayed the saved SDK transcript: AgentRuntime
was not forwarding a durable provider session ID. It now forwards the explicit
provider override when supplied, otherwise durable session ID plus thread ID.
The repeat live smoke created and advanced an upstream checkpoint across two
processes. Separate-root and explicit-override regressions cover ID isolation.


Model family/speed validation additionally covers account-derived catalogs,
exact IDs, suffix precedence, unsupported combinations, thinking-only families,
request credential overrides, resolved-variant state isolation, Standard/Fast
service-tier mapping and conflicts, and CLI family filtering. The full workspace
all-feature suite, workspace Clippy with warnings denied, rustfmt check and
rebuilt CLI passed after these changes.


Default catalog follow-up: the rebuilt CLI now hides raw Cursor variants without
an extra flag. In `roci-auth-live`, the user's exact table command returned 36
family/singleton entries, exit 0. JSON checks confirmed no explicit variants by
default and 247 entries with `--include-variants`, preserving every default row.
CLI tests cover table/JSON defaults and opt-in variants; CLI tests, Clippy and
rustfmt checks passed.


## Live model discovery follow-up (2026-09-14)

The missing Astra entry came from `CodexFactory` returning a bundled static
catalog without an HTTP request. A direct authenticated probe of
`https://chatgpt.com/backend-api/codex/models?client_version=0.154.0` returned
HTTP 200, nine account models, including `gpt-6-astra` with visibility `list`.
Two entries were explicitly hidden. Astra's live default effort is `medium`,
with low/medium/high/xhigh/max/ultra options and priority/fast speed. The raw
account response takes precedence over bundled client metadata. Codex gates
Astra on client compatibility version 0.153.0 or later.

The rebuilt CLI was exercised in `roci-auth-live`:

| Discovery/execution path | Result |
| --- | --- |
| Native Codex, `live-test` | 7 visible models, all dynamic, including `gpt-6-astra` |
| Native Codex Astra, `--reasoning-effort low --speed fast` | `roci-codex-astra-ok`, exit 0 |
| Native Copilot, default account | 24 live models |
| Gemini API key | 41 generation-capable live models |
| OpenAI and OpenAI-compatible, `framed:8317/v1` | 320 live models each |
| Anthropic and Anthropic-compatible, `framed:8317/v1` | 320 live models each; proxy API-key verification |
| Native Cursor, `live-test` | 36 family/singleton entries derived from upstream |
| LM Studio and Ollama local endpoints | Connection unavailable; explicit errors, no presets |
| `framed:4001/v1` | HTTP 500; explicit error, no fallback |

All successful catalog commands exited 0. Neither model enums nor deleted
static catalogs determine discovery IDs. Existing model enums remain transport
and capability defaults when an upstream API provides only IDs. Codex receives
its reasoning levels, context limits, modalities and speed tiers from its live
account catalog. Unknown Codex models can forward explicit typed reasoning
without being rejected by the old preset list.

Azure deployment discovery, native Gemini OAuth discovery, and native xAI OAuth
discovery have no supported catalog endpoint with their configured credentials;
they report unsupported discovery. Other unconfigured remote providers have
local HTTP coverage but still require real credentials for native live validation.
No full native-live claim is made for those providers. See `docs/models.md`.


Final discovery verification passed workspace all-feature tests, workspace
all-target Clippy with warnings denied, rustfmt check, and the CLI build with
`roci/all-providers`. Focused HTTP tests additionally cover account isolation,
query redaction/pagination, future model IDs, no fallback after errors, bounded
responses, and disabled redirects. The Cursor-only provider Clippy check passed;
the minimal provider build compiled.

## Contract review follow-up (2026-09-14)

Three parallel reviews covered semantic runtime ownership and the auth/provider
boundaries. Credential selection now returns one typed credential/endpoint pair;
provider construction and discovery consume that pair. Token-store adapters must
implement atomic conditional writes and exclusive refresh leases. PKCE completion
requires preserved session material, and hosts advance browser/device logins with
one opaque-session operation. Aggregate discovery skips only the explicit
`ModelDiscoveryUnsupported` result; other discovery failures retain their meaning.

The final workspace all-feature suite passed 1,767 tests (6 ignored). Workspace
all-target Clippy with warnings denied, rustfmt, and the all-provider CLI build
passed. Minimal-provider tests passed (71), as did Cursor-only Clippy. Controlled
runtime tests also reproduced and fixed dropped-caller lifecycle work, first-poll
admission, and public abort bypassing a rejected semantic cancellation commit.

Live terminal: `tmux attach -t roci-semantic-owner`.

| Path | Observable result |
| --- | --- |
| OpenAI, `http://framed:8317/v1`, `gpt-4.1` | Dynamic catalog and `roci-contract-openai-ok`, exit 0 |
| Native Copilot, `claude-haiku-4.5` | Dynamic catalog and `roci-contract-github-copilot-ok`, exit 0 |
| Gemini API key, `gemini-2.5-flash` | Catalog and `roci-contract-google-ok`, exit 0 |
| Native Codex, `live-test`, `gpt-6-astra`, effort low | Catalog and `roci-contract-codex-ok`, exit 0 |
| Native Cursor, `live-test`, `claude-4.6-sonnet`, effort medium | Catalog and `roci-contract-cursor-ok`, exit 0 |
| Final Copilot delegation and second-process resume | Child read the generated marker; both turns completed, 41 ordered events, marker preserved in semantic export and provider history |

The first Gemini attempt inherited an older tmux environment without the configured
API key. A fresh window received the existing key through its environment and both
checks passed; no repository credential files were changed. The local endpoint
`127.0.0.1:1234` was unavailable, and `framed:4001/v1/models` returned HTTP 500.
Native Google OAuth and Claude authorization remain pending in the `google-login`
and `claude-login` windows for account `live-test`; their new browser flows are
not claimed as live-verified by this follow-up. Tasque `tsq-11` tracks completion.

The separate mismatch between discovered and runtime Codex capabilities is tracked
in [issue #9](https://github.com/BumpyClock/roci/issues/9). Provider registration's
credential identity and shared discovery recovery remain tracked in issues #5/#6.

### Browser-login failure diagnosis

The first native Google login exchanged its authorization code successfully, then
Code Assist completed onboarding without returning a project. This matches the
reference implementation's project-selection-required case; no
`GOOGLE_CLOUD_PROJECT` was configured. A real existing project selection is still
required to verify this account. No partially onboarded token was persisted.

Claude's first exchange returned HTTP 400 because the adapter sent form data to
an older endpoint and omitted OAuth state. The corrected adapter uses the current
platform endpoint and JSON request contract, validates pasted hosted callback
URLs and state, and applies the same JSON contract to refresh. Local HTTP tests
cover these requests; a fresh browser login is required for live acceptance.

GitHub push protection rejected the initial unpublished provider commit because
it contained bundled Gemini desktop OAuth client credentials. These defaults were
removed instead of bypassing protection. Hosts now provide the existing
`ROCI_GEMINI_OAUTH_CLIENT_ID` and `ROCI_GEMINI_OAUTH_CLIENT_SECRET` settings or use
`with_oauth_client`; missing settings fail before a network request. The local
values were preserved only in ignored `.env`, without replacing existing settings.

The user deferred Google Cloud project selection and native Google OAuth
verification. The repaired auth workspace suite passed 1772 tests (6 ignored),
with workspace Clippy and formatting checks passing. The fresh Claude browser login completed successfully for the default account.
Native catalog discovery also passed. A Sonnet generation request reached
Anthropic but returned a rate limit. A bounded Haiku request then succeeded:
`anthropic:claude-haiku-4-5-20251001` returned
`roci-contract-claude-native-ok`, exit 0, with the native default-account OAuth
credential. This verifies the repaired browser exchange, catalog, and generation
path. No successful Sonnet call is claimed.
