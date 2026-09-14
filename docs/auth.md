---
summary: "Manager-backed provider auth CLI for roci-agent"
read_when: "Working on roci-cli auth login/status/logout/configure/providers or provider credential flows"
---

# Provider auth CLI

`roci-agent auth` routes every login, status, logout, configure, and providers
command through Roci's host-facing `ProviderAuthManager`. The CLI does not
inspect token files, invent provider aliases, or print secret material.

## Credential storage

CLI and Andromeda share the same Roci-owned provider credential store. There is
no separate GUI key store and no default Keychain prompt path.

| Platform | Default API-key store | Notes |
| --- | --- | --- |
| Unix (macOS, Linux) | Plaintext `~/.roci/auth.json` | Directory mode `0700`; `auth.json` and `auth.json.lock` mode `0600`. Same-user processes can read the file. |
| Windows | OS credential manager (`OsProviderCredentialStore`) | File-backed storage waits on an ACL-safe design. |

Tradeoff: Unix defaults favor a shared, inspectable file that CLI and Andromeda
both read without Keychain prompts. The cost is plaintext exposure to anything
running as the same user. Mode bits reduce casual cross-user reads; they do not
protect against same-user malware or a compromised account.

Additional rules:

- JSON map keyed by canonical provider id; values use the versioned credential
  record schema (`version`, `api_key`, optional `endpoint`).
- Existing OAuth `FileTokenStore` TOMLs under `~/.roci/` stay on their own path
  this stage and are not migrated into `auth.json`.
- No automatic Keychain → file migration. API keys that only lived in Keychain
  must be reconfigured (`auth configure ... --api-key-stdin`).
- On Unix, `OsProviderCredentialStore` remains public for explicit host
  injection (for example an embedding app that wants Keychain). Default Unix
  `RociConfig` / `roci-agent` builds do not open Keychain for provider API keys.
  Windows continues to use the OS credential manager by default.
- Hosts that need a custom root still construct
  `FileProviderCredentialStore::new(root)` and inject it through
  `RociConfig::with_provider_credential_store`. Production defaults resolve
  `~/.roci` from the process home (`HOME` / `directories`); there is no extra
  `ROCI_HOME` override in this stage.

## Commands

### Login

```bash
roci-agent auth login <provider>
```

Starts the manager login flow for a known provider alias or canonical key
(`github-copilot`, `codex`, `anthropic`, ...).

- Device-code: prints verification URL + user code, polls with opaque session
  ids, respects `SlowDown` interval updates, and fails cleanly on denial/expiry.
- PKCE: prints the authorization URL and receives the browser callback for
  supported loopback redirects, with a pasted URL/code fallback. It never logs the code. The `>` prompt appears only when both stdin and stdout
  are TTYs.
- Normal login creates a Roci-owned session. Existing Codex/Claude CLI credentials
  are read only by the explicit `auth import <provider>` command. Imported rotating
  refresh tokens remain shared with their original CLI; prefer independent login.
- Pending login sessions are single-flight. Concurrent use of one session id is
  rejected; cancellation and retryable provider responses release it for retry,
  while success and terminal responses consume it.

Primary output goes to stdout. Diagnostics go to stderr. Handlers return
actionable errors to `main` (no `process::exit` inside auth handlers).

### Status

```bash
roci-agent auth status
roci-agent auth status --json
```

Lists secret-free `ProviderAuthStatus` rows from the manager. `--json` prints a
stable JSON array (same shape as `auth providers --json`).

### Providers

```bash
roci-agent auth providers
roci-agent auth providers --json
```

Lists known launch-capable providers with secret-free auth state. JSON output is
a `ProviderAuthStatus` array and must never include API keys, tokens, or raw
session material.

### Configure API key

```bash
printf '%s\n' "$API_KEY" | roci-agent auth configure <provider> --api-key-stdin
printf '%s\n' "$API_KEY" | roci-agent auth configure openai-compatible \
  --endpoint http://framed:4001/v1 --api-key-stdin
```

Requirements:

- `--api-key-stdin` is required.
- There is no `--api-key <value>` flag; keys must not appear on argv.
- The key is read from stdin as bounded UTF-8 (maximum 16 KiB). Only trailing CR/LF are stripped.
- TTY stdin is rejected so keys are not typed into interactive shells by accident.
- Empty, invalid UTF-8, or oversized stdin fails with a clean error.
- Success prints only `Configured <provider>` (no endpoint, no key).
- Default `roci-agent` builds include the OpenAI-compatible factory, so the Framed command works without extra feature flags.
- On Unix, configure writes the versioned record into `~/.roci/auth.json` (not Keychain).

### Logout

```bash
roci-agent auth logout <provider>
```

Clears Roci-owned OAuth/token and protected API-key records for the provider
through the manager. External environment/in-process configuration is left alone
and may still appear as `ExternallyConfigured` in status. Success prints only
`Logged out from <provider>`.

## Named accounts and saved sessions

```bash
roci-agent auth --account work login codex --flow pkce
roci-agent auth --account work login anthropic
roci-agent auth --account work import codex
roci-agent auth --account work status
roci-agent models --account work list --provider codex
roci-agent chat --account work --model codex:gpt-5.4 "Hello"
```

Accounts are explicit namespaces per provider, with `default` retained for
existing credentials. Names contain 1–64 lowercase letters, digits or hyphens
and must contain a letter or digit. OAuth uses `<provider>.<account>.toml` for
named accounts; API-key records use `<provider>@<account>`. Login, import,
configure, logout, status, model listing and execution use the same namespace.
Missing named credentials never fall back to `default`. Selecting a different
account clears inherited environment/in-process API keys; an SDK host can set
an intentional override after selection.

SDK hosts select an account with `RociConfig::with_account("work")?` and build
`AuthService` from that configuration's `token_store()`. Saved sessions record
`CreateSessionOptions::credential_account`; the SDK rejects account mismatches
on resume. The CLI inherits the saved account when `--account` is omitted.
Legacy sessions use `default`. There is no automatic account rotation.

## Runtime credential renewal

Providers receive typed API-key or OAuth credentials, preserving the required
wire authentication scheme. Refreshable OAuth sessions renew before expiry and
on one HTTP 401 response. A stream is retried only before exposing its first
event; HTTP 403 and errors after output are not replayed. Copilot's primary
GitHub credential and short-lived API token remain separate.

OAuth writes use private files, atomic replacement, cross-process refresh
leases and conditional publication. Logout or a newer login cannot be
replaced by a stale refresh result. Custom stores must implement distributed
leases and atomic conditional publication if shared across processes.

Status shows `Refresh needed` for expired refreshable tokens, `Login required`
for expired nonrefreshable credentials, and propagates storage errors. Status
reports locally known state; a revoked unexpired token is detected on use.

## Login choices

| Provider | Flow | Native execution |
| --- | --- | --- |
| Codex | Device code (default), `--flow pkce` browser | ChatGPT Responses |
| Anthropic Claude Code | PKCE browser | Messages with OAuth bearer/beta headers |
| GitHub Copilot | Device code | GitHub token exchange and discovered API endpoint |
| xAI (`grok`, `xai`) | Device code | Grok CLI Responses transport |
| Cursor | Browser polling | HTTP/2 Connect/Protobuf AgentService |
| Gemini (`google`, `gemini`) | PKCE browser | Cloud Code Assist |

Codex browser login binds `127.0.0.1:1455` before showing the URL. A pasted full
callback URL is available when loopback hosting is unavailable. SDK hosts own
browser and callback UI; credentials, verifier and state remain in the pending
login manager. `auth import codex` and `auth import anthropic` are separate from
all normal login flows.

## Wiring

Production commands build one `ProviderAuthManager` with:

- one `Arc<FileTokenStore>` injected into `AuthService`; `ProviderAuthManager`
  makes that store `RociConfig`'s OAuth source so login, status, and launch cannot
  diverge even if callers supplied a different config token store
- `roci::default_registry()` / `roci::default_auth_service(...)`
- `RociConfig::from_env()` for explicit/environment values
- platform default provider credential store:
  - Unix: locked `FileProviderCredentialStore` at `~/.roci/auth.json`
  - non-Unix: `OsProviderCredentialStore`
- provider constructors resolve API key and endpoint from one credential snapshot;
  compatible-provider fallback selects one complete dedicated or inherited pair
  instead of mixing partial sources
- protected provider-store read failures stop credential fallback; OAuth aliases
  are consulted only when the provider store successfully reports no record

Hosts advance browser and device-code login with
`ProviderAuthManager::advance_login(&session_id)`. The manager selects the
protocol from its pending session; callers do not provide a flow selector or
secret session material. PKCE instead uses `complete_pkce(&session_id, code)`.
These operations share exclusive session ownership: cancellation releases the
claim, retryable failures preserve the session, and terminal results consume it.

Provider adapters implement one `AuthBackend::complete_pkce` operation accepting
the authorization code, state, and required opaque session data returned by
`start_login`. `AuthService` forwards that same contract. There is no sessionless
PKCE completion or optional session-data fallback; the manager retains the verifier
and supplies it to the adapter.

## Native Cursor login and execution

`roci-agent auth login cursor` opens a browser-polling authorization flow. Visit
the printed URL and authorize Cursor in the browser; no response code needs to
be pasted. The SDK keeps the PKCE verifier in its pending session and polls
Cursor until authorization completes or the session expires. Tokens are stored
through the same account-scoped OAuth store used at launch, and refreshed by
the SDK credential lifecycle. For a separate credential namespace, use
`roci-agent auth --account work login cursor` and `chat --account work`.

Cursor runs through its native HTTP/2 Connect/Protobuf AgentService. Use
`roci-agent models list --provider cursor` to obtain model IDs, then
`roci-agent chat --model cursor:<model-id> "Hello"`. Text, streaming reasoning,
and SDK function tools are supported. Cursor's native service has no supported
mapping for the SDK's general sampling controls: temperature, max-token caps,
structured output, and other unsupported generation settings produce explicit
errors. Image and opaque/redacted-thinking inputs are also rejected. Omit
`--temperature` and `--max-tokens` from native Cursor smoke commands.

Cursor model families follow [CLIProxyAPIPlus PR #235](https://github.com/kaitranntt/CLIProxyAPIPlus/pull/235).
The account's advertised variants are coalesced into family IDs with typed
reasoning and speed capabilities. Original variant IDs remain available.
The CLI lists families by default. Add `--include-variants` to show the original
upstream IDs alongside families, in either text or JSON output:

```bash
roci-agent models --account live-test list --provider cursor
roci-agent models --account live-test list --provider cursor --include-variants --json
roci-agent chat --account live-test --no-skills --no-tools \
  --model cursor:claude-4.6-sonnet --reasoning-effort medium "Hello"
roci-agent chat --account live-test --no-skills --no-tools \
  --model cursor:gpt-5.6-terra --reasoning-effort high --speed fast "Hello"
```

The SDK exposes `GenerationSettings.reasoning_effort` and
`GenerationSettings.speed: Option<GenerationSpeed>` (`Standard` / `Fast`).
Model catalog capabilities publish `reasoning_effort.supported` and
`supported_speeds`; Cursor metadata includes `cursor_family`,
`cursor_family_id`, `cursor_explicit_variant`, and the original `cursor_variants`.
Options are derived from the selected account, and unavailable combinations
fail without silently choosing another speed, effort, or account. Resolution
happens before checkpoint lookup, using the actual upstream variant as the
state key. Family lookup uses the request's selected credential.

With no controls, an advertised exact base wins; otherwise resolution prefers
thinking when present, medium then high effort, and standard speed. Effort
`none` disables thinking. Composer-style families without thinking or effort
variants ignore harness reasoning defaults. Exact variant IDs preserve their
meaning and ignore family selection controls. A recognized parenthetical
suffix such as `claude-4.6-sonnet(medium)` overrides body reasoning controls.
Thinking-only families accept SDK `ThinkingMode::Enabled` without interpreting
its budget as a nonexistent named effort level.

Codex and OpenAI Responses use the same speed setting: `Fast` sends
`service_tier: "priority"`, `Standard` sends `"default"`, and omission retains
the provider default. Existing `openai_responses.service_tier` remains available;
conflicting typed speed and explicit service tier are rejected. Speed is
independent of reasoning effort and is a per-request CLI option. Agent execution
rejects speed selection for providers that do not advertise support.

```bash
roci-agent chat --model codex:gpt-5.4 --reasoning-effort high --speed fast "Hello"
```

For a durable SDK session, Cursor stores upstream checkpoints and their blobs
under `~/.roci/cursor/sessions`. The SDK owns this state; hosts can inject
`CursorSessionStore` through `CursorProvider::with_session_store`. The file
implementation creates Unix directories/files as 0700/0600, commits atomically,
and rejects concurrent use of one session across processes. State is isolated
by named credential account, upstream account identity, model, endpoint, and
SDK session ID. There is no rotating-access-token fallback for account identity.
On other platforms, hosts must inject a protected state store for durable use.

A completed checkpoint is reused only when the saved conversation prefix and
assistant response match the next SDK request, followed by a new text user
turn. After process/provider recreation, the SDK restores both the checkpoint
and its referenced blobs. Edited/compacted history, incomplete turns, or absent
compatible checkpoints use the complete SDK history as a new conversation.
Corrupt or inaccessible state is reported instead of silently discarded.

Cursor tool execution callbacks are tied to a live HTTP/2 stream. The currently
verified protocol cannot reconnect an in-flight execution ID to a new stream.
After returning a tool call to the SDK, the next request therefore includes the
full call/result history in a new conversation; it does not claim to resume the
old execution callback. Already completed tool IDs with identical arguments
receive their saved result without another SDK execution. Direct upstream
filesystem, shell, and network operations are rejected; the host's SDK tools
remain responsible for executing actions.

## Security rules

- Primary result text on stdout; diagnostics on stderr.
- Secrets only via stdin, never argv/env flags/output/errors.
- Stable `--json` payloads are secret-free serializations of manager types.
- Prompts only when interactive TTYs require them.
- Unix `auth.json` is same-user readable by design; treat filesystem access as
  the trust boundary, not Keychain ACLs.

## Native Gemini browser login and execution

`roci-agent auth login google` starts Google's installed-application PKCE flow.
Visit the printed URL, sign in, then paste the code shown by Google's
`codeassist.google.com/authcode` page. A complete callback URL is accepted only
when its destination and state match the pending session. Each login has a fresh
state and S256 verifier; the verifier remains inside the SDK's pending session.
The CLI does not need to open a local callback listener for this manual-code flow.

Hosts must configure an installed-application OAuth client through
`GeminiAuth::with_oauth_client` or both `ROCI_GEMINI_OAUTH_CLIENT_ID` and
`ROCI_GEMINI_OAUTH_CLIENT_SECRET`. The SDK does not bundle client credentials.
Export the variables in the launching environment or load the ignored `.env`
in the host process; the CLI does not load `.env` automatically. These settings
are required for browser login and refresh, but not API-key execution. Set
`GOOGLE_CLOUD_PROJECT` to select an existing Cloud project when required by the
account. Otherwise the SDK discovers or provisions the Code Assist project with
`loadCodeAssist` and `onboardUser`. Login is persisted only after project setup
succeeds. The project ID lives in typed OAuth provider metadata and survives
refresh; it is redacted from token debug output.

When `google` resolves to OAuth credentials, model execution uses the native
Cloud Code Assist `v1internal:generateContent` and `streamGenerateContent`
endpoints. API-key credentials continue to use Google's Gemini API. The native
OAuth transport supports SDK messages, function tools, reasoning, generation
settings, and streaming usage. It shares Gemini request conversion with the
API-key provider and keeps response tool IDs and thought signatures.

For a separate credential account, use
`roci-agent auth --account personal login google`, then
`roci-agent chat --account personal --model google:gemini-2.5-flash "Hello"`.
OAuth tokens use the `gemini` store key under the selected account. Refresh runs
through the SDK request lifecycle and preserves rotating refresh tokens and
project metadata. Hermetic tests cover auth, onboarding, and native transport;
a successful live account call is still required to verify an installation.
