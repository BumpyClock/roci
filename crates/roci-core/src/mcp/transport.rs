//! MCP transport layer.

use async_trait::async_trait;
use rmcp::service::{ClientInitializeError, DynService, RoleClient, RunningService};
use std::time::Duration;

use crate::error::RociError;

use super::elicitation::MCPClientHandler;

pub type DynClientService = Box<dyn DynService<RoleClient>>;
pub type MCPRunningService = RunningService<RoleClient, DynClientService>;

/// Reconnect and backoff policy for remote MCP transports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MCPRemoteReconnectPolicy {
    /// Total reconnect attempts, including the immediate first attempt.
    pub max_attempts: usize,
    /// First retry sleep in milliseconds after the immediate attempt fails.
    pub initial_backoff_ms: u64,
    /// Maximum retry sleep in milliseconds after multiplier and jitter apply.
    pub max_backoff_ms: u64,
    /// Backoff multiplier between retry sleeps. Values below `1.0` are clamped to `1.0`.
    pub backoff_multiplier: f64,
    /// Symmetric jitter ratio applied to retry sleeps. Values are clamped to `0.0..=1.0`.
    pub jitter_ratio: f64,
    /// Reconnect before the next request after this many idle milliseconds.
    pub idle_timeout_ms: Option<u64>,
    /// Reconnect before the next request after this many milliseconds since session start.
    pub periodic_reconnect_ms: Option<u64>,
}

impl MCPRemoteReconnectPolicy {
    pub const DEFAULT_MAX_ATTEMPTS: usize = 3;
    pub const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1_000;
    pub const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;
    pub const DEFAULT_BACKOFF_MULTIPLIER: f64 = 2.0;
    pub const DEFAULT_JITTER_RATIO: f64 = 0.2;

    /// Compute the sleep for a retry attempt after the immediate reconnect try.
    ///
    /// `attempt` is zero-based and returns `None` once it reaches `max_attempts`.
    #[must_use]
    pub fn backoff_delay(&self, attempt: usize) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }

        let multiplier = self.backoff_multiplier.max(1.0);
        let initial_ms = self.initial_backoff_ms as f64;
        let exponential_ms = initial_ms * multiplier.powi(attempt.min(32) as i32);
        let capped_ms = exponential_ms
            .min(self.max_backoff_ms as f64)
            .max(0.0)
            .round() as u64;
        let jitter_ratio = self.jitter_ratio.clamp(0.0, 1.0);
        if jitter_ratio == 0.0 || capped_ms == 0 {
            return Some(Duration::from_millis(capped_ms));
        }

        let jitter_ms = ((capped_ms as f64) * jitter_ratio).round() as u64;
        let span = jitter_ms.saturating_mul(2);
        let random_offset = if span == 0 {
            0
        } else {
            (uuid::Uuid::new_v4().as_u128() % (u128::from(span) + 1)) as u64
        };
        let lower_bound = capped_ms.saturating_sub(jitter_ms);
        let delayed_ms = lower_bound
            .saturating_add(random_offset)
            .min(self.max_backoff_ms);

        Some(Duration::from_millis(delayed_ms))
    }
}

impl Default for MCPRemoteReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
            initial_backoff_ms: Self::DEFAULT_INITIAL_BACKOFF_MS,
            max_backoff_ms: Self::DEFAULT_MAX_BACKOFF_MS,
            backoff_multiplier: Self::DEFAULT_BACKOFF_MULTIPLIER,
            jitter_ratio: Self::DEFAULT_JITTER_RATIO,
            idle_timeout_ms: None,
            periodic_reconnect_ms: None,
        }
    }
}

/// Transport trait for MCP communication.
#[async_trait]
pub trait MCPTransport: Send {
    /// Create and initialize a new rmcp running service for this transport.
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError>;

    /// Send a JSON-RPC message.
    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError>;

    /// Receive a JSON-RPC message.
    async fn receive(&mut self) -> Result<serde_json::Value, RociError>;

    /// Close the transport.
    async fn close(&mut self) -> Result<(), RociError>;

    /// Remote reconnect policy. Local stdio transports return `None`.
    fn remote_reconnect_policy(&self) -> Option<MCPRemoteReconnectPolicy> {
        None
    }
}

mod common;
mod stdio;
mod streamable_http;
mod websocket;

pub use stdio::StdioTransport;
pub use streamable_http::{
    StreamableHttpAuthHeaderProvider, StreamableHttpTransport, StreamableHttpTransportConfig,
};
pub use websocket::{WebSocketAuthHeaderProvider, WebSocketTransport, WebSocketTransportConfig};

#[cfg(test)]
mod test_support;
