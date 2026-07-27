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
- PKCE: prints the authorization URL, reads the response code from stdin, and
  never logs the code. The `>` prompt appears only when both stdin and stdout
  are TTYs.
- Imported credentials complete immediately without a browser step.

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

## Wiring

Production commands build one `ProviderAuthManager` with:

- the same `Arc<FileTokenStore>` injected into `AuthService` and `RociConfig`
- `roci::default_registry()` / `roci::default_auth_service(...)`
- `RociConfig::from_env()` for explicit/environment values
- platform default provider credential store:
  - Unix: locked `FileProviderCredentialStore` at `~/.roci/auth.json`
  - non-Unix: `OsProviderCredentialStore`

## Security rules

- Primary result text on stdout; diagnostics on stderr.
- Secrets only via stdin, never argv/env flags/output/errors.
- Stable `--json` payloads are secret-free serializations of manager types.
- Prompts only when interactive TTYs require them.
- Unix `auth.json` is same-user readable by design; treat filesystem access as
  the trust boundary, not Keychain ACLs.
