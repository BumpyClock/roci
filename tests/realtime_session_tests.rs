#![cfg(feature = "audio")]

mod realtime {
    use std::sync::{Arc, Mutex};

    use futures::{SinkExt, StreamExt};
    use roci::audio::realtime::{RealtimeConfiguration, RealtimeEvent, RealtimeSession};
    use roci::error::RociError;
    use serde_json::{json, Value};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::time::{timeout, Duration, Instant};
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{Request, Response},
            http::StatusCode,
            Message,
        },
    };

    #[derive(Debug)]
    struct HappyPathObservation {
        auth_header: String,
        beta_header: String,
        query: String,
        bootstrap: Value,
        ping_seen: bool,
    }

    #[tokio::test]
    async fn connect_bootstraps_parses_events_sends_heartbeat_and_closes_gracefully() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("local addr should be available");

        let (observation_tx, observation_rx) = oneshot::channel::<HappyPathObservation>();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("server should accept");
            let auth_capture = Arc::new(Mutex::new(String::new()));
            let beta_capture = Arc::new(Mutex::new(String::new()));
            let query_capture = Arc::new(Mutex::new(String::new()));

            let auth_capture_inner = Arc::clone(&auth_capture);
            let beta_capture_inner = Arc::clone(&beta_capture);
            let query_capture_inner = Arc::clone(&query_capture);
            let mut ws = accept_hdr_async(stream, move |req: &Request, response: Response| {
                *auth_capture_inner
                    .lock()
                    .expect("auth lock should not poison") = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *beta_capture_inner
                    .lock()
                    .expect("beta lock should not poison") = req
                    .headers()
                    .get("openai-beta")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *query_capture_inner
                    .lock()
                    .expect("query lock should not poison") =
                    req.uri().query().unwrap_or_default().to_string();
                Ok(response)
            })
            .await
            .expect("handshake should succeed");

            let bootstrap_message = timeout(Duration::from_secs(1), ws.next())
                .await
                .expect("bootstrap wait should not timeout")
                .expect("bootstrap frame should exist")
                .expect("bootstrap frame should parse");
            let bootstrap_text = match bootstrap_message {
                Message::Text(text) => text.to_string(),
                other => panic!("unexpected bootstrap frame: {other:?}"),
            };
            let bootstrap =
                serde_json::from_str::<Value>(&bootstrap_text).expect("bootstrap should be JSON");

            ws.send(Message::Text(
                json!({"type":"session.created","session":{"id":"session-happy"}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("session.created should send");
            ws.send(Message::Text(
                json!({"type":"response.text.delta","delta":"hello world"})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("text delta should send");

            let mut ping_seen = false;
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                match timeout(Duration::from_millis(100), ws.next()).await {
                    Ok(Some(Ok(Message::Ping(_)))) => {
                        ping_seen = true;
                        break;
                    }
                    Ok(Some(Ok(Message::Pong(_)))) => {}
                    Ok(Some(Ok(Message::Text(_)))) => {}
                    Ok(Some(Ok(Message::Binary(_)))) => {}
                    Ok(Some(Ok(Message::Close(_)))) => break,
                    Ok(Some(Ok(Message::Frame(_)))) => {}
                    Ok(Some(Err(_))) => break,
                    Ok(None) => break,
                    Err(_) => {}
                }
            }

            let _ = timeout(Duration::from_secs(1), ws.next()).await;
            let _ = observation_tx.send(HappyPathObservation {
                auth_header: auth_capture
                    .lock()
                    .expect("auth lock should not poison")
                    .clone(),
                beta_header: beta_capture
                    .lock()
                    .expect("beta lock should not poison")
                    .clone(),
                query: query_capture
                    .lock()
                    .expect("query lock should not poison")
                    .clone(),
                bootstrap,
                ping_seen,
            });
        });

        let mut config = RealtimeConfiguration::default();
        config.api_key = Some("test-key".into());
        config.base_url = format!("ws://{address}/v1/realtime");
        config.model = "gpt-4o-realtime-preview-2024-10-01".into();
        config.heartbeat_interval = Duration::from_millis(10);
        config.reconnect_max_attempts = 0;

        let mut session = RealtimeSession::new(config);
        session.connect().await.expect("connect should succeed");

        let session_event = wait_for_event(&mut session, Duration::from_secs(1), |event| {
            matches!(event, RealtimeEvent::SessionCreated { .. })
        })
        .await;
        assert_eq!(
            session_event,
            RealtimeEvent::SessionCreated {
                session_id: "session-happy".into()
            }
        );

        let text_event = wait_for_event(&mut session, Duration::from_secs(1), |event| {
            matches!(event, RealtimeEvent::TextDelta { .. })
        })
        .await;
        assert_eq!(
            text_event,
            RealtimeEvent::TextDelta {
                text: "hello world".into()
            }
        );

        tokio::time::sleep(Duration::from_millis(80)).await;
        session.close().await.expect("close should succeed");
        let closed_event = wait_for_event(&mut session, Duration::from_secs(1), |event| {
            matches!(event, RealtimeEvent::SessionClosed)
        })
        .await;
        assert_eq!(closed_event, RealtimeEvent::SessionClosed);

        let observation = observation_rx
            .await
            .expect("observation should be collected");
        assert_eq!(observation.auth_header, "Bearer test-key");
        assert_eq!(observation.beta_header, "realtime=v1");
        assert!(observation
            .query
            .contains("model=gpt-4o-realtime-preview-2024-10-01"));
        assert_eq!(observation.bootstrap["type"], "session.update");
        assert_eq!(
            observation.bootstrap["session"]["model"],
            "gpt-4o-realtime-preview-2024-10-01"
        );
        assert!(observation.ping_seen);

        server.await.expect("server task should complete");
    }

    #[tokio::test]
    async fn connect_returns_authentication_error_when_server_rejects_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("local addr should be available");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("server should accept");
            let result = accept_hdr_async(stream, |_req: &Request, _response: Response| {
                let response = tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Some("unauthorized".to_string()))
                    .expect("auth failure response should build");
                Err(response)
            })
            .await;
            assert!(result.is_err());
        });

        let mut config = RealtimeConfiguration::default();
        config.api_key = Some("wrong-key".into());
        config.base_url = format!("ws://{address}/v1/realtime");
        config.model = "gpt-4o-realtime-preview".into();

        let mut session = RealtimeSession::new(config);
        let error = session.connect().await.expect_err("connect should fail");
        assert!(matches!(error, RociError::Authentication(_)));

        server.await.expect("server task should complete");
    }

    #[tokio::test]
    async fn disconnect_triggers_reconnect_and_resends_bootstrap() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener
            .local_addr()
            .expect("local addr should be available");

        let bootstrap_count = Arc::new(Mutex::new(0usize));
        let bootstrap_count_server = Arc::clone(&bootstrap_count);
        let server = tokio::spawn(async move {
            for connection_index in 0..2 {
                let (stream, _) = listener.accept().await.expect("server should accept");
                let mut ws =
                    accept_hdr_async(stream, |_req: &Request, response: Response| Ok(response))
                        .await
                        .expect("handshake should succeed");

                let bootstrap_frame = timeout(Duration::from_secs(1), ws.next())
                    .await
                    .expect("bootstrap wait should not timeout")
                    .expect("bootstrap frame should exist")
                    .expect("bootstrap frame should parse");
                match bootstrap_frame {
                    Message::Text(_) => {
                        *bootstrap_count_server
                            .lock()
                            .expect("bootstrap counter lock should not poison") += 1;
                    }
                    other => panic!("unexpected bootstrap frame: {other:?}"),
                }

                let session_id = if connection_index == 0 {
                    "session-first"
                } else {
                    "session-second"
                };
                ws.send(Message::Text(
                    json!({"type":"session.created","session":{"id":session_id}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("session.created should send");

                if connection_index == 0 {
                    ws.close(None).await.expect("first connection should close");
                    continue;
                }

                ws.send(Message::Text(
                    json!({"type":"response.text.delta","delta":"after reconnect"})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("text delta should send");
                let _ = timeout(Duration::from_secs(1), ws.next()).await;
            }
        });

        let mut config = RealtimeConfiguration::default();
        config.api_key = Some("test-key".into());
        config.base_url = format!("ws://{address}/v1/realtime");
        config.model = "gpt-4o-realtime-preview".into();
        config.reconnect_max_attempts = 3;
        config.reconnect_base_delay = Duration::from_millis(20);
        config.reconnect_max_delay = Duration::from_millis(80);
        config.heartbeat_interval = Duration::from_millis(200);

        let mut session = RealtimeSession::new(config);
        session.connect().await.expect("connect should succeed");

        let first_session = wait_for_event(&mut session, Duration::from_secs(1), |event| {
            matches!(event, RealtimeEvent::SessionCreated { session_id } if session_id == "session-first")
        })
        .await;
        assert_eq!(
            first_session,
            RealtimeEvent::SessionCreated {
                session_id: "session-first".into()
            }
        );

        let second_session = wait_for_event(&mut session, Duration::from_secs(2), |event| {
            matches!(event, RealtimeEvent::SessionCreated { session_id } if session_id == "session-second")
        })
        .await;
        assert_eq!(
            second_session,
            RealtimeEvent::SessionCreated {
                session_id: "session-second".into()
            }
        );

        let reconnect_text = wait_for_event(
            &mut session,
            Duration::from_secs(1),
            |event| matches!(event, RealtimeEvent::TextDelta { text } if text == "after reconnect"),
        )
        .await;
        assert_eq!(
            reconnect_text,
            RealtimeEvent::TextDelta {
                text: "after reconnect".into()
            }
        );

        session.close().await.expect("close should succeed");
        server.await.expect("server task should complete");
        assert_eq!(
            *bootstrap_count
                .lock()
                .expect("bootstrap counter lock should not poison"),
            2
        );
    }

    async fn wait_for_event<F>(
        session: &mut RealtimeSession,
        max_wait: Duration,
        mut predicate: F,
    ) -> RealtimeEvent
    where
        F: FnMut(&RealtimeEvent) -> bool,
    {
        let deadline = Instant::now() + max_wait;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("event did not arrive before timeout");
            let event = timeout(remaining, session.next_event())
                .await
                .expect("waiting for event should not timeout")
                .expect("event stream should stay open");
            if predicate(&event) {
                return event;
            }
        }
    }
}
