//! MCP error mapping functions.

use crate::error::RociError;
use rmcp::service::{ClientInitializeError, ServiceError};

pub(super) fn map_client_initialize_error(error: ClientInitializeError) -> RociError {
    match error {
        ClientInitializeError::ConnectionClosed(context) => {
            RociError::Stream(format!("MCP initialize connection closed: {context}"))
        }
        ClientInitializeError::TransportError { error, context } => RociError::Stream(format!(
            "MCP initialize transport error ({context}): {error}"
        )),
        ClientInitializeError::JsonRpcError(error) => RociError::Provider {
            provider: "mcp".into(),
            message: format!(
                "MCP initialize JSON-RPC error {}: {}",
                error.code.0, error.message
            ),
        },
        ClientInitializeError::Cancelled => RociError::Stream("MCP initialize cancelled".into()),
        other => RociError::Provider {
            provider: "mcp".into(),
            message: format!("MCP initialize error: {other}"),
        },
    }
}

pub(super) fn map_service_error(context: &str, error: ServiceError) -> RociError {
    match error {
        ServiceError::McpError(error) => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: MCP error {}: {}", error.code.0, error.message),
        },
        ServiceError::TransportSend(error) => {
            RociError::Stream(format!("{context}: MCP transport send failed: {error}"))
        }
        ServiceError::TransportClosed => {
            RociError::Stream(format!("{context}: MCP transport closed"))
        }
        ServiceError::UnexpectedResponse => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: unexpected MCP response"),
        },
        ServiceError::Cancelled { reason } => {
            let suffix = reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            RociError::Stream(format!("{context}: MCP request cancelled{suffix}"))
        }
        ServiceError::Timeout { timeout } => RociError::Timeout(timeout.as_millis() as u64),
        other => RociError::Provider {
            provider: "mcp".into(),
            message: format!("{context}: MCP service error: {other}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::service::ServiceError;
    use std::time::Duration;

    #[test]
    fn map_service_error_protocol_violation_maps_to_provider_error() {
        let err = map_service_error("list_tools", ServiceError::UnexpectedResponse);
        assert!(matches!(
            err,
            RociError::Provider { provider, message }
            if provider == "mcp" && message.contains("unexpected MCP response")
        ));
    }

    #[test]
    fn map_service_error_timeout_maps_to_timeout_error() {
        let err = map_service_error(
            "call_tool",
            ServiceError::Timeout {
                timeout: Duration::from_millis(2750),
            },
        );
        assert!(matches!(err, RociError::Timeout(2750)));
    }

    #[test]
    fn map_service_error_cancelled_reason_is_preserved() {
        let err = map_service_error(
            "call_tool",
            ServiceError::Cancelled {
                reason: Some("client cancelled".into()),
            },
        );
        assert!(matches!(
            err,
            RociError::Stream(message) if message.contains("client cancelled")
        ));
    }
}
