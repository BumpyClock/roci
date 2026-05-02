//! Human interaction contracts shared by tools, runtimes, and hosts.
//!
//! This module owns the provider-neutral interaction lifecycle. Model-facing
//! tools, MCP adapters, and permission prompts should map into these typed
//! request payloads instead of inventing separate request queues.

use std::collections::HashMap;
#[cfg(feature = "agent")]
use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot, Mutex};
use uuid::Uuid;

#[cfg(feature = "agent")]
use crate::agent_loop::{ApprovalDecision, ApprovalRequest};
use crate::tools::{
    AskUserPrompt, UnknownUserInputRequest, UserInputError, UserInputRequest, UserInputRequestId,
    UserInputResponse, UserInputResult,
};
#[cfg(feature = "agent")]
use crate::types::AgentToolCall;

/// Unique identifier for a human interaction request.
pub type HumanInteractionRequestId = Uuid;

/// Origin of a human interaction request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HumanInteractionSource {
    /// Request originated from a model-visible tool.
    ModelTool {
        /// Tool call id from provider output.
        tool_call_id: String,
        /// Tool name that requested interaction.
        tool_name: String,
    },
    /// Request originated from an MCP server.
    Mcp {
        /// MCP server identifier.
        server_id: String,
        /// Optional MCP operation that produced the nested request.
        operation: Option<String>,
    },
    /// Request originated from tool permission policy.
    ToolPermission {
        /// Tool call id when available.
        tool_call_id: Option<String>,
        /// Tool name requesting permission.
        tool_name: String,
    },
    /// Request originated from host/runtime code.
    Host,
}

/// Shape of a human interaction request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HumanInteractionPayload {
    /// Model-facing user input request.
    AskUser(AskUserRequest),
    /// Host/protocol-facing structured UI elicitation.
    UiElicitation(UiElicitationRequest),
    /// Tool permission prompt.
    #[cfg(feature = "agent")]
    ToolPermission(ToolPermissionRequest),
}

/// Request envelope tracked by the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanInteractionRequest {
    /// Unique request id.
    pub request_id: HumanInteractionRequestId,
    /// Request origin.
    pub source: HumanInteractionSource,
    /// Interaction payload.
    pub payload: HumanInteractionPayload,
    /// Optional timeout in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl HumanInteractionRequest {
    /// Build a human interaction request for the current `ask_user` tool.
    #[must_use]
    pub fn from_user_input(request: UserInputRequest) -> Self {
        Self {
            request_id: request.request_id,
            source: HumanInteractionSource::ModelTool {
                tool_call_id: request.tool_call_id.clone(),
                tool_name: "ask_user".to_string(),
            },
            payload: HumanInteractionPayload::AskUser(AskUserRequest {
                prompt: request.prompt,
            }),
            timeout_ms: request.timeout_ms,
            created_at: Utc::now(),
        }
    }

    /// Convert an `AskUser` payload into the current ask_user request shape.
    #[must_use]
    pub fn to_user_input(&self) -> Option<UserInputRequest> {
        let HumanInteractionPayload::AskUser(payload) = &self.payload else {
            return None;
        };
        let tool_call_id = match &self.source {
            HumanInteractionSource::ModelTool { tool_call_id, .. } => tool_call_id.clone(),
            _ => String::new(),
        };
        Some(UserInputRequest {
            request_id: self.request_id,
            tool_call_id,
            prompt: payload.prompt.clone(),
            timeout_ms: self.timeout_ms,
        })
    }
}

/// Model-facing `ask_user` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskUserRequest {
    /// Prompt to ask user.
    pub prompt: AskUserPrompt,
}

/// Host/protocol-facing UI elicitation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiElicitationRequest {
    /// Elicitation mode. MVP supports only form mode.
    #[serde(default)]
    pub mode: UiElicitationMode,
    /// Message shown to user.
    pub message: String,
    /// Form schema. MVP intentionally supports only flat object forms.
    #[serde(rename = "requestedSchema")]
    pub requested_schema: UiElicitationSchema,
}

/// MCP `elicitation/create` params before MVP capability validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiElicitationCreateParams {
    /// Elicitation mode. MCP treats omitted mode as form mode.
    #[serde(default)]
    pub mode: UiElicitationMode,
    /// Message shown to user.
    pub message: String,
    /// Form schema. Required for form mode.
    #[serde(default, rename = "requestedSchema")]
    pub requested_schema: Option<UiElicitationSchema>,
    /// URL for URL mode. MVP parses but rejects URL mode.
    #[serde(default)]
    pub url: Option<String>,
    /// URL mode correlation id. MVP parses but rejects URL mode.
    #[serde(default, rename = "elicitationId")]
    pub elicitation_id: Option<String>,
}

impl TryFrom<UiElicitationCreateParams> for UiElicitationRequest {
    type Error = UiElicitationValidationError;

    fn try_from(params: UiElicitationCreateParams) -> Result<Self, Self::Error> {
        if params.mode == UiElicitationMode::Url {
            return Err(UiElicitationValidationError::UnsupportedMode(
                UiElicitationMode::Url,
            ));
        }

        let requested_schema = params
            .requested_schema
            .ok_or(UiElicitationValidationError::MissingRequestedSchema)?;
        Self::form(params.message, requested_schema)
    }
}

impl UiElicitationRequest {
    /// Build a supported form-mode UI elicitation request.
    ///
    /// # Errors
    ///
    /// Returns [`UiElicitationValidationError`] if `requested_schema` is not in
    /// the supported MCP-compatible form subset.
    pub fn form(
        message: impl Into<String>,
        requested_schema: UiElicitationSchema,
    ) -> Result<Self, UiElicitationValidationError> {
        requested_schema.validate()?;
        Ok(Self {
            mode: UiElicitationMode::Form,
            message: message.into(),
            requested_schema,
        })
    }

    /// Validate this request against the MVP supported elicitation subset.
    ///
    /// # Errors
    ///
    /// Returns [`UiElicitationValidationError`] for unsupported URL mode or
    /// unsupported schema forms.
    pub fn validate(&self) -> Result<(), UiElicitationValidationError> {
        if self.mode == UiElicitationMode::Url {
            return Err(UiElicitationValidationError::UnsupportedMode(
                UiElicitationMode::Url,
            ));
        }
        self.requested_schema.validate()
    }
}

/// UI elicitation mode from MCP.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiElicitationMode {
    /// In-band structured form data collection.
    #[default]
    Form,
    /// Out-of-band URL navigation. Not supported in MVP.
    Url,
}

/// Form schema for UI elicitation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiElicitationSchema {
    /// MCP-compatible schema root type. MVP supports only object schemas.
    #[serde(rename = "type")]
    pub schema_type: UiElicitationSchemaType,
    /// Field definitions keyed by field name.
    pub properties: HashMap<String, UiElicitationField>,
    /// Required field names.
    #[serde(default)]
    pub required: Vec<String>,
}

impl UiElicitationSchema {
    /// Build a flat object form schema.
    #[must_use]
    pub fn object(properties: HashMap<String, UiElicitationField>, required: Vec<String>) -> Self {
        Self {
            schema_type: UiElicitationSchemaType::Object,
            properties,
            required,
        }
    }

    /// Validate schema is in the supported MCP-compatible form subset.
    ///
    /// # Errors
    ///
    /// Returns [`UiElicitationValidationError`] when the schema is not a flat
    /// object or its `required` list references an unknown field.
    pub fn validate(&self) -> Result<(), UiElicitationValidationError> {
        if self.schema_type != UiElicitationSchemaType::Object {
            return Err(UiElicitationValidationError::UnsupportedSchemaRoot);
        }

        for field_name in &self.required {
            if !self.properties.contains_key(field_name) {
                return Err(UiElicitationValidationError::UnknownRequiredField(
                    field_name.clone(),
                ));
            }
        }

        Ok(())
    }
}

/// Supported UI elicitation schema root type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiElicitationSchemaType {
    /// JSON object schema.
    Object,
}

/// Supported UI elicitation field types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiElicitationField {
    String {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
        enum_values: Option<Vec<String>>,
    },
    Boolean {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    Number {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<f64>,
    },
    Integer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
    },
}

/// UI elicitation capabilities advertised by a host/client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiElicitationCapabilities {
    /// Supports form mode.
    pub form: bool,
    /// Supports URL mode.
    pub url: bool,
}

impl Default for UiElicitationCapabilities {
    fn default() -> Self {
        Self {
            form: true,
            url: false,
        }
    }
}

impl UiElicitationCapabilities {
    /// MVP capability set: form mode supported, URL mode unsupported.
    #[must_use]
    pub const fn form_only() -> Self {
        Self {
            form: true,
            url: false,
        }
    }

    /// Return whether this capability set supports `mode`.
    #[must_use]
    pub const fn supports(self, mode: UiElicitationMode) -> bool {
        match mode {
            UiElicitationMode::Form => self.form,
            UiElicitationMode::Url => self.url,
        }
    }

    /// Validate request mode against capabilities and supported MVP subset.
    ///
    /// # Errors
    ///
    /// Returns [`UiElicitationValidationError`] when mode is unsupported or the
    /// request schema is invalid.
    pub fn validate_request(
        self,
        request: &UiElicitationRequest,
    ) -> Result<(), UiElicitationValidationError> {
        if !self.supports(request.mode) {
            return Err(UiElicitationValidationError::UnsupportedMode(request.mode));
        }
        request.validate()
    }
}

/// UI elicitation validation error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiElicitationValidationError {
    /// Elicitation mode is not supported.
    UnsupportedMode(UiElicitationMode),
    /// Form mode request did not include `requestedSchema`.
    MissingRequestedSchema,
    /// Schema root is not a supported object.
    UnsupportedSchemaRoot,
    /// Required list references a field not present in `properties`.
    UnknownRequiredField(String),
}

impl std::fmt::Display for UiElicitationValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMode(mode) => {
                write!(formatter, "unsupported UI elicitation mode: {mode:?}")
            }
            Self::MissingRequestedSchema => {
                write!(
                    formatter,
                    "UI elicitation form request missing requestedSchema"
                )
            }
            Self::UnsupportedSchemaRoot => {
                write!(formatter, "UI elicitation schema must be an object")
            }
            Self::UnknownRequiredField(field_name) => {
                write!(
                    formatter,
                    "UI elicitation required field is not defined: {field_name}"
                )
            }
        }
    }
}

impl std::error::Error for UiElicitationValidationError {}

/// Tool permission request payload.
#[cfg(feature = "agent")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolPermissionRequest {
    /// Existing approval request shape until tool catalog policy owns richer metadata.
    pub approval: ApprovalRequest,
    /// Permission category shown to host UI.
    pub kind: ToolPermissionKind,
    /// Tool call id when available.
    pub tool_call_id: Option<String>,
    /// Tool name requesting permission.
    pub tool_name: String,
    /// Tool arguments being approved.
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Exact key eligible for allow-for-session reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<ToolPermissionSessionKey>,
}

/// Permission category for tool approval prompts.
#[cfg(feature = "agent")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionKind {
    Shell,
    Write,
    Read,
    Mcp,
    CustomTool,
    Other,
}

/// Stable allow-for-session cache key.
#[cfg(feature = "agent")]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolPermissionSessionKey(pub String);

#[cfg(feature = "agent")]
impl ToolPermissionSessionKey {
    /// Build an exact cache key for a tool permission request.
    #[must_use]
    pub fn for_tool_call(kind: ToolPermissionKind, call: &AgentToolCall) -> Self {
        let arguments =
            serde_json::to_string(&call.arguments).unwrap_or_else(|_| "null".to_string());
        Self(format!(
            "{kind:?}|{}|{}|{}",
            call.recipient.as_deref().unwrap_or_default(),
            call.name,
            arguments
        ))
    }
}

/// Shared permission approval cache for a runtime session.
#[cfg(feature = "agent")]
pub type ToolPermissionSessionApprovals = Arc<Mutex<HashSet<ToolPermissionSessionKey>>>;

/// Approval decision returned by host UI for a tool permission prompt.
#[cfg(feature = "agent")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionDecision {
    AllowOnce,
    AllowForSession,
    Deny,
    Cancel,
}

#[cfg(feature = "agent")]
impl From<ToolPermissionDecision> for ApprovalDecision {
    fn from(value: ToolPermissionDecision) -> Self {
        match value {
            ToolPermissionDecision::AllowOnce => Self::Accept,
            ToolPermissionDecision::AllowForSession => Self::AcceptForSession,
            ToolPermissionDecision::Deny => Self::Decline,
            ToolPermissionDecision::Cancel => Self::Cancel,
        }
    }
}

#[cfg(feature = "agent")]
impl From<ApprovalDecision> for ToolPermissionDecision {
    fn from(value: ApprovalDecision) -> Self {
        match value {
            ApprovalDecision::Accept => Self::AllowOnce,
            ApprovalDecision::AcceptForSession => Self::AllowForSession,
            ApprovalDecision::Decline => Self::Deny,
            ApprovalDecision::Cancel => Self::Cancel,
        }
    }
}

/// Response envelope tracked by the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanInteractionResponse {
    /// Request id this response resolves.
    pub request_id: HumanInteractionRequestId,
    /// Response payload.
    pub payload: HumanInteractionResponsePayload,
    /// Resolution timestamp.
    pub resolved_at: DateTime<Utc>,
}

impl HumanInteractionResponse {
    /// Build a response from current `ask_user` response type.
    #[must_use]
    pub fn from_user_input(response: UserInputResponse) -> Self {
        let request_id = response.request_id;
        let payload = match response.result {
            UserInputResult::Canceled => HumanInteractionResponsePayload::Canceled,
            result => HumanInteractionResponsePayload::AskUser(AskUserResponse { result }),
        };
        Self {
            request_id,
            payload,
            resolved_at: Utc::now(),
        }
    }
}

/// Response payload for a human interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HumanInteractionResponsePayload {
    AskUser(AskUserResponse),
    UiElicitation(UiElicitationResponse),
    #[cfg(feature = "agent")]
    ToolPermission(ToolPermissionResponse),
    Declined,
    Canceled,
}

/// `ask_user` response payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskUserResponse {
    /// Typed ask_user response payload.
    pub result: UserInputResult,
}

/// UI elicitation response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UiElicitationResponse {
    Accept {
        #[serde(default)]
        content: serde_json::Map<String, serde_json::Value>,
    },
    Decline,
    Cancel,
}

/// Tool permission response payload.
#[cfg(feature = "agent")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPermissionResponse {
    /// Host decision for the tool permission prompt.
    pub decision: ToolPermissionDecision,
}

/// Error returned when submitting a response for an unknown request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnknownHumanInteractionRequest(pub HumanInteractionRequestId);

impl std::fmt::Display for UnknownHumanInteractionRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unknown human interaction request: {}", self.0)
    }
}

impl std::error::Error for UnknownHumanInteractionRequest {}

/// Errors that can occur during human interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HumanInteractionError {
    UnknownRequest {
        request_id: HumanInteractionRequestId,
    },
    Timeout {
        request_id: HumanInteractionRequestId,
    },
    Canceled {
        request_id: HumanInteractionRequestId,
    },
    Unavailable {
        request_id: HumanInteractionRequestId,
        reason: String,
    },
}

impl std::fmt::Display for HumanInteractionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRequest { request_id } => {
                write!(formatter, "unknown human interaction request: {request_id}")
            }
            Self::Timeout { request_id } => {
                write!(
                    formatter,
                    "human interaction request timed out: {request_id}"
                )
            }
            Self::Canceled { request_id } => {
                write!(
                    formatter,
                    "human interaction request canceled: {request_id}"
                )
            }
            Self::Unavailable { request_id, reason } => {
                write!(
                    formatter,
                    "human interaction unavailable for request {request_id}: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for HumanInteractionError {}

type HumanInteractionOutcome = Result<HumanInteractionResponse, HumanInteractionError>;
type PendingSender = oneshot::Sender<HumanInteractionOutcome>;
type PendingReceiver = oneshot::Receiver<HumanInteractionOutcome>;

#[derive(Debug)]
struct PendingHumanInteractionRecord {
    request: HumanInteractionRequest,
    tx: PendingSender,
}

/// An in-flight human interaction request owned by the waiter.
#[derive(Debug)]
pub struct PendingHumanInteraction {
    coordinator: HumanInteractionCoordinator,
    request_id: HumanInteractionRequestId,
    rx: PendingReceiver,
}

/// Coordinates human interaction requests and responses.
#[derive(Debug, Clone)]
pub struct HumanInteractionCoordinator {
    pending: Arc<Mutex<HashMap<HumanInteractionRequestId, PendingHumanInteractionRecord>>>,
    completion_tx: broadcast::Sender<HumanInteractionRequestId>,
}

impl Default for HumanInteractionCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl HumanInteractionCoordinator {
    /// Create a new coordinator.
    #[must_use]
    pub fn new() -> Self {
        let (completion_tx, _) = broadcast::channel(32);
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            completion_tx,
        }
    }

    /// Register a request and return a pending handle.
    pub async fn create_request(
        &self,
        request: HumanInteractionRequest,
    ) -> Result<PendingHumanInteraction, HumanInteractionError> {
        let (tx, rx) = oneshot::channel();
        let request_id = request.request_id;

        let mut pending = self.pending.lock().await;
        pending.insert(request_id, PendingHumanInteractionRecord { request, tx });

        Ok(PendingHumanInteraction {
            coordinator: self.clone(),
            request_id,
            rx,
        })
    }

    /// Submit a response for a pending request.
    pub async fn submit_response(
        &self,
        response: HumanInteractionResponse,
    ) -> Result<(), UnknownHumanInteractionRequest> {
        let request_id = response.request_id;
        let mut pending = self.pending.lock().await;

        if let Some(record) = pending.remove(&request_id) {
            let _ = record.tx.send(Ok(response));
            let _ = self.completion_tx.send(request_id);
            Ok(())
        } else {
            Err(UnknownHumanInteractionRequest(request_id))
        }
    }

    /// Submit an error for a pending request.
    pub async fn submit_error(
        &self,
        request_id: HumanInteractionRequestId,
        error: HumanInteractionError,
    ) -> Result<(), UnknownHumanInteractionRequest> {
        let mut pending = self.pending.lock().await;

        if let Some(record) = pending.remove(&request_id) {
            let _ = record.tx.send(Err(error));
            let _ = self.completion_tx.send(request_id);
            Ok(())
        } else {
            Err(UnknownHumanInteractionRequest(request_id))
        }
    }

    /// Register current ask_user request shape through the shared coordinator.
    pub async fn create_user_input_request(
        &self,
        request: UserInputRequest,
    ) -> Result<PendingHumanInteraction, UserInputError> {
        self.create_request(HumanInteractionRequest::from_user_input(request))
            .await
            .map_err(Into::into)
    }

    /// Submit current ask_user response shape through the shared coordinator.
    pub async fn submit_user_input_response(
        &self,
        response: UserInputResponse,
    ) -> Result<(), UnknownUserInputRequest> {
        self.submit_response(HumanInteractionResponse::from_user_input(response))
            .await
            .map_err(|err| UnknownUserInputRequest(err.0))
    }

    /// Submit current ask_user error shape through the shared coordinator.
    pub async fn submit_user_input_error(
        &self,
        request_id: UserInputRequestId,
        error: UserInputError,
    ) -> Result<(), UnknownUserInputRequest> {
        self.submit_error(request_id, HumanInteractionError::from(error))
            .await
            .map_err(|err| UnknownUserInputRequest(err.0))
    }

    /// Register a tool permission request through the shared coordinator.
    #[cfg(feature = "agent")]
    pub async fn create_tool_permission_request(
        &self,
        request: HumanInteractionRequest,
    ) -> Result<PendingHumanInteraction, HumanInteractionError> {
        self.create_request(request).await
    }

    /// Submit a tool permission decision through the shared coordinator.
    #[cfg(feature = "agent")]
    pub async fn submit_tool_permission_response(
        &self,
        request_id: HumanInteractionRequestId,
        decision: ToolPermissionDecision,
    ) -> Result<(), UnknownHumanInteractionRequest> {
        self.submit_response(HumanInteractionResponse {
            request_id,
            payload: HumanInteractionResponsePayload::ToolPermission(ToolPermissionResponse {
                decision,
            }),
            resolved_at: Utc::now(),
        })
        .await
    }

    /// Cancel all pending requests.
    pub async fn cancel_all(&self) {
        let mut pending = self.pending.lock().await;
        let pending_ids = pending.keys().copied().collect::<Vec<_>>();
        pending.clear();
        drop(pending);

        for request_id in pending_ids {
            let _ = self.completion_tx.send(request_id);
        }
    }

    /// Return whether a request is still pending.
    pub async fn is_pending(&self, request_id: HumanInteractionRequestId) -> bool {
        let pending = self.pending.lock().await;
        pending.contains_key(&request_id)
    }

    /// Return a pending request snapshot when the request is still unresolved.
    pub async fn pending_request(
        &self,
        request_id: HumanInteractionRequestId,
    ) -> Option<HumanInteractionRequest> {
        let pending = self.pending.lock().await;
        pending
            .get(&request_id)
            .map(|record| record.request.clone())
    }

    /// Return pending request snapshots.
    pub async fn pending_requests(&self) -> Vec<HumanInteractionRequest> {
        let pending = self.pending.lock().await;
        pending
            .values()
            .map(|record| record.request.clone())
            .collect()
    }

    /// Subscribe to request completion notifications.
    pub fn subscribe_completion(&self) -> broadcast::Receiver<HumanInteractionRequestId> {
        self.completion_tx.subscribe()
    }

    async fn remove_request(&self, request_id: HumanInteractionRequestId) {
        let mut pending = self.pending.lock().await;
        let removed = pending.remove(&request_id).is_some();
        drop(pending);
        if removed {
            let _ = self.completion_tx.send(request_id);
        }
    }
}

impl PendingHumanInteraction {
    /// Wait for response with optional timeout.
    pub async fn wait(
        self,
        timeout_ms: Option<u64>,
    ) -> Result<HumanInteractionResponse, HumanInteractionError> {
        let request_id = self.request_id;
        let coordinator = self.coordinator;
        let rx = self.rx;

        let result = if let Some(ms) = timeout_ms {
            match tokio::time::timeout(std::time::Duration::from_millis(ms), rx).await {
                Ok(result) => result,
                Err(_) => {
                    coordinator.remove_request(request_id).await;
                    return Err(HumanInteractionError::Timeout { request_id });
                }
            }
        } else {
            rx.await
        };

        match result {
            Ok(Ok(response)) => match response.payload {
                HumanInteractionResponsePayload::Canceled => {
                    Err(HumanInteractionError::Canceled { request_id })
                }
                _ => Ok(response),
            },
            Ok(Err(error)) => Err(error),
            Err(_) => {
                coordinator.remove_request(request_id).await;
                Err(HumanInteractionError::Canceled { request_id })
            }
        }
    }

    /// Wait for current ask_user response shape.
    pub async fn wait_user_input(
        self,
        timeout_ms: Option<u64>,
    ) -> Result<UserInputResponse, UserInputError> {
        let request_id = self.request_id;
        match self.wait(timeout_ms).await {
            Ok(response) => match response.payload {
                HumanInteractionResponsePayload::AskUser(response) => Ok(UserInputResponse {
                    request_id,
                    result: response.result,
                }),
                HumanInteractionResponsePayload::Canceled => {
                    Err(UserInputError::Canceled { request_id })
                }
                _ => Err(UserInputError::InteractivePromptUnavailable {
                    request_id,
                    reason: "human interaction response was not ask_user".to_string(),
                }),
            },
            Err(error) => Err(error.into_user_input_error()),
        }
    }

    /// Wait for a tool permission decision.
    #[cfg(feature = "agent")]
    pub async fn wait_tool_permission(
        self,
        timeout_ms: Option<u64>,
    ) -> Result<ToolPermissionDecision, HumanInteractionError> {
        let request_id = self.request_id;
        match self.wait(timeout_ms).await {
            Ok(response) => match response.payload {
                HumanInteractionResponsePayload::ToolPermission(response) => Ok(response.decision),
                HumanInteractionResponsePayload::Declined => Ok(ToolPermissionDecision::Deny),
                HumanInteractionResponsePayload::Canceled => {
                    Err(HumanInteractionError::Canceled { request_id })
                }
                _ => Err(HumanInteractionError::Unavailable {
                    request_id,
                    reason: "human interaction response was not tool_permission".to_string(),
                }),
            },
            Err(error) => Err(error),
        }
    }
}

impl From<UserInputError> for HumanInteractionError {
    fn from(value: UserInputError) -> Self {
        match value {
            UserInputError::UnknownRequest { request_id } => Self::UnknownRequest { request_id },
            UserInputError::Timeout { request_id } => Self::Timeout { request_id },
            UserInputError::Canceled { request_id } => Self::Canceled { request_id },
            UserInputError::InteractivePromptUnavailable { request_id, reason } => {
                Self::Unavailable { request_id, reason }
            }
            UserInputError::NoCallback => Self::Unavailable {
                request_id: Uuid::nil(),
                reason: "no user input callback configured".to_string(),
            },
        }
    }
}

impl From<HumanInteractionError> for UserInputError {
    fn from(value: HumanInteractionError) -> Self {
        value.into_user_input_error()
    }
}

impl HumanInteractionError {
    fn into_user_input_error(self) -> UserInputError {
        match self {
            Self::UnknownRequest { request_id } => UserInputError::UnknownRequest { request_id },
            Self::Timeout { request_id } => UserInputError::Timeout { request_id },
            Self::Canceled { request_id } => UserInputError::Canceled { request_id },
            Self::Unavailable { request_id, reason } => {
                UserInputError::InteractivePromptUnavailable { request_id, reason }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_input_request(request_id: Uuid) -> UserInputRequest {
        UserInputRequest {
            request_id,
            tool_call_id: "call_123".to_string(),
            prompt: AskUserPrompt::Question {
                id: "unit".to_string(),
                question: "C or F?".to_string(),
                placeholder: None,
                default: None,
                multiline: false,
            },
            timeout_ms: None,
        }
    }

    fn ui_schema() -> UiElicitationSchema {
        UiElicitationSchema::object(
            HashMap::from([
                (
                    "name".to_string(),
                    UiElicitationField::String {
                        title: Some("Name".to_string()),
                        description: Some("Display name".to_string()),
                        default: None,
                        enum_values: None,
                    },
                ),
                (
                    "age".to_string(),
                    UiElicitationField::Integer {
                        title: Some("Age".to_string()),
                        description: None,
                        default: Some(30),
                    },
                ),
                (
                    "notifications".to_string(),
                    UiElicitationField::Boolean {
                        title: None,
                        description: None,
                        default: Some(true),
                    },
                ),
            ]),
            vec!["name".to_string()],
        )
    }

    #[test]
    fn ui_elicitation_request_serializes_mcp_form_shape_with_metadata_envelope() {
        let request_id = Uuid::new_v4();
        let request = HumanInteractionRequest {
            request_id,
            source: HumanInteractionSource::Mcp {
                server_id: "github".to_string(),
                operation: Some("elicitation/create".to_string()),
            },
            payload: HumanInteractionPayload::UiElicitation(
                UiElicitationRequest::form("Need profile details", ui_schema()).unwrap(),
            ),
            timeout_ms: Some(30_000),
            created_at: Utc::now(),
        };

        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["request_id"], request_id.to_string());
        assert_eq!(value["source"]["type"], "mcp");
        assert_eq!(value["source"]["server_id"], "github");
        assert_eq!(value["payload"]["type"], "ui_elicitation");
        assert_eq!(value["payload"]["mode"], "form");
        assert_eq!(value["payload"]["message"], "Need profile details");
        assert_eq!(value["payload"]["requestedSchema"]["type"], "object");
        assert_eq!(
            value["payload"]["requestedSchema"]["required"],
            serde_json::json!(["name"])
        );
        assert_eq!(
            value["payload"]["requestedSchema"]["properties"]["name"]["type"],
            "string"
        );
        assert_eq!(
            value["payload"]["requestedSchema"]["properties"]["age"]["type"],
            "integer"
        );
        assert_eq!(
            value["payload"]["requestedSchema"]["properties"]["notifications"]["type"],
            "boolean"
        );
    }

    #[test]
    fn ui_elicitation_response_supports_accept_decline_and_cancel_actions() {
        let accepted = UiElicitationResponse::Accept {
            content: serde_json::Map::from_iter([(
                "name".to_string(),
                serde_json::Value::String("octocat".to_string()),
            )]),
        };

        let accepted_json = serde_json::to_value(&accepted).unwrap();
        let declined_json = serde_json::to_value(UiElicitationResponse::Decline).unwrap();
        let canceled_json = serde_json::to_value(UiElicitationResponse::Cancel).unwrap();

        assert_eq!(accepted_json["action"], "accept");
        assert_eq!(accepted_json["content"]["name"], "octocat");
        assert_eq!(declined_json, serde_json::json!({ "action": "decline" }));
        assert_eq!(canceled_json, serde_json::json!({ "action": "cancel" }));
        assert_eq!(
            serde_json::from_value::<UiElicitationResponse>(declined_json).unwrap(),
            UiElicitationResponse::Decline
        );
    }

    #[test]
    fn ui_elicitation_schema_rejects_required_field_not_in_properties() {
        let schema = UiElicitationSchema::object(HashMap::new(), vec!["missing".to_string()]);

        let error = schema.validate().unwrap_err();

        assert_eq!(
            error,
            UiElicitationValidationError::UnknownRequiredField("missing".to_string())
        );
    }

    #[test]
    fn ui_elicitation_schema_rejects_non_object_root_and_non_primitive_fields() {
        let non_object = serde_json::json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let nested_object_field = serde_json::json!({
            "type": "object",
            "properties": {
                "profile": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        });

        assert!(serde_json::from_value::<UiElicitationSchema>(non_object).is_err());
        assert!(serde_json::from_value::<UiElicitationSchema>(nested_object_field).is_err());
    }

    #[test]
    fn ui_elicitation_rejects_url_mode_for_mvp() {
        let params: UiElicitationCreateParams = serde_json::from_value(serde_json::json!({
            "mode": "url",
            "message": "Open auth URL",
            "url": "https://example.com/auth",
            "elicitationId": "550e8400-e29b-41d4-a716-446655440000"
        }))
        .unwrap();

        let error = UiElicitationRequest::try_from(params).unwrap_err();

        assert_eq!(
            error,
            UiElicitationValidationError::UnsupportedMode(UiElicitationMode::Url)
        );
    }

    #[test]
    fn ui_elicitation_capabilities_support_form_and_gate_url() {
        let capabilities = UiElicitationCapabilities::form_only();
        let form_request = UiElicitationRequest::form("Need profile details", ui_schema()).unwrap();
        let url_request = UiElicitationRequest {
            mode: UiElicitationMode::Url,
            message: "Open auth URL".to_string(),
            requested_schema: ui_schema(),
        };

        assert!(capabilities.validate_request(&form_request).is_ok());
        assert_eq!(
            capabilities.validate_request(&url_request).unwrap_err(),
            UiElicitationValidationError::UnsupportedMode(UiElicitationMode::Url)
        );
    }

    #[tokio::test]
    async fn create_and_submit_response() {
        let coordinator = HumanInteractionCoordinator::new();
        let request_id = Uuid::new_v4();
        let pending = coordinator
            .create_user_input_request(user_input_request(request_id))
            .await
            .unwrap();

        coordinator
            .submit_user_input_response(UserInputResponse {
                request_id,
                result: UserInputResult::Question {
                    answer: "C".to_string(),
                },
            })
            .await
            .unwrap();

        let response = pending.wait_user_input(None).await.unwrap();
        assert_eq!(response.request_id, request_id);
        assert!(matches!(
            response.result,
            UserInputResult::Question { ref answer } if answer == "C"
        ));
    }

    #[tokio::test]
    async fn wait_times_out_and_rejects_late_submit() {
        let coordinator = HumanInteractionCoordinator::new();
        let request_id = Uuid::new_v4();
        let pending = coordinator
            .create_user_input_request(user_input_request(request_id))
            .await
            .unwrap();

        let result = pending.wait_user_input(Some(10)).await;
        assert!(matches!(result, Err(UserInputError::Timeout { .. })));

        let late = coordinator
            .submit_user_input_response(UserInputResponse {
                request_id,
                result: UserInputResult::Question {
                    answer: "C".to_string(),
                },
            })
            .await;
        assert!(matches!(late, Err(UnknownUserInputRequest(_))));
    }

    #[tokio::test]
    async fn cancel_all_unblocks_waiter() {
        let coordinator = HumanInteractionCoordinator::new();
        let request_id = Uuid::new_v4();
        let pending = coordinator
            .create_user_input_request(user_input_request(request_id))
            .await
            .unwrap();

        coordinator.cancel_all().await;

        let result = pending.wait_user_input(None).await;
        assert!(matches!(result, Err(UserInputError::Canceled { .. })));
    }
}
