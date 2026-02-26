//! CLI auth command handlers for login, status, and logout.

use std::sync::Arc;

use roci::auth::service::{AuthPollResult, AuthStep};
use roci::auth::store::FileTokenStore;

/// Handle `roci-agent auth login <provider>`.
pub async fn handle_login(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());
    let svc = roci::default_auth_service(store);

    match svc.start_login(provider).await? {
        AuthStep::Imported { .. } => {
            println!("Imported existing credentials for {provider}");
        }
        AuthStep::DeviceCode {
            verification_url,
            user_code,
            interval,
            session,
            ..
        } => {
            println!("Visit: {verification_url}");
            println!("Enter code: {user_code}");
            println!("Waiting for authorization...");

            loop {
                tokio::time::sleep(interval).await;
                match svc.poll_device_code(provider, &session).await? {
                    AuthPollResult::Authorized { .. } => {
                        println!("{provider} login successful!");
                        return Ok(());
                    }
                    AuthPollResult::Pending => continue,
                    AuthPollResult::SlowDown { new_interval } => {
                        tokio::time::sleep(new_interval).await;
                        continue;
                    }
                    AuthPollResult::Denied => {
                        eprintln!("Authorization denied");
                        std::process::exit(1);
                    }
                    AuthPollResult::Expired => {
                        eprintln!("Device code expired, please try again");
                        std::process::exit(1);
                    }
                }
            }
        }
        AuthStep::Pkce {
            authorize_url,
            state,
            ..
        } => {
            println!("Visit: {authorize_url}");
            println!("After authorizing, paste the response code below:");
            print!("> ");
            use std::io::Write;
            std::io::stdout().flush()?;

            let mut response = String::new();
            std::io::stdin().read_line(&mut response)?;
            let response = response.trim();

            if response.is_empty() {
                eprintln!("No code provided");
                std::process::exit(1);
            }

            svc.complete_pkce(provider, response, &state).await?;
            println!("{provider} login successful!");
        }
    }

    Ok(())
}

/// Handle `roci-agent auth status`.
pub async fn handle_status() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());
    let svc = roci::default_auth_service(store);

    println!("Authentication Status\n");

    for (name, _key, result) in svc.all_statuses() {
        match result {
            Ok(Some(token)) => {
                let status = if let Some(expires) = token.expires_at {
                    if expires > chrono::Utc::now() {
                        format!("Logged in (expires {})", expires.format("%Y-%m-%d %H:%M"))
                    } else {
                        "Token expired (may auto-refresh)".to_string()
                    }
                } else {
                    "Logged in".to_string()
                };
                println!("  {name}: {status}");
            }
            Ok(None) => println!("  {name}: Not logged in"),
            Err(e) => println!("  {name}: Error: {e}"),
        }
    }

    println!("\nEnvironment Variables:");
    for (name, env_key) in [
        ("OPENAI_API_KEY", "OPENAI_API_KEY"),
        ("ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"),
    ] {
        let status = if std::env::var(env_key).is_ok() {
            "Set"
        } else {
            "Not set"
        };
        println!("  {name}: {status}");
    }

    Ok(())
}

/// Handle `roci-agent auth logout <provider>`.
pub async fn handle_logout(provider: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(FileTokenStore::new_default());
    let svc = roci::default_auth_service(store);

    svc.logout(provider)?;
    println!("Logged out from {provider}");
    Ok(())
}
