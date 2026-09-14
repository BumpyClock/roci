//! Host-side loopback receiver for OAuth browser callbacks.

use roci::auth::AuthError;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub(super) struct LoopbackCallback {
    listener: TcpListener,
    redirect: url::Url,
    state: String,
}

impl LoopbackCallback {
    /// Bind before displaying the authorization URL so the browser cannot race
    /// listener startup. Non-loopback OAuth redirects retain manual completion.
    pub(super) async fn bind(authorize_url: &str) -> Result<Option<Self>, AuthError> {
        let authorize = url::Url::parse(authorize_url)
            .map_err(|_| AuthError::InvalidResponse("invalid authorization URL".into()))?;
        let pairs: Vec<_> = authorize.query_pairs().collect();
        let Some((_, redirect)) = pairs.iter().find(|(key, _)| key == "redirect_uri") else {
            return Ok(None);
        };
        let redirect = url::Url::parse(redirect)
            .map_err(|_| AuthError::InvalidResponse("invalid redirect URL".into()))?;
        if redirect.scheme() != "http"
            || !matches!(redirect.host_str(), Some("localhost" | "127.0.0.1"))
        {
            return Ok(None);
        }
        let state = pairs
            .iter()
            .find(|(key, _)| key == "state")
            .map(|(_, state)| state.to_string())
            .filter(|state| !state.is_empty())
            .ok_or_else(|| {
                AuthError::InvalidResponse("authorization URL is missing state".into())
            })?;
        let port = redirect
            .port_or_known_default()
            .ok_or_else(|| AuthError::InvalidResponse("redirect URL has no port".into()))?;
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        Ok(Some(Self {
            listener,
            redirect,
            state,
        }))
    }

    pub(super) async fn receive(&self) -> Result<String, AuthError> {
        tokio::time::timeout(Duration::from_secs(300), self.receive_inner())
            .await
            .map_err(|_| AuthError::Network("browser callback timed out".into()))?
    }

    async fn receive_inner(&self) -> Result<String, AuthError> {
        loop {
            let (mut socket, _) = self.listener.accept().await?;
            let request = tokio::time::timeout(Duration::from_secs(5), async {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 1024];
                while bytes.len() < 8192 {
                    let count = socket.read(&mut chunk).await?;
                    if count == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Ok::<_, std::io::Error>(bytes)
            })
            .await;
            let Ok(Ok(bytes)) = request else { continue };
            let callback = parse_callback(&bytes, &self.redirect, &self.state);
            let (status, message) = if callback.is_some() {
                (
                    "200 OK",
                    "Authorization received. Return to the terminal to finish signing in.",
                )
            } else {
                (
                    "400 Bad Request",
                    "This callback does not match the pending login.",
                )
            };
            let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{message}", message.len());
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                socket.write_all(response.as_bytes()),
            )
            .await;
            if let Some(callback) = callback {
                return Ok(callback);
            }
        }
    }
}

fn parse_callback(bytes: &[u8], redirect: &url::Url, state: &str) -> Option<String> {
    let request = std::str::from_utf8(bytes).ok()?;
    let mut fields = request.lines().next()?.split_whitespace();
    if fields.next()? != "GET" {
        return None;
    }
    let target = fields.next()?;
    if !target.starts_with('/') || target.starts_with("//") {
        return None;
    }
    let url = redirect.join(target).ok()?;
    if url.path() != redirect.path() || url.fragment().is_some() {
        return None;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(key, _)| key == "state").collect();
    if states.len() != 1 || states[0].1 != state {
        return None;
    }
    if !pairs
        .iter()
        .any(|(key, value)| (key == "code" && !value.is_empty()) || key == "error")
    {
        return None;
    }
    Some(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_requires_exact_path_and_state() {
        let redirect = url::Url::parse("http://localhost:1455/auth/callback").unwrap();
        let valid = b"GET /auth/callback?code=secret&state=expected HTTP/1.1\r\nHost: localhost:1455\r\n\r\n";
        assert!(parse_callback(valid, &redirect, "expected").is_some());
        assert!(parse_callback(valid, &redirect, "different").is_none());
        for target in [
            "/other?code=secret&state=expected",
            "/auth/callback?code=secret&state=expected&state=bad",
            "//evil.test/auth/callback?code=secret&state=expected",
        ] {
            let request = format!("GET {target} HTTP/1.1\r\n\r\n");
            assert!(parse_callback(request.as_bytes(), &redirect, "expected").is_none());
        }
    }

    #[tokio::test]
    async fn loopback_delivers_full_callback_without_echoing_secrets_to_browser() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = LoopbackCallback {
            listener,
            redirect: url::Url::parse(&format!("http://localhost:{port}/auth/callback")).unwrap(),
            state: "expected".into(),
        };
        let task = tokio::spawn(async move { receiver.receive().await });
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        socket.write_all(b"GET /auth/callback?code=secret&state=expected HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(!response.contains("secret"));
        let callback = task.await.unwrap().unwrap();
        assert!(callback.contains("code=secret&state=expected"));
    }
}
