//! Realtime audio session over WebSocket.

use crate::error::RociError;
use super::config::RealtimeConfiguration;

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
