//! Descriptor-checked protobuf messages and bounded Connect envelopes.

use std::sync::OnceLock;

use base64::{engine::general_purpose::STANDARD, Engine};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage};
use roci_core::error::RociError;
use serde_json::{json, Value};

pub(super) const MAX_FRAME: usize = 8 * 1024 * 1024;

pub(super) fn error(message: impl Into<String>) -> RociError {
    RociError::Provider {
        provider: "cursor".into(),
        message: message.into(),
    }
}

pub(super) fn api_error(status: u16, message: &str) -> RociError {
    RociError::Api {
        status,
        message: message.into(),
        source: None,
        details: None,
    }
}

fn pool() -> Result<&'static DescriptorPool, RociError> {
    static POOL: OnceLock<Result<DescriptorPool, String>> = OnceLock::new();
    POOL.get_or_init(|| {
        let file = prost_types::FileDescriptorProto::decode(
            include_bytes!("agent_descriptor.bin").as_slice(),
        )
        .map_err(|e| e.to_string())?;
        DescriptorPool::from_file_descriptor_set(prost_types::FileDescriptorSet {
            file: vec![file],
        })
        .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|_| error("invalid embedded Cursor protocol descriptor"))
}

pub(super) fn encode(kind: &str, value: Value) -> Result<Vec<u8>, RociError> {
    let descriptor = pool()?
        .get_message_by_name(&format!("agent.v1.{kind}"))
        .ok_or_else(|| error("Cursor protocol message is unavailable"))?;
    let message = DynamicMessage::deserialize(descriptor, value)
        .map_err(|e| error(format!("invalid Cursor protocol message: {e}")))?;
    Ok(message.encode_to_vec())
}

pub(super) fn decode(kind: &str, bytes: &[u8]) -> Result<Value, RociError> {
    let descriptor = pool()?
        .get_message_by_name(&format!("agent.v1.{kind}"))
        .ok_or_else(|| error("Cursor protocol message is unavailable"))?;
    let message = DynamicMessage::decode(descriptor, bytes)
        .map_err(|_| error("malformed Cursor protobuf response"))?;
    serde_json::to_value(message).map_err(|_| error("invalid Cursor protobuf response"))
}

pub(super) fn frame(bytes: Vec<u8>) -> Result<Vec<u8>, RociError> {
    if bytes.len() > MAX_FRAME {
        return Err(error("Cursor request exceeds frame limit"));
    }
    let mut output = Vec::with_capacity(bytes.len() + 5);
    output.push(0);
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend(bytes);
    Ok(output)
}

pub(super) fn client(value: Value) -> Result<Vec<u8>, RociError> {
    frame(encode("AgentClientMessage", value)?)
}

pub(super) enum Frame {
    Message(Value),
    End,
}

pub(super) fn take_frame(buffer: &mut Vec<u8>) -> Result<Option<Frame>, RociError> {
    if buffer.len() < 5 {
        return Ok(None);
    }
    let flags = buffer[0];
    let length = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    if length > MAX_FRAME {
        return Err(error("Cursor response exceeds frame limit"));
    }
    if flags & !2 != 0 {
        return Err(error("unsupported Cursor Connect compression/flags"));
    }
    if buffer.len() < length + 5 {
        return Ok(None);
    }
    let payload = &buffer[5..5 + length];
    let result = if flags == 2 {
        if !payload.is_empty() {
            let trailer: Value = serde_json::from_slice(payload)
                .map_err(|_| error("malformed Cursor stream trailer"))?;
            if let Some(upstream) = trailer.get("error") {
                let code = upstream
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(match code {
                    "unauthenticated" => {
                        api_error(401, "Cursor authentication expired or rejected")
                    }
                    "resource_exhausted" => RociError::RateLimited {
                        retry_after_ms: None,
                    },
                    "permission_denied" => api_error(403, "Cursor permission denied"),
                    "unavailable" => api_error(503, "Cursor service unavailable"),
                    "invalid_argument" => error("Cursor rejected the request arguments"),
                    "not_found" => error("Cursor requested resource was not found"),
                    "unimplemented" => error("Cursor operation is not implemented by the service"),
                    "failed_precondition" => error("Cursor request precondition failed"),
                    "internal" => error("Cursor internal service error"),
                    "deadline_exceeded" => error("Cursor service deadline exceeded"),
                    "canceled" => error("Cursor service canceled the request"),
                    _ => error("Cursor stream failed"),
                });
            }
        }
        Frame::End
    } else {
        Frame::Message(decode("AgentServerMessage", payload)?)
    };
    buffer.drain(..length + 5);
    Ok(Some(result))
}

pub(super) fn bytes(value: &Value) -> Result<Vec<u8>, RociError> {
    STANDARD
        .decode(value.as_str().unwrap_or_default())
        .map_err(|_| error("invalid Cursor binary field"))
}

pub(super) fn protobuf_value(value: &Value) -> prost_types::Value {
    use prost_types::value::Kind;
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(v) => Kind::BoolValue(*v),
        Value::Number(v) => Kind::NumberValue(v.as_f64().unwrap_or_default()),
        Value::String(v) => Kind::StringValue(v.clone()),
        Value::Array(v) => Kind::ListValue(prost_types::ListValue {
            values: v.iter().map(protobuf_value).collect(),
        }),
        Value::Object(v) => Kind::StructValue(prost_types::Struct {
            fields: v
                .iter()
                .map(|(k, v)| (k.clone(), protobuf_value(v)))
                .collect(),
        }),
    };
    prost_types::Value { kind: Some(kind) }
}

fn from_protobuf_value(value: prost_types::Value) -> Result<Value, RociError> {
    use prost_types::value::Kind;
    Ok(match value.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(v)) => json!(v),
        Some(Kind::NumberValue(v)) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| error("non-finite Cursor tool argument"))?,
        Some(Kind::StringValue(v)) => json!(v),
        Some(Kind::ListValue(v)) => Value::Array(
            v.values
                .into_iter()
                .map(from_protobuf_value)
                .collect::<Result<_, _>>()?,
        ),
        Some(Kind::StructValue(v)) => Value::Object(
            v.fields
                .into_iter()
                .map(|(k, v)| Ok((k, from_protobuf_value(v)?)))
                .collect::<Result<_, RociError>>()?,
        ),
    })
}

pub(super) fn tool_arguments(args: &Value) -> Result<Value, RociError> {
    let mut output = serde_json::Map::new();
    if let Some(args) = args.as_object() {
        for (name, value) in args {
            let raw = bytes(value)?;
            let decoded = prost_types::Value::decode(raw.as_slice())
                .map_err(|_| error("malformed Cursor tool argument"))?;
            output.insert(name.clone(), from_protobuf_value(decoded)?);
        }
    }
    Ok(Value::Object(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_frame_and_trailer_are_decoded_without_losing_tail() {
        let body = encode(
            "AgentServerMessage",
            json!({"interactionUpdate":{"textDelta":{"text":"hello"}}}),
        )
        .unwrap();
        let framed = frame(body).unwrap();
        let mut buffer = framed[..4].to_vec();
        assert!(take_frame(&mut buffer).unwrap().is_none());
        buffer.extend_from_slice(&framed[4..]);
        buffer.extend([2, 0, 0, 0, 2, b'{', b'}']);
        let Some(Frame::Message(value)) = take_frame(&mut buffer).unwrap() else {
            panic!()
        };
        assert_eq!(value["interactionUpdate"]["textDelta"]["text"], "hello");
        assert!(matches!(take_frame(&mut buffer).unwrap(), Some(Frame::End)));
        assert!(buffer.is_empty());
    }

    #[test]
    fn oversized_and_compressed_frames_fail_before_buffering_payload() {
        let mut oversized = vec![0, 1, 0, 0, 0];
        assert!(take_frame(&mut oversized).is_err());
        assert!(take_frame(&mut vec![1, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn tool_arguments_preserve_nested_values() {
        let value = json!({"nested":[true, null, 3.5], "text":"héllo"});
        let bytes = STANDARD.encode(protobuf_value(&value).encode_to_vec());
        assert_eq!(
            tool_arguments(&json!({"payload":bytes})).unwrap(),
            json!({"payload":value})
        );
    }

    #[test]
    fn authentication_trailer_is_typed_for_refresh_without_exposing_body() {
        let body = serde_json::to_vec(
            &json!({"error":{"code":"unauthenticated","message":"sensitive upstream body"}}),
        )
        .unwrap();
        let mut framed = frame(body).unwrap();
        framed[0] = 2;
        let error = take_frame(&mut framed).err().unwrap();
        assert!(matches!(error, RociError::Api { status: 401, .. }));
        assert!(!error.to_string().contains("sensitive"));
    }
}
