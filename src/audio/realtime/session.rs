//! Realtime audio session over WebSocket.

use super::config::RealtimeConfiguration;
use crate::error::RociError;

/// A WebSocket-based realtime audio session.
pub struct RealtimeSession {
    _config: RealtimeConfiguration,
}

impl RealtimeSession {
    /// Create a new realtime session (does not connect yet).
    pub fn new(config: RealtimeConfiguration) -> Self {
        Self { _config: config }
    }

    /// Connect to the realtime endpoint.
    pub async fn connect(&mut self) -> Result<(), RociError> {
        Err(RociError::UnsupportedOperation(
            "Realtime audio sessions not yet implemented".into(),
        ))
    }
}
