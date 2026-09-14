//! Manager-backed CLI auth commands (login/status/logout/configure/providers).
//!
//! All credential work goes through [`roci::auth::ProviderAuthManager`]. Secrets
//! stay off argv, stdout, stderr, and error messages.

use std::fmt;
use std::future::Future;
use std::io::{self, IsTerminal, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use roci::auth::{
    AuthError, CredentialFlow, FileTokenStore, HostAuthPollResult, HostAuthStep, LoginSessionId,
    ProviderApiKey, ProviderAuthManager, ProviderAuthState, ProviderAuthStatus, ProviderEndpoint,
    TokenStore,
};
use roci::config::RociConfig;

const MAX_SECRET_INPUT_BYTES: usize = 16 * 1024;

/// CLI-local port over host auth manager operations used by auth commands.
///
/// Futures returned by async methods are explicitly `Send` so handlers can run
/// on a multi-threaded Tokio runtime. Keep this trait crate-private; production
/// code uses [`ProviderAuthManager`], while tests inject a fake.
pub(crate) trait AuthManagerPort: Send + Sync {
    /// Explicitly import external credentials and expose only the canonical provider.
    fn import_credentials(
        &self,
        provider: &str,
    ) -> Result<roci::auth::HostAuthCompletion, AuthError>;

    /// Persist an API key and optional endpoint for a known provider.
    fn configure_api_key(
        &self,
        provider: &str,
        api_key: ProviderApiKey,
        endpoint: Option<ProviderEndpoint>,
    ) -> Result<(), AuthError>;

    /// Project status for every known canonical provider.
    fn list_statuses(&self) -> Vec<ProviderAuthStatus>;

    /// Clear Roci-owned credentials for a provider.
    fn logout(&self, provider: &str) -> Result<(), AuthError>;

    /// Start a login flow; pending secrets stay inside the manager.
    fn start_login(
        &self,
        provider: &str,
    ) -> impl Future<Output = Result<HostAuthStep, AuthError>> + Send;

    fn start_login_with_flow(
        &self,
        provider: &str,
        _flow: CredentialFlow,
    ) -> impl Future<Output = Result<HostAuthStep, AuthError>> + Send {
        self.start_login(provider)
    }

    /// Advance a polling login without exposing provider session material.
    fn advance_login(
        &self,
        session_id: &LoginSessionId,
    ) -> impl Future<Output = Result<HostAuthPollResult, AuthError>> + Send;

    /// Complete a PKCE login by opaque session id + authorization code.
    fn complete_pkce(
        &self,
        session_id: &LoginSessionId,
        code: &str,
    ) -> impl Future<Output = Result<roci::auth::HostAuthCompletion, AuthError>> + Send;
}

impl AuthManagerPort for ProviderAuthManager {
    fn start_login_with_flow(
        &self,
        provider: &str,
        flow: CredentialFlow,
    ) -> impl Future<Output = Result<HostAuthStep, AuthError>> + Send {
        ProviderAuthManager::start_login_with_flow(self, provider, flow)
    }
    fn import_credentials(
        &self,
        provider: &str,
    ) -> Result<roci::auth::HostAuthCompletion, AuthError> {
        ProviderAuthManager::import_credentials(self, provider)
    }

    fn configure_api_key(
        &self,
        provider: &str,
        api_key: ProviderApiKey,
        endpoint: Option<ProviderEndpoint>,
    ) -> Result<(), AuthError> {
        ProviderAuthManager::configure_api_key(self, provider, api_key, endpoint)
    }

    fn list_statuses(&self) -> Vec<ProviderAuthStatus> {
        ProviderAuthManager::list_statuses(self)
    }

    fn logout(&self, provider: &str) -> Result<(), AuthError> {
        ProviderAuthManager::logout(self, provider)
    }

    fn start_login(
        &self,
        provider: &str,
    ) -> impl Future<Output = Result<HostAuthStep, AuthError>> + Send {
        ProviderAuthManager::start_login(self, provider)
    }

    fn advance_login(
        &self,
        session_id: &LoginSessionId,
    ) -> impl Future<Output = Result<HostAuthPollResult, AuthError>> + Send {
        ProviderAuthManager::advance_login(self, session_id)
    }

    fn complete_pkce(
        &self,
        session_id: &LoginSessionId,
        code: &str,
    ) -> impl Future<Output = Result<roci::auth::HostAuthCompletion, AuthError>> + Send {
        ProviderAuthManager::complete_pkce(self, session_id, code)
    }
}

/// Actionable CLI auth failure (no process::exit inside handlers).
#[derive(Debug)]
pub(crate) enum AuthCliError {
    Auth(AuthError),
    Io(io::Error),
    Message(String),
}

impl fmt::Display for AuthCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(err) => write!(f, "{err}"),
            Self::Io(err) => write!(f, "{err}"),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AuthCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Auth(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::Message(_) => None,
        }
    }
}

impl From<AuthError> for AuthCliError {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<io::Error> for AuthCliError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for AuthCliError {
    fn from(value: serde_json::Error) -> Self {
        Self::Message(value.to_string())
    }
}

/// Build production manager: shared file token store, env config, default credentials.
fn build_manager(account: &str) -> Result<ProviderAuthManager, AuthCliError> {
    let store: Arc<FileTokenStore> = Arc::new(FileTokenStore::new_default());
    let token_store: Arc<dyn TokenStore> = store.clone();
    let registry = roci::default_registry();
    // Keep the shared FileTokenStore override; provider credentials use the
    // platform RociConfig default (Unix auth.json / non-Unix OS store).
    let config = RociConfig::from_env()
        .with_token_store(Some(token_store))
        .with_account(account)?;
    let scoped_store = config
        .token_store()
        .cloned()
        .ok_or_else(|| AuthError::InvalidResponse("missing configured token store".into()))?;
    let auth = roci::default_auth_service(scoped_store);
    ProviderAuthManager::new(auth, registry, config).map_err(AuthCliError::from)
}

/// Handle `roci-agent auth login <provider>`.
pub async fn handle_login(
    provider: &str,
    account: &str,
    flow: Option<CredentialFlow>,
) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    run_login_with_flow(
        &manager,
        provider,
        &mut stdin,
        &mut stdout,
        &mut stderr,
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        flow,
    )
    .await
    .map_err(Into::into)
}

/// Handle `roci-agent auth import <provider>` without starting an OAuth flow.
pub async fn handle_import(
    provider: &str,
    account: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    run_import(&manager, provider, &mut io::stdout()).map_err(Into::into)
}

pub(crate) fn run_import<M: AuthManagerPort, W: Write>(
    manager: &M,
    provider: &str,
    stdout: &mut W,
) -> Result<(), AuthCliError> {
    let completion = manager.import_credentials(provider)?;
    writeln!(
        stdout,
        "Imported existing credentials for {}",
        completion.provider
    )?;
    Ok(())
}

/// Handle `roci-agent auth status [--json]`.
pub async fn handle_status(json: bool, account: &str) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    let mut stdout = io::stdout();
    run_status(&manager, json, &mut stdout).map_err(Into::into)
}

/// Handle `roci-agent auth logout <provider>`.
pub async fn handle_logout(
    provider: &str,
    account: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    let mut stdout = io::stdout();
    run_logout(&manager, provider, &mut stdout).map_err(Into::into)
}

/// Handle `roci-agent auth configure <provider> [--endpoint] --api-key-stdin`.
pub async fn handle_configure(
    provider: &str,
    endpoint: Option<&str>,
    account: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    run_configure(
        &manager,
        provider,
        endpoint,
        &mut stdin,
        &mut stdout,
        io::stdin().is_terminal(),
    )
    .map_err(Into::into)
}

/// Handle `roci-agent auth providers [--json]`.
pub async fn handle_providers(json: bool, account: &str) -> Result<(), Box<dyn std::error::Error>> {
    let manager = build_manager(account)?;
    let mut stdout = io::stdout();
    run_providers(&manager, json, &mut stdout).map_err(Into::into)
}

#[cfg(test)]
pub(crate) async fn run_login<M, R, W, E>(
    manager: &M,
    provider: &str,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    R: Read,
    W: Write,
    E: Write,
{
    run_login_with_flow(
        manager,
        provider,
        stdin,
        stdout,
        stderr,
        stdin_is_tty,
        stdout_is_tty,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_login_with_flow<M, R, W, E>(
    manager: &M,
    provider: &str,
    stdin: &mut R,
    stdout: &mut W,
    stderr: &mut E,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
    flow: Option<CredentialFlow>,
) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    R: Read,
    W: Write,
    E: Write,
{
    let step = match flow {
        Some(flow) => manager.start_login_with_flow(provider, flow).await?,
        None => manager.start_login(provider).await?,
    };
    match step {
        HostAuthStep::ImportedAndComplete {
            provider: canonical,
        } => {
            writeln!(stdout, "Imported existing credentials for {canonical}")?;
        }
        HostAuthStep::DeviceCode {
            verification_uri,
            user_code,
            interval_secs,
            expires_at,
            session_id,
        } => {
            writeln!(stdout, "Visit: {verification_uri}")?;
            writeln!(stdout, "Enter code: {user_code}")?;
            writeln!(stdout, "Waiting for authorization...")?;
            stdout.flush()?;

            poll_login(
                interval_secs,
                expires_at,
                "Device code expired; please try again",
                stdout,
                stderr,
                || manager.advance_login(&session_id),
            )
            .await?;
        }
        HostAuthStep::BrowserPoll {
            authorization_url,
            interval_secs,
            expires_at,
            session_id,
        } => {
            writeln!(stdout, "Visit: {authorization_url}")?;
            writeln!(stdout, "Waiting for authorization...")?;
            stdout.flush()?;
            poll_login(
                interval_secs,
                expires_at,
                "Browser login expired; please try again",
                stdout,
                stderr,
                || manager.advance_login(&session_id),
            )
            .await?;
        }
        HostAuthStep::Pkce {
            authorization_url,
            session_id,
        } => {
            let receiver = if stdin_is_tty {
                match super::auth_callback::LoopbackCallback::bind(&authorization_url).await {
                    Ok(receiver) => receiver,
                    Err(_) => {
                        writeln!(stderr, "Local callback unavailable; paste the complete redirected URL to finish.")?;
                        None
                    }
                }
            } else {
                None
            };
            writeln!(stdout, "Visit: {authorization_url}")?;
            let received = match receiver {
                Some(receiver) => {
                    writeln!(stdout, "Waiting for the browser callback...")?;
                    stdout.flush()?;
                    match receiver.receive().await {
                        Ok(callback) => Some(callback),
                        Err(_) => {
                            writeln!(
                                stderr,
                                "No browser callback received; paste the complete redirected URL."
                            )?;
                            None
                        }
                    }
                }
                None => None,
            };
            if received.is_none() {
                writeln!(
                    stdout,
                    "After authorizing, paste the response code or complete redirected URL below:"
                )?;
            }
            if stdin_is_tty && stdout_is_tty {
                write!(stdout, "> ")?;
                stdout.flush()?;
            } else {
                stdout.flush()?;
            }

            let code = match received {
                Some(callback) => callback,
                None => read_secret_line(stdin)?,
            };
            if code.is_empty() {
                return Err(AuthCliError::Message(
                    "No authorization code provided".into(),
                ));
            }

            let completion = manager.complete_pkce(&session_id, &code).await?;
            writeln!(stdout, "{} login successful!", completion.provider)?;
        }
    }
    Ok(())
}

/// Both polling flows share timing and terminal outcomes, but dispatch through
/// their own opaque manager operation.
async fn poll_login<W, E, P, F>(
    interval_secs: u64,
    expires_at: chrono::DateTime<chrono::Utc>,
    expiry_message: &str,
    stdout: &mut W,
    stderr: &mut E,
    mut poll: P,
) -> Result<(), AuthCliError>
where
    W: Write,
    E: Write,
    P: FnMut() -> F,
    F: Future<Output = Result<HostAuthPollResult, AuthError>>,
{
    let remaining = expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .unwrap_or_default();
    let deadline = tokio::time::Instant::now() + remaining;
    let mut interval = Duration::from_secs(interval_secs.max(1));
    loop {
        tokio::time::sleep_until((tokio::time::Instant::now() + interval).min(deadline)).await;
        if tokio::time::Instant::now() >= deadline {
            writeln!(stderr, "{expiry_message}")?;
            return Err(AuthCliError::Message(expiry_message.into()));
        }
        match poll().await? {
            HostAuthPollResult::Pending => {}
            HostAuthPollResult::SlowDown { interval_secs } => {
                interval = Duration::from_secs(interval_secs.max(1));
            }
            HostAuthPollResult::Authorized { provider } => {
                writeln!(stdout, "{provider} login successful!")?;
                return Ok(());
            }
            HostAuthPollResult::Denied => {
                writeln!(stderr, "Authorization denied")?;
                return Err(AuthCliError::Message("Authorization denied".into()));
            }
            HostAuthPollResult::Expired => {
                writeln!(stderr, "{expiry_message}")?;
                return Err(AuthCliError::Message(expiry_message.into()));
            }
        }
    }
}

pub(crate) fn run_status<M, W>(manager: &M, json: bool, stdout: &mut W) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    W: Write,
{
    write_statuses(manager.list_statuses(), json, stdout)
}

pub(crate) fn run_providers<M, W>(
    manager: &M,
    json: bool,
    stdout: &mut W,
) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    W: Write,
{
    write_statuses(manager.list_statuses(), json, stdout)
}

pub(crate) fn run_logout<M, W>(
    manager: &M,
    provider: &str,
    stdout: &mut W,
) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    W: Write,
{
    manager.logout(provider)?;
    // Success names provider only — never endpoint or secret material.
    writeln!(stdout, "Logged out from {provider}")?;
    Ok(())
}

pub(crate) fn run_configure<M, R, W>(
    manager: &M,
    provider: &str,
    endpoint: Option<&str>,
    stdin: &mut R,
    stdout: &mut W,
    stdin_is_tty: bool,
) -> Result<(), AuthCliError>
where
    M: AuthManagerPort,
    R: Read,
    W: Write,
{
    if stdin_is_tty {
        return Err(AuthCliError::Message(
            "API key must be piped on stdin with --api-key-stdin (refusing TTY input)".into(),
        ));
    }

    let api_key = read_secret_line(stdin)?;
    if api_key.is_empty() {
        return Err(AuthCliError::Message("No API key provided on stdin".into()));
    }

    let endpoint = endpoint.map(ProviderEndpoint::new);
    manager.configure_api_key(provider, ProviderApiKey::new(api_key), endpoint)?;
    // Success names provider only — never endpoint or key.
    writeln!(stdout, "Configured {provider}")?;
    Ok(())
}

fn write_statuses<W: Write>(
    statuses: Vec<ProviderAuthStatus>,
    json: bool,
    stdout: &mut W,
) -> Result<(), AuthCliError> {
    if json {
        writeln!(stdout, "{}", serde_json::to_string_pretty(&statuses)?)?;
        return Ok(());
    }

    writeln!(stdout, "Authentication Status")?;
    writeln!(stdout)?;
    if statuses.is_empty() {
        writeln!(stdout, "  (no providers registered)")?;
        return Ok(());
    }

    for status in statuses {
        let key = &status.descriptor.canonical_key;
        let name = &status.descriptor.display_name;
        let state = match &status.auth_state {
            ProviderAuthState::RefreshNeeded => "Refresh needed".to_string(),
            ProviderAuthState::ReauthRequired => "Login required".to_string(),
            ProviderAuthState::SignedOut => "Signed out".to_string(),
            ProviderAuthState::ExternallyConfigured => "Externally configured".to_string(),
            ProviderAuthState::SignedIn { label } => label.clone(),
        };
        let launch = if status.launch_available {
            "available"
        } else {
            "unavailable"
        };
        writeln!(stdout, "  {name} ({key}): {state} [launch: {launch}]")?;
    }
    Ok(())
}

/// Read one bounded UTF-8 secret line; strip only its trailing CR/LF.
fn read_secret_line<R: Read>(stdin: &mut R) -> Result<String, AuthCliError> {
    let mut bytes = Vec::new();
    // Read one line so piped `printf '%s\n' key` works without consuming later input.
    let mut byte = [0u8; 1];
    loop {
        let read = stdin.read(&mut byte)?;
        if read == 0 || byte[0] == b'\n' {
            break;
        }
        if bytes.len() >= MAX_SECRET_INPUT_BYTES {
            return Err(AuthCliError::Message(format!(
                "Secret input exceeds {MAX_SECRET_INPUT_BYTES} bytes"
            )));
        }
        bytes.push(byte[0]);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map_err(|_| AuthCliError::Message("Secret input must be valid UTF-8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use roci::auth::{
        ConfiguredSource, CredentialFlow, HostAuthCompletion, ProviderAuthState, ProviderDescriptor,
    };

    const SENTINEL_SECRET: &str = "sk-sentinel-do-not-leak-ever";

    #[derive(Default)]
    struct FakeManager {
        import_results: Mutex<VecDeque<Result<HostAuthCompletion, AuthError>>>,
        import_calls: Mutex<Vec<String>>,
        configure_calls: Mutex<Vec<ConfigureCall>>,
        logout_calls: Mutex<Vec<String>>,
        list_calls: Mutex<u32>,
        statuses: Mutex<Vec<ProviderAuthStatus>>,
        login_steps: Mutex<VecDeque<Result<HostAuthStep, AuthError>>>,
        poll_results: Mutex<VecDeque<Result<HostAuthPollResult, AuthError>>>,
        pkce_codes: Mutex<Vec<String>>,
        advanced_sessions: Mutex<Vec<String>>,
    }

    #[derive(Debug, Clone)]
    struct ConfigureCall {
        provider: String,
        api_key: String,
        endpoint: Option<String>,
    }

    impl FakeManager {
        fn with_status(status: ProviderAuthStatus) -> Self {
            let fake = Self::default();
            fake.statuses.lock().unwrap().push(status);
            fake
        }
    }

    #[test]
    fn explicit_import_prints_receipt_without_starting_login() {
        let manager = FakeManager::default();
        manager
            .import_results
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthCompletion {
                provider: "codex".into(),
            }));
        let mut output = Vec::new();
        run_import(&manager, "openai-codex", &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Imported existing credentials for codex\n"
        );
        assert_eq!(*manager.import_calls.lock().unwrap(), ["openai-codex"]);
        assert!(manager.login_steps.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_import_does_not_print_success_or_fall_back_to_login() {
        let manager = FakeManager::default();
        manager
            .import_results
            .lock()
            .unwrap()
            .push_back(Err(AuthError::Io("cannot read source".into())));
        let mut output = Vec::new();
        assert!(run_import(&manager, "codex", &mut output).is_err());
        assert!(output.is_empty());
    }

    impl AuthManagerPort for FakeManager {
        fn import_credentials(&self, provider: &str) -> Result<HostAuthCompletion, AuthError> {
            self.import_calls.lock().unwrap().push(provider.into());
            self.import_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected import call")
        }
        fn configure_api_key(
            &self,
            provider: &str,
            api_key: ProviderApiKey,
            endpoint: Option<ProviderEndpoint>,
        ) -> Result<(), AuthError> {
            self.configure_calls.lock().unwrap().push(ConfigureCall {
                provider: provider.to_string(),
                api_key: api_key.expose_secret().to_string(),
                endpoint: endpoint.map(|value| value.as_str().to_string()),
            });
            Ok(())
        }

        fn list_statuses(&self) -> Vec<ProviderAuthStatus> {
            *self.list_calls.lock().unwrap() += 1;
            self.statuses.lock().unwrap().clone()
        }

        fn logout(&self, provider: &str) -> Result<(), AuthError> {
            self.logout_calls.lock().unwrap().push(provider.to_string());
            Ok(())
        }

        fn start_login(
            &self,
            _provider: &str,
        ) -> impl Future<Output = Result<HostAuthStep, AuthError>> + Send {
            let step = self
                .login_steps
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected start_login call");
            async move { step }
        }

        fn advance_login(
            &self,
            session_id: &LoginSessionId,
        ) -> impl Future<Output = Result<HostAuthPollResult, AuthError>> + Send {
            self.advanced_sessions
                .lock()
                .unwrap()
                .push(session_id.as_str().into());
            let result = self
                .poll_results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected advance_login call");
            async move { result }
        }

        fn complete_pkce(
            &self,
            _session_id: &LoginSessionId,
            code: &str,
        ) -> impl Future<Output = Result<HostAuthCompletion, AuthError>> + Send {
            self.pkce_codes.lock().unwrap().push(code.to_string());
            async move {
                Ok(HostAuthCompletion {
                    provider: "anthropic".into(),
                })
            }
        }
    }

    fn sample_status(key: &str, state: ProviderAuthState) -> ProviderAuthStatus {
        ProviderAuthStatus {
            descriptor: ProviderDescriptor::new(key, key, vec![CredentialFlow::ApiKey], true),
            auth_state: state,
            configured_sources: vec![ConfiguredSource::StoredApiKey],
            launch_available: true,
        }
    }

    #[test]
    fn configure_delegates_to_manager_and_redacts_secret_from_output() {
        let manager = FakeManager::default();
        let mut stdin = SENTINEL_SECRET.as_bytes();
        let mut stdout = Vec::new();

        run_configure(
            &manager,
            "openai-compatible",
            Some("http://framed:4001/v1"),
            &mut stdin,
            &mut stdout,
            false,
        )
        .unwrap();

        let calls = manager.configure_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].provider, "openai-compatible");
        assert_eq!(calls[0].api_key, SENTINEL_SECRET);
        assert_eq!(calls[0].endpoint.as_deref(), Some("http://framed:4001/v1"));

        let out = String::from_utf8(stdout).unwrap();
        assert_eq!(out, "Configured openai-compatible\n");
        assert!(!out.contains(SENTINEL_SECRET));
        assert!(!out.contains("framed"));
        assert!(!out.contains("http"));
    }

    #[test]
    fn configure_rejects_tty_and_empty_stdin() {
        let manager = FakeManager::default();
        let mut stdout = Vec::new();

        let tty_err = run_configure(
            &manager,
            "openai",
            None,
            &mut SENTINEL_SECRET.as_bytes(),
            &mut stdout,
            true,
        )
        .unwrap_err();
        assert!(tty_err.to_string().contains("TTY"));
        assert!(manager.configure_calls.lock().unwrap().is_empty());

        let empty_err = run_configure(
            &manager,
            "openai",
            None,
            &mut b"".as_slice(),
            &mut stdout,
            false,
        )
        .unwrap_err();
        assert!(empty_err.to_string().contains("No API key"));
        assert!(!empty_err.to_string().contains(SENTINEL_SECRET));
    }

    #[test]
    fn configure_trims_only_line_endings() {
        let manager = FakeManager::default();
        let mut stdin = b"  keep-leading\r\n".as_slice();
        let mut stdout = Vec::new();
        run_configure(&manager, "openai", None, &mut stdin, &mut stdout, false).unwrap();
        let key = manager.configure_calls.lock().unwrap()[0].api_key.clone();
        assert_eq!(key, "  keep-leading");
    }

    #[test]
    fn configure_preserves_utf8_and_interior_carriage_return() {
        let manager = FakeManager::default();
        let mut stdin = "clé\rvalue\n".as_bytes();
        let mut stdout = Vec::new();

        run_configure(
            &manager,
            "openai",
            /*endpoint*/ None,
            &mut stdin,
            &mut stdout,
            /*stdin_is_tty*/ false,
        )
        .unwrap();

        assert_eq!(
            manager.configure_calls.lock().unwrap()[0].api_key,
            "clé\rvalue"
        );
    }

    #[test]
    fn secret_input_rejects_invalid_utf8_without_echoing_bytes() {
        let mut input = [0xff].as_slice();

        let error = read_secret_line(&mut input).unwrap_err();

        assert_eq!(error.to_string(), "Secret input must be valid UTF-8");
    }

    #[test]
    fn configure_rejects_oversized_secret_without_delegating() {
        let manager = FakeManager::default();
        let input = vec![b'x'; MAX_SECRET_INPUT_BYTES + 1];
        let mut stdout = Vec::new();

        let error = run_configure(
            &manager,
            "openai",
            /*endpoint*/ None,
            &mut input.as_slice(),
            &mut stdout,
            /*stdin_is_tty*/ false,
        )
        .unwrap_err();

        assert!(error.to_string().contains("exceeds"));
        assert!(manager.configure_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn status_and_providers_delegate_and_serialize_manager_status() {
        let status = sample_status(
            "openai-compatible",
            ProviderAuthState::SignedIn {
                label: "Signed in".into(),
            },
        );
        let manager = FakeManager::with_status(status.clone());
        let mut stdout = Vec::new();

        run_status(&manager, true, &mut stdout).unwrap();
        assert_eq!(*manager.list_calls.lock().unwrap(), 1);
        let status_json: Vec<ProviderAuthStatus> = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(status_json, vec![status.clone()]);

        stdout.clear();
        run_providers(&manager, true, &mut stdout).unwrap();
        assert_eq!(*manager.list_calls.lock().unwrap(), 2);
        let providers_json: Vec<ProviderAuthStatus> = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(providers_json, vec![status]);
    }

    #[test]
    fn status_human_output_names_provider_state_and_availability() {
        let manager = FakeManager::with_status(sample_status(
            "anthropic",
            ProviderAuthState::SignedIn {
                label: "Signed in".into(),
            },
        ));
        let mut stdout = Vec::new();
        run_status(&manager, false, &mut stdout).unwrap();
        let text = String::from_utf8(stdout).unwrap();
        assert_eq!(
            text,
            "Authentication Status\n\n  anthropic (anthropic): Signed in [launch: available]\n"
        );
    }

    #[test]
    fn logout_delegates_and_names_provider_only() {
        let manager = FakeManager::default();
        let mut stdout = Vec::new();
        run_logout(&manager, "codex", &mut stdout).unwrap();
        assert_eq!(
            manager.logout_calls.lock().unwrap().as_slice(),
            ["codex".to_string()]
        );
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Logged out from codex\n"
        );
    }

    #[tokio::test]
    async fn login_device_denied_and_expired_return_actionable_errors() {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::DeviceCode {
                verification_uri: "https://example.com/device".into(),
                user_code: "ABCD".into(),
                interval_secs: 0,
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                session_id: LoginSessionId::new("sess-denied"),
            }));
        manager
            .poll_results
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthPollResult::Denied));

        let mut stdin = b"".as_slice();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_login(
            &manager,
            "codex",
            &mut stdin,
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), "Authorization denied");
        assert!(String::from_utf8(stderr).unwrap().contains("denied"));

        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::DeviceCode {
                verification_uri: "https://example.com/device".into(),
                user_code: "ABCD".into(),
                interval_secs: 0,
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                session_id: LoginSessionId::new("sess-expired"),
            }));
        manager
            .poll_results
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthPollResult::Expired));
        let mut stderr = Vec::new();
        let err = run_login(
            &manager,
            "codex",
            &mut stdin,
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("expired"));
    }

    #[tokio::test]
    async fn login_device_deadline_expires_before_polling() {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::DeviceCode {
                verification_uri: "https://example.com/device".into(),
                user_code: "ABCD".into(),
                interval_secs: 30,
                expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
                session_id: LoginSessionId::new("sess-deadline"),
            }));
        manager
            .poll_results
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthPollResult::Pending));

        let mut stdin = b"".as_slice();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = run_login(
            &manager,
            "codex",
            &mut stdin,
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("expired"));
        assert_eq!(manager.poll_results.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn login_device_slow_down_updates_interval_then_authorizes() {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::DeviceCode {
                verification_uri: "https://example.com/device".into(),
                user_code: "ABCD".into(),
                interval_secs: 0,
                expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
                session_id: LoginSessionId::new("sess-slow"),
            }));
        {
            let mut polls = manager.poll_results.lock().unwrap();
            polls.push_back(Ok(HostAuthPollResult::SlowDown { interval_secs: 5 }));
            polls.push_back(Ok(HostAuthPollResult::Authorized {
                provider: "codex".into(),
            }));
        }

        let mut stdin = b"".as_slice();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let started = tokio::time::Instant::now();
        run_login(
            &manager,
            "codex",
            &mut stdin,
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(6));
        let out = String::from_utf8(stdout).unwrap();
        assert!(out.contains("codex login successful"));
        assert!(manager.poll_results.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn login_pkce_reads_code_from_stdin_without_logging_it() {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::Pkce {
                authorization_url: "https://example.com/auth".into(),
                session_id: LoginSessionId::new("sess-pkce"),
            }));

        let stdin = format!("{SENTINEL_SECRET}\n").into_bytes();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run_login(
            &manager,
            "anthropic",
            &mut stdin.as_slice(),
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap();

        assert_eq!(
            manager.pkce_codes.lock().unwrap().as_slice(),
            [SENTINEL_SECRET.to_string()]
        );
        let out = String::from_utf8(stdout).unwrap();
        let err = String::from_utf8(stderr).unwrap();
        assert!(out.contains("anthropic login successful"));
        assert!(!out.contains(SENTINEL_SECRET));
        assert!(!err.contains(SENTINEL_SECRET));
        assert!(!out.contains("> "));
    }

    #[tokio::test]
    async fn login_pkce_prompts_only_on_tty() {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::Pkce {
                authorization_url: "https://example.com/auth".into(),
                session_id: LoginSessionId::new("sess-pkce-tty"),
            }));

        let mut stdin = b"code-value\n".as_slice();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run_login(
            &manager,
            "anthropic",
            &mut stdin,
            &mut stdout,
            &mut stderr,
            true,
            true,
        )
        .await
        .unwrap();
        assert!(String::from_utf8(stdout).unwrap().contains("> "));
    }

    #[cfg(feature = "openai-compatible")]
    #[test]
    fn default_registry_registers_openai_compatible() {
        let registry = roci::default_registry();

        assert!(registry.has_provider("openai-compatible"));
    }

    #[test]
    fn providers_json_object_equality_snapshot() {
        let status = ProviderAuthStatus {
            descriptor: ProviderDescriptor::new(
                "openai-compatible",
                "OpenAI Compatible",
                vec![CredentialFlow::ApiKey],
                true,
            ),
            auth_state: ProviderAuthState::SignedOut,
            configured_sources: vec![],
            launch_available: false,
        };
        let manager = FakeManager::with_status(status);
        let mut stdout = Vec::new();
        run_providers(&manager, true, &mut stdout).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let expected = serde_json::json!([{
            "descriptor": {
                "canonical_key": "openai-compatible",
                "display_name": "OpenAI Compatible",
                "credential_flows": ["api_key"],
                "endpoint_configurable": true
            },
            "auth_state": { "kind": "signed_out" },
            "configured_sources": [],
            "launch_available": false
        }]);
        assert_eq!(value, expected);
    }

    struct NoCodeInput;

    impl Read for NoCodeInput {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            panic!("browser polling must not request a code");
        }
    }

    fn browser_manager(expires_at: chrono::DateTime<chrono::Utc>) -> FakeManager {
        let manager = FakeManager::default();
        manager
            .login_steps
            .lock()
            .unwrap()
            .push_back(Ok(HostAuthStep::BrowserPoll {
                authorization_url: "https://example.com/cursor/login".into(),
                interval_secs: 1,
                expires_at,
                session_id: LoginSessionId::new("opaque-browser-session"),
            }));
        manager
    }

    #[tokio::test(start_paused = true)]
    async fn browser_login_displays_url_and_polls_without_code_entry() {
        let manager = browser_manager(chrono::Utc::now() + chrono::Duration::minutes(5));
        manager.poll_results.lock().unwrap().extend([
            Ok(HostAuthPollResult::Pending),
            Ok(HostAuthPollResult::SlowDown { interval_secs: 4 }),
            Ok(HostAuthPollResult::Authorized {
                provider: "cursor".into(),
            }),
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let started = tokio::time::Instant::now();
        run_login(
            &manager,
            "cursor",
            &mut NoCodeInput,
            &mut stdout,
            &mut stderr,
            true,
            true,
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), Duration::from_secs(6));
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains("Visit: https://example.com/cursor/login"));
        assert!(output.contains("Waiting for authorization"));
        assert!(output.contains("cursor login successful!"));
        assert!(!output.contains("code"));
        assert!(!output.contains("opaque-browser-session"));
        assert!(stderr.is_empty());
        assert_eq!(
            *manager.advanced_sessions.lock().unwrap(),
            vec!["opaque-browser-session"; 3]
        );
        assert!(manager.pkce_codes.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn browser_login_reports_denial_and_expiry() {
        for result in [HostAuthPollResult::Denied, HostAuthPollResult::Expired] {
            let manager = browser_manager(chrono::Utc::now() + chrono::Duration::minutes(5));
            let expected = if matches!(result, HostAuthPollResult::Denied) {
                "Authorization denied"
            } else {
                "Browser login expired; please try again"
            };
            manager.poll_results.lock().unwrap().push_back(Ok(result));
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let error = run_login(
                &manager,
                "cursor",
                &mut NoCodeInput,
                &mut stdout,
                &mut stderr,
                false,
                false,
            )
            .await
            .unwrap_err();
            assert_eq!(error.to_string(), expected);
            assert!(String::from_utf8(stderr).unwrap().contains(expected));
            assert!(!String::from_utf8(stdout).unwrap().contains("successful"));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn browser_login_stops_at_deadline_before_polling() {
        let manager = browser_manager(chrono::Utc::now() + chrono::Duration::milliseconds(100));
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let error = run_login(
            &manager,
            "cursor",
            &mut NoCodeInput,
            &mut stdout,
            &mut stderr,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Browser login expired"));
        assert!(manager.advanced_sessions.lock().unwrap().is_empty());
    }
}
