//! Cross-process Unix auth smoke: configure and status share `~/.roci/auth.json`.
//!
//! Uses a temporary absolute `HOME` and scrubs external credential env vars so
//! the real home/keychain and ambient env cannot satisfy the flow.

#![cfg(all(unix, feature = "openai-compatible"))]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// Sentinel key used only inside this hermetic smoke. Never print this value.
const API_KEY: &str = "sk-roci-auth-smoke-do-not-print";
const ENDPOINT: &str = "http://127.0.0.1:9/v1";
const PROVIDER: &str = "openai-compatible";

/// Env keys RociConfig::from_env reads for API keys / base URLs.
const SCRUBBED_ENV: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_CODEX_TOKEN",
    "CHATGPT_TOKEN",
    "OPENAI_COMPAT_API_KEY",
    "ANTHROPIC_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "GROK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "TOGETHER_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_CODEX_BASE_URL",
    "CHATGPT_BASE_URL",
    "OPENAI_COMPAT_BASE_URL",
    "ANTHROPIC_BASE_URL",
    "OLLAMA_BASE_URL",
    "LMSTUDIO_BASE_URL",
    "AZURE_OPENAI_ENDPOINT",
];

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_roci-agent")
}

fn assert_no_secret(label: &str, text: &str) {
    assert!(
        !text.contains(API_KEY),
        "{label} must not include API key material"
    );
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("metadata for {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn run_auth(home: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
    let mut command = Command::new(binary_path());
    command
        .args(args)
        // Temp HOME only; keep ambient loader vars. Scrub credential env below.
        .current_dir(home)
        .env("HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }

    if stdin.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }

    if let Some(payload) = stdin {
        let mut child = command.spawn().expect("spawn roci-agent");
        let mut child_stdin = child.stdin.take().expect("stdin");
        child_stdin.write_all(payload).expect("write stdin");
        drop(child_stdin);
        return child.wait_with_output().expect("wait roci-agent");
    }

    command.output().expect("run roci-agent")
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn configure_then_status_share_auth_json_under_temp_home() {
    let home = TempDir::new().expect("temp HOME");
    let home_path = home.path().canonicalize().expect("absolute HOME");
    fs::write(home_path.join(".env"), b"").expect("isolate dotenv lookup");
    let roci_dir = home_path.join(".roci");
    let auth_json = roci_dir.join("auth.json");
    let auth_lock = roci_dir.join("auth.json.lock");

    // Process 1: persist API key + endpoint into the file store.
    let configure_input = format!("{API_KEY}\n");
    let configure = run_auth(
        &home_path,
        &[
            "auth",
            "configure",
            PROVIDER,
            "--endpoint",
            ENDPOINT,
            "--api-key-stdin",
        ],
        Some(configure_input.as_bytes()),
    );
    let configure_stdout = utf8(&configure.stdout);
    let configure_stderr = utf8(&configure.stderr);
    assert_no_secret("configure stdout", &configure_stdout);
    assert_no_secret("configure stderr", &configure_stderr);
    assert!(
        configure.status.success(),
        "configure failed: status={:?} stderr={configure_stderr}",
        configure.status.code()
    );
    assert_eq!(configure_stdout, format!("Configured {PROVIDER}\n"));

    assert!(roci_dir.is_dir(), "expected ~/.roci directory");
    assert!(auth_json.is_file(), "expected ~/.roci/auth.json");
    assert!(auth_lock.is_file(), "expected ~/.roci/auth.json.lock");
    assert_eq!(mode(&roci_dir), 0o700, ".roci mode");
    assert_eq!(mode(&auth_json), 0o600, "auth.json mode");
    assert_eq!(mode(&auth_lock), 0o600, "auth.json.lock mode");

    // Inspect the on-disk record without printing secret material.
    let raw = fs::read(&auth_json).expect("read auth.json");
    let auth: Value = serde_json::from_slice(&raw).expect("auth.json JSON");
    let record = auth
        .get(PROVIDER)
        .unwrap_or_else(|| panic!("missing {PROVIDER} record key"));
    assert_eq!(record.get("version").and_then(Value::as_u64), Some(1));
    assert_eq!(
        record.get("endpoint").and_then(Value::as_str),
        Some(ENDPOINT)
    );
    let stored_key = record.get("api_key").and_then(Value::as_str);
    assert!(
        stored_key == Some(API_KEY),
        "auth.json api_key mismatch for {PROVIDER}"
    );
    // Map must be exactly the one configured provider.
    assert_eq!(
        auth.as_object().map(|object| object.len()),
        Some(1),
        "auth.json should contain only the configured provider"
    );

    // Process 2: fresh CLI process must observe stored status without env/keychain.
    let status = run_auth(&home_path, &["auth", "status", "--json"], None);
    let status_stdout = utf8(&status.stdout);
    let status_stderr = utf8(&status.stderr);
    assert_no_secret("status stdout", &status_stdout);
    assert_no_secret("status stderr", &status_stderr);
    assert!(
        status.status.success(),
        "status failed: status={:?} stderr={status_stderr}",
        status.status.code()
    );

    let statuses: Value = serde_json::from_str(&status_stdout).expect("status JSON");
    let row = statuses
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| {
            entry
                .pointer("/descriptor/canonical_key")
                .and_then(Value::as_str)
                == Some(PROVIDER)
        })
        .expect("openai-compatible status row");

    assert_eq!(
        row.get("configured_sources"),
        Some(&Value::Array(vec![Value::String("stored_api_key".into())]))
    );
    assert_eq!(row.get("launch_available"), Some(&Value::Bool(true)));
    assert_eq!(
        row.pointer("/auth_state/kind").and_then(Value::as_str),
        Some("signed_in")
    );
    assert_eq!(
        row.pointer("/auth_state/label").and_then(Value::as_str),
        Some("Signed in")
    );

    // Exercise both output formats and the providers alias with a stored secret.
    // Unit fixtures contain only status DTOs, so they cannot establish redaction.
    for args in [vec!["auth", "status"], vec!["auth", "providers", "--json"]] {
        let output = run_auth(&home_path, &args, None);
        let stdout = utf8(&output.stdout);
        let stderr = utf8(&output.stderr);
        assert_no_secret("auth stdout", &stdout);
        assert_no_secret("auth stderr", &stderr);
        assert!(
            output.status.success(),
            "{args:?} failed: status={:?} stderr={stderr}",
            output.status.code()
        );
        if args.contains(&"--json") {
            assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), statuses);
        } else {
            assert!(stdout.contains(PROVIDER));
            assert!(stdout.contains("Signed in [launch: available]"));
        }
    }

    // Status commands must not have rewritten modes or dropped the record.
    assert_eq!(mode(&roci_dir), 0o700, ".roci mode after status");
    assert_eq!(mode(&auth_json), 0o600, "auth.json mode after status");
    assert_eq!(mode(&auth_lock), 0o600, "lock mode after status");
    let reread = fs::read(&auth_json).expect("reread auth.json");
    assert!(
        reread == raw,
        "status must not rewrite auth.json bytes (len before={}, after={})",
        raw.len(),
        reread.len()
    );
}

#[test]
fn named_account_configure_status_and_logout_are_isolated_across_processes() {
    let home = TempDir::new().unwrap();
    fs::write(home.path().join(".env"), "").unwrap();
    let configured = run_auth(
        home.path(),
        &[
            "auth",
            "--account",
            "work",
            "configure",
            PROVIDER,
            "--endpoint",
            ENDPOINT,
            "--api-key-stdin",
        ],
        Some(API_KEY.as_bytes()),
    );
    assert!(configured.status.success(), "{}", utf8(&configured.stderr));
    let available = |account: &str| {
        let result = run_auth(
            home.path(),
            &["auth", "--account", account, "status", "--json"],
            None,
        );
        assert!(result.status.success(), "{}", utf8(&result.stderr));
        let text = utf8(&result.stdout);
        assert_no_secret("status", &text);
        let rows: Value = serde_json::from_str(&text).unwrap();
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["descriptor"]["canonical_key"] == PROVIDER)
            .unwrap()["launch_available"]
            .as_bool()
            .unwrap()
    };
    assert!(available("work"));
    assert!(!available("personal"));
    assert!(!available("default"));
    let logout = run_auth(
        home.path(),
        &["auth", "--account", "personal", "logout", PROVIDER],
        None,
    );
    assert!(logout.status.success());
    assert!(available("work"));
    let logout = run_auth(
        home.path(),
        &["auth", "--account", "work", "logout", PROVIDER],
        None,
    );
    assert!(logout.status.success());
    assert!(!available("work"));
}
