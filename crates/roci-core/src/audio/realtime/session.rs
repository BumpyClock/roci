//! Realtime audio session over WebSocket.

use std::{env, time::Duration};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Error as WsError, Message},
    MaybeTlsStream, WebSocketStream,
};

use super::{config::RealtimeConfiguration, events::RealtimeEvent};
use crate::{audio::types::AudioFormat, error::RociError};

type RealtimeWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct RealtimeRuntime {
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Clone)]
struct RuntimeParams {
    url: String,
    api_key: String,
    bootstrap_payload: String,
    heartbeat_interval: Duration,
    reconnect_max_attempts: usize,
    reconnect_base_delay: Duration,
    reconnect_max_delay: Duration,
}

/// A WebSocket-based realtime audio session.
pub struct RealtimeSession {
    config: RealtimeConfiguration,
    events_rx: Option<mpsc::UnboundedReceiver<RealtimeEvent>>,
    runtime: Option<RealtimeRuntime>,
}

impl RealtimeSession {
    /// Create a new realtime session (does not connect yet).
    pub fn new(config: RealtimeConfiguration) -> Self {
        Self {
            config,
            events_rx: None,
            runtime: None,
        }
    }

    /// Connect to the realtime endpoint.
    pub async fn connect(&mut self) -> Result<(), RociError> {
        if self.runtime.is_some() {
            return Err(RociError::InvalidState(
                "Realtime session is already connected".into(),
            ));
        }

        let api_key = resolve_api_key(&self.config)?;
        let url = build_realtime_url(&self.config.base_url, &self.config.model)?;
        let bootstrap_payload = build_session_bootstrap_payload(&self.config)?;

        let mut socket = connect_realtime_socket(&url, &api_key).await?;
        send_bootstrap_message(&mut socket, &bootstrap_payload).await?;

        let params = RuntimeParams {
            url,
            api_key,
            bootstrap_payload,
            heartbeat_interval: self.config.heartbeat_interval,
            reconnect_max_attempts: self.config.reconnect_max_attempts,
            reconnect_base_delay: self.config.reconnect_base_delay,
            reconnect_max_delay: self.config.reconnect_max_delay,
        };

        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(run_supervisor_loop(socket, events_tx, shutdown_rx, params));

        self.events_rx = Some(events_rx);
        self.runtime = Some(RealtimeRuntime { shutdown_tx, task });
        Ok(())
    }

    /// Wait for the next event from the realtime stream.
    pub async fn next_event(&mut self) -> Option<RealtimeEvent> {
        self.events_rx.as_mut()?.recv().await
    }

    /// Close the realtime session gracefully.
    pub async fn close(&mut self) -> Result<(), RociError> {
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.shutdown_tx.send(true);
            runtime.task.await.map_err(|error| {
                RociError::Stream(format!("Realtime runtime task failed: {error}"))
            })?;
        }
        Ok(())
    }
}

impl Drop for RealtimeSession {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.shutdown_tx.send(true);
            runtime.task.abort();
        }
    }
}

enum ConnectionOutcome {
    Shutdown,
    Disconnected,
}

async fn run_supervisor_loop(
    mut socket: RealtimeWebSocket,
    events_tx: mpsc::UnboundedSender<RealtimeEvent>,
    mut shutdown_rx: watch::Receiver<bool>,
    params: RuntimeParams,
) {
    let mut reconnect_attempt = 0usize;
    loop {
        let outcome = run_active_connection(
            &mut socket,
            &events_tx,
            &mut shutdown_rx,
            params.heartbeat_interval,
        )
        .await;

        if matches!(outcome, ConnectionOutcome::Shutdown) || *shutdown_rx.borrow() {
            break;
        }

        if reconnect_attempt >= params.reconnect_max_attempts {
            let _ = events_tx.send(RealtimeEvent::Error {
                message: "Realtime reconnect attempts exhausted".into(),
            });
            break;
        }
        reconnect_attempt += 1;

        let delay = compute_backoff_delay(
            reconnect_attempt,
            params.reconnect_base_delay,
            params.reconnect_max_delay,
        );
        let sleep = time::sleep(delay);
        tokio::pin!(sleep);

        tokio::select! {
            _ = &mut sleep => {}
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
            }
        }
        if *shutdown_rx.borrow() {
            break;
        }

        match connect_realtime_socket(&params.url, &params.api_key).await {
            Ok(mut reconnected_socket) => {
                if let Err(error) =
                    send_bootstrap_message(&mut reconnected_socket, &params.bootstrap_payload).await
                {
                    let _ = events_tx.send(RealtimeEvent::Error {
                        message: format!("Realtime bootstrap failed during reconnect: {error}"),
                    });
                    continue;
                }
                socket = reconnected_socket;
                reconnect_attempt = 0;
            }
            Err(error) => {
                let _ = events_tx.send(RealtimeEvent::Error {
                    message: format!("Realtime reconnect failed: {error}"),
                });
                if matches!(error, RociError::Authentication(_)) {
                    break;
                }
            }
        }
    }

    let _ = events_tx.send(RealtimeEvent::SessionClosed);
}

async fn run_active_connection(
    socket: &mut RealtimeWebSocket,
    events_tx: &mpsc::UnboundedSender<RealtimeEvent>,
    shutdown_rx: &mut watch::Receiver<bool>,
    heartbeat_interval: Duration,
) -> ConnectionOutcome {
    let mut heartbeat = time::interval(heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    let _ = socket.send(Message::Close(None)).await;
                    return ConnectionOutcome::Shutdown;
                }
                if changed.is_err() {
                    return ConnectionOutcome::Shutdown;
                }
            }
            _ = heartbeat.tick() => {
                if let Err(error) = socket.send(Message::Ping(Default::default())).await {
                    let _ = events_tx.send(RealtimeEvent::Error {
                        message: format!("Realtime heartbeat failed: {error}"),
                    });
                    return ConnectionOutcome::Disconnected;
                }
            }
            frame = socket.next() => {
                match frame {
                    Some(Ok(message)) => {
                        if let Err(error) = handle_server_message(socket, events_tx, message).await {
                            let _ = events_tx.send(RealtimeEvent::Error {
                                message: format!("Realtime websocket frame handling failed: {error}"),
                            });
                            return ConnectionOutcome::Disconnected;
                        }
                    }
                    Some(Err(error)) => {
                        let _ = events_tx.send(RealtimeEvent::Error {
                            message: format!("Realtime websocket receive failed: {error}"),
                        });
                        return ConnectionOutcome::Disconnected;
                    }
                    None => return ConnectionOutcome::Disconnected,
                }
            }
        }
    }
}

async fn handle_server_message(
    socket: &mut RealtimeWebSocket,
    events_tx: &mpsc::UnboundedSender<RealtimeEvent>,
    message: Message,
) -> Result<(), WsError> {
    match message {
        Message::Text(text) => parse_and_forward_event(text.as_ref(), events_tx),
        Message::Binary(bytes) => {
            if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                parse_and_forward_event(&text, events_tx);
            }
        }
        Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
        Message::Pong(_) => {}
        Message::Close(_) => return Err(WsError::ConnectionClosed),
        Message::Frame(_) => {}
    }
    Ok(())
}

fn parse_and_forward_event(payload: &str, events_tx: &mpsc::UnboundedSender<RealtimeEvent>) {
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => {
            if let Some(event) = RealtimeEvent::from_server_payload(&value) {
                let _ = events_tx.send(event);
            }
        }
        Err(error) => {
            let _ = events_tx.send(RealtimeEvent::Error {
                message: format!("Failed to parse realtime event payload: {error}"),
            });
        }
    }
}

fn resolve_api_key(config: &RealtimeConfiguration) -> Result<String, RociError> {
    if let Some(api_key) = config
        .api_key
        .clone()
        .or_else(|| env::var("OPENAI_API_KEY").ok())
        .filter(|value| !value.trim().is_empty())
    {
        Ok(api_key)
    } else {
        Err(RociError::Authentication("Missing OPENAI_API_KEY".into()))
    }
}

fn build_realtime_url(base_url: &str, model: &str) -> Result<String, RociError> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err(RociError::Configuration(
            "Realtime base URL cannot be empty".into(),
        ));
    }
    let separator = if trimmed.contains('?') { "&" } else { "?" };
    Ok(format!("{trimmed}{separator}model={model}"))
}

fn build_session_bootstrap_payload(config: &RealtimeConfiguration) -> Result<String, RociError> {
    let mut session = Map::new();
    session.insert("model".into(), Value::String(config.model.clone()));
    session.insert(
        "input_audio_format".into(),
        Value::String(audio_format_value(config.input_format).into()),
    );
    session.insert(
        "output_audio_format".into(),
        Value::String(audio_format_value(config.output_format).into()),
    );
    if let Some(voice) = &config.voice {
        session.insert("voice".into(), Value::String(voice.id.clone()));
    }
    if config.turn_detection {
        session.insert("turn_detection".into(), json!({ "type": "server_vad" }));
    }

    serde_json::to_string(&json!({
        "type": "session.update",
        "session": Value::Object(session),
    }))
    .map_err(RociError::from)
}

fn audio_format_value(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Opus => "opus",
        AudioFormat::Aac => "aac",
        AudioFormat::Flac => "flac",
        AudioFormat::Wav => "wav",
        AudioFormat::Pcm16 => "pcm16",
    }
}

async fn connect_realtime_socket(url: &str, api_key: &str) -> Result<RealtimeWebSocket, RociError> {
    let mut request = url.into_client_request().map_err(|error| {
        RociError::Configuration(format!("Invalid realtime websocket URL: {error}"))
    })?;
    let auth_value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|error| {
        RociError::Configuration(format!("Invalid realtime auth header: {error}"))
    })?;
    request.headers_mut().insert("Authorization", auth_value);
    request
        .headers_mut()
        .insert("OpenAI-Beta", HeaderValue::from_static("realtime=v1"));

    connect_async(request)
        .await
        .map(|(socket, _)| socket)
        .map_err(map_connect_error)
}

async fn send_bootstrap_message(
    socket: &mut RealtimeWebSocket,
    payload: &str,
) -> Result<(), RociError> {
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|error| RociError::Stream(format!("Realtime bootstrap send failed: {error}")))
}

fn map_connect_error(error: WsError) -> RociError {
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if matches!(status, 401 | 403) {
                RociError::Authentication(format!(
                    "Realtime websocket authentication failed with status {status}"
                ))
            } else {
                RociError::api(
                    status,
                    format!("Realtime websocket handshake failed with status {status}"),
                )
            }
        }
        WsError::Io(error) => RociError::Io(error),
        WsError::Url(error) => {
            RociError::Configuration(format!("Invalid realtime websocket URL: {error}"))
        }
        other => RociError::Stream(format!("Realtime websocket connect failed: {other}")),
    }
}

fn compute_backoff_delay(attempt: usize, base: Duration, max_delay: Duration) -> Duration {
    let multiplier = 2u32.saturating_pow(attempt.saturating_sub(1) as u32) as f64;
    let scaled = base.as_secs_f64() * multiplier;
    Duration::from_secs_f64(scaled.min(max_delay.as_secs_f64()))
}
