use async_trait::async_trait;
use rmcp::service::{RoleClient, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport as RmcpTransport;

use crate::error::RociError;

fn map_transport_error(operation: &'static str, error: impl std::fmt::Display) -> RociError {
    RociError::Provider {
        provider: "mcp".into(),
        message: format!("mcp transport {operation} failed: {error}"),
    }
}

#[async_trait]
pub(super) trait DynRoleClientTransport: Send {
    async fn send(&mut self, message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError>;
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>>;
    async fn close(&mut self) -> Result<(), RociError>;
}

pub(super) struct ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    inner: T,
    closed: bool,
}

impl<T> ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    pub(super) fn new(inner: T) -> Self {
        Self {
            inner,
            closed: false,
        }
    }
}

#[async_trait]
impl<T> DynRoleClientTransport for ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    async fn send(&mut self, message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }

        RmcpTransport::send(&mut self.inner, message)
            .await
            .map_err(|error| map_transport_error("send", error))
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        if self.closed {
            return None;
        }

        RmcpTransport::receive(&mut self.inner).await
    }

    async fn close(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        RmcpTransport::close(&mut self.inner)
            .await
            .map_err(|error| map_transport_error("close", error))
    }
}
