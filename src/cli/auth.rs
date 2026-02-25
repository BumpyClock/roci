//! CLI auth command handlers for login, status, and logout.

use std::sync::Arc;

use crate::auth::device_code::DeviceCodePoll;
use crate::auth::providers::claude_code::ClaudeCodeAuth;
use crate::auth::providers::github_copilot::GitHubCopilotAuth;
use crate::auth::providers::openai_codex::OpenAiCodexAuth;
use crate::auth::store::{FileTokenStore, TokenStore};

/// Handle `roci auth login <provider>`.
pub async fn handle_login(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());

    match provider {
        "copilot" | "github-copilot" | "github" => login_copilot(store).await,
        "chatgpt" | "codex" => login_codex(store).await,
        "claude" | "anthropic" => login_claude(store).await,
        _ => {
            eprintln!("Unknown provider: {provider}");
            eprintln!("Supported: copilot, codex, claude");
            std::process::exit(1);
        }
    }
}

async fn login_copilot(store: Arc<FileTokenStore>) -> Result<(), Box<dyn std::error::Error>> {
    let auth = GitHubCopilotAuth::new(store.clone());
    let session = auth.start_device_code().await?;

    println!("🔗 Visit: {}", session.verification_url);
    println!("📋 Enter code: {}", session.user_code);
    println!("⏳ Waiting for authorization...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(session.interval_secs)).await;
        match auth.poll_device_code(&session).await? {
            DeviceCodePoll::Authorized { .. } => {
                // Exchange GitHub token for Copilot JWT to verify it works
                match auth.exchange_copilot_token().await {
                    Ok(copilot_token) => {
                        // Save the Copilot JWT + base_url so create_provider can read them
                        let api_token = crate::auth::token::Token {
                            access_token: copilot_token.token,
                            refresh_token: None,
                            id_token: None,
                            expires_at: Some(copilot_token.expires_at),
                            last_refresh: Some(chrono::Utc::now()),
                            scopes: None,
                            account_id: Some(copilot_token.base_url.clone()),
                        };
                        // Store as "github-copilot-api" — the provider reads this
                        let _ = store.save("github-copilot-api", "default", &api_token);
                        println!("✅ GitHub Copilot login successful!");
                        println!(
                            "   API: {}",
                            copilot_token
                                .base_url
                                .split('/')
                                .take(3)
                                .collect::<Vec<_>>()
                                .join("/")
                        );
                    }
                    Err(e) => {
                        println!("⚠️  GitHub token saved but Copilot token exchange failed: {e}");
                        println!("   You may need a GitHub Copilot subscription.");
                    }
                }
                return Ok(());
            }
            DeviceCodePoll::Pending { .. } => continue,
            DeviceCodePoll::SlowDown { interval_secs } => {
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
                continue;
            }
            DeviceCodePoll::AccessDenied => {
                eprintln!("❌ Authorization denied");
                std::process::exit(1);
            }
            DeviceCodePoll::Expired => {
                eprintln!("❌ Device code expired, please try again");
                std::process::exit(1);
            }
        }
    }
}

async fn login_codex(store: Arc<FileTokenStore>) -> Result<(), Box<dyn std::error::Error>> {
    let auth = OpenAiCodexAuth::new(store);

    if let Ok(Some(_token)) = auth.import_codex_auth_json(None) {
        println!("✅ Imported credentials from ~/.codex/auth.json");
        return Ok(());
    }

    let session = auth.start_device_code().await?;

    println!("🔗 Visit: {}", session.verification_url);
    println!("📋 Enter code: {}", session.user_code);
    println!("⏳ Waiting for authorization...");

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(session.interval_secs)).await;
        match auth.poll_device_code(&session).await? {
            DeviceCodePoll::Authorized { .. } => {
                println!("✅ OpenAI/ChatGPT login successful!");
                return Ok(());
            }
            DeviceCodePoll::Pending { .. } => continue,
            DeviceCodePoll::SlowDown { interval_secs } => {
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
                continue;
            }
            DeviceCodePoll::AccessDenied => {
                eprintln!("❌ Authorization denied");
                std::process::exit(1);
            }
            DeviceCodePoll::Expired => {
                eprintln!("❌ Device code expired, please try again");
                std::process::exit(1);
            }
        }
    }
}

async fn login_claude(store: Arc<FileTokenStore>) -> Result<(), Box<dyn std::error::Error>> {
    let auth = ClaudeCodeAuth::new(store);

    // Try importing existing credentials first (zero-friction path).
    if let Ok(Some(_token)) = auth.import_cli_credentials(None) {
        println!("✅ Imported credentials from ~/.claude/.credentials.json");
        return Ok(());
    }

    // Fall back to interactive PKCE authorization-code flow.
    let session = auth.start_auth()?;
    println!("🔗 Visit: {}", session.authorize_url);
    println!("📋 After authorizing, paste the response code below:");
    print!("> ");
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut response = String::new();
    std::io::stdin().read_line(&mut response)?;
    let response = response.trim();

    if response.is_empty() {
        eprintln!("❌ No code provided.");
        std::process::exit(1);
    }

    auth.exchange_code(&session, response).await?;
    println!("✅ Claude login successful!");
    Ok(())
}

/// Handle `roci auth status`.
pub async fn handle_status() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());

    println!("🔐 Authentication Status\n");

    for (name, provider_key) in [
        ("GitHub Copilot", "github-copilot"),
        ("Codex", "openai-codex"),
        ("Claude", "claude-code"),
    ] {
        match store.load(provider_key, "default") {
            Ok(Some(token)) => {
                let status = if let Some(expires) = token.expires_at {
                    if expires > chrono::Utc::now() {
                        format!(
                            "✅ Logged in (expires {})",
                            expires.format("%Y-%m-%d %H:%M")
                        )
                    } else {
                        "⚠️  Token expired (may auto-refresh)".to_string()
                    }
                } else {
                    "✅ Logged in".to_string()
                };
                println!("  {name}: {status}");
            }
            Ok(None) => println!("  {name}: ❌ Not logged in"),
            Err(e) => println!("  {name}: ⚠️  Error: {e}"),
        }
    }

    println!("\n📌 Environment Variables:");
    for (name, env_key) in [
        ("OPENAI_API_KEY", "OPENAI_API_KEY"),
        ("ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"),
    ] {
        let status = if std::env::var(env_key).is_ok() {
            "✅ Set"
        } else {
            "❌ Not set"
        };
        println!("  {name}: {status}");
    }

    Ok(())
}

/// Handle `roci auth logout <provider>`.
pub async fn handle_logout(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());

    let provider_key = match provider {
        "copilot" | "github-copilot" | "github" => "github-copilot",
        "chatgpt" | "codex" => "openai-codex",
        "claude" | "anthropic" => "claude-code",
        _ => {
            eprintln!("Unknown provider: {provider}");
            eprintln!("Supported: copilot, codex, claude");
            std::process::exit(1);
        }
    };

    store.clear(provider_key, "default")?;
    println!("✅ Logged out from {provider}");
    Ok(())
}
