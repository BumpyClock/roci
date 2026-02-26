use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use serde_json::json;

use rmcp::service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage};

use super::common::DynRoleClientTransport;
use crate::error::RociError;

pub(super) struct MockInnerTransport {
    receive_queue: VecDeque<Option<RxJsonRpcMessage<RoleClient>>>,
    send_delay_ms: Option<u64>,
    receive_delay_ms: Option<u64>,
    send_calls: Arc<AtomicUsize>,
    close_calls: Arc<AtomicUsize>,
}

impl MockInnerTransport {
    pub(super) fn new(
        receive_queue: Vec<Option<RxJsonRpcMessage<RoleClient>>>,
    ) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let send_calls = Arc::new(AtomicUsize::new(0));
        let close_calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                receive_queue: receive_queue.into(),
                send_delay_ms: None,
                receive_delay_ms: None,
                send_calls: Arc::clone(&send_calls),
                close_calls: Arc::clone(&close_calls),
            },
            send_calls,
            close_calls,
        )
    }

    pub(super) fn with_delays(
        receive_queue: Vec<Option<RxJsonRpcMessage<RoleClient>>>,
        send_delay_ms: Option<u64>,
        receive_delay_ms: Option<u64>,
    ) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (mut mock, send_calls, close_calls) = Self::new(receive_queue);
        mock.send_delay_ms = send_delay_ms;
        mock.receive_delay_ms = receive_delay_ms;
        (mock, send_calls, close_calls)
    }
}

#[async_trait]
impl DynRoleClientTransport for MockInnerTransport {
    async fn send(&mut self, _message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError> {
        if let Some(delay_ms) = self.send_delay_ms {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        self.send_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        if let Some(delay_ms) = self.receive_delay_ms {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        self.receive_queue.pop_front().unwrap_or(None)
    }

    async fn close(&mut self) -> Result<(), RociError> {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

pub(super) fn test_client_request() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    })
}

pub(super) fn test_server_response() -> RxJsonRpcMessage<RoleClient> {
    serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": []
        }
    }))
    .expect("test server response should deserialize")
}
