//! Realtime event types.

use serde::{Deserialize, Serialize};

/// Events in a realtime audio session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    SessionCreated { session_id: String },
    AudioDelta { delta: String },
    TextDelta { text: String },
    TranscriptionDelta { text: String },
    Error { message: String },
    SessionClosed,
}
