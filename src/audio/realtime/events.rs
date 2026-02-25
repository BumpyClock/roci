//! Realtime event types.

use serde_json::Value;

/// Events in a realtime audio session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeEvent {
    SessionCreated { session_id: String },
    SessionUpdated { session_id: Option<String> },
    AudioDelta { delta: String },
    TextDelta { text: String },
    TranscriptionDelta { text: String },
    Error { message: String },
    Unknown { event_type: String },
    SessionClosed,
}

impl RealtimeEvent {
    /// Parse a server event payload into a typed realtime event.
    pub fn from_server_payload(payload: &Value) -> Option<Self> {
        let event_type = payload.get("type")?.as_str()?;
        match event_type {
            "session.created" => Some(Self::SessionCreated {
                session_id: string_at(payload, &["session", "id"])
                    .or_else(|| string_field(payload, "session_id"))
                    .unwrap_or_else(|| "unknown".to_string()),
            }),
            "session.updated" => Some(Self::SessionUpdated {
                session_id: string_at(payload, &["session", "id"])
                    .or_else(|| string_field(payload, "session_id")),
            }),
            "response.audio.delta" => {
                string_field(payload, "delta").map(|delta| Self::AudioDelta { delta })
            }
            "response.text.delta" => {
                string_field(payload, "delta").map(|text| Self::TextDelta { text })
            }
            "response.audio_transcript.delta" => {
                string_field(payload, "delta").map(|text| Self::TranscriptionDelta { text })
            }
            "conversation.item.input_audio_transcription.completed" => {
                string_field(payload, "text")
                    .or_else(|| string_field(payload, "transcript"))
                    .map(|text| Self::TranscriptionDelta { text })
            }
            "error" => Some(Self::Error {
                message: string_at(payload, &["error", "message"])
                    .or_else(|| string_field(payload, "message"))
                    .unwrap_or_else(|| "Realtime server error".to_string()),
            }),
            _ => Some(Self::Unknown {
                event_type: event_type.to_string(),
            }),
        }
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str().map(ToString::to_string)
}
