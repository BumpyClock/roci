//! Tests for SubagentSupervisor lifecycle and public runtime behavior.
//!
//! Drop-behavior tests and internal-access tests live in
//! `supervisor.rs`'s own `mod tests` block. These tests exercise the
//! public API only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use uuid::Uuid;

use crate::agent::runtime::AgentConfig;
use crate::agent::subagents::profiles::SubagentProfileRegistry;
use crate::agent::subagents::supervisor::SubagentSupervisor;
use crate::agent::subagents::types::SubagentEvent;
use crate::agent::subagents::types::{
    ModelCandidate, SnapshotMode, SubagentContext, SubagentInput, SubagentProfile, SubagentSpec,
    SubagentStatus, SubagentSupervisorConfig, ToolPolicy,
};
use crate::agent::subagents::SubagentPromptPolicy;
use crate::agent_loop::AgentEvent;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::{LanguageModel, ModelCapabilities, ModelInputCapabilities};
use crate::provider::{
    ModelProvider, ProviderFactory, ProviderRegistry, ProviderRequest, ProviderResponse,
};
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::tool::Tool;
use crate::tools::{
    AgentTool, AgentToolParameters, AskUserPrompt, ToolArguments, ToolExecutionContext,
    ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary, ToolVisibilityPolicy, UserInputRequest,
    UserInputResult,
};
use crate::types::{ModelMessage, Role, StreamEventType, TextStreamDelta, Usage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_test_model() -> LanguageModel {
    LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    }
}

fn host_input_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: ToolSafetyKind::Other,
    }
}

fn make_base_config() -> AgentConfig {
    use crate::agent::runtime::QueueDrainMode;
    use crate::agent_loop::runner::RetryBackoffPolicy;
    use crate::resource::CompactionSettings;
    use crate::types::GenerationSettings;

    AgentConfig {
        candidates: vec![make_test_model()],
        system_prompt: None,
        tools: Vec::new(),
        tool_visibility_policy: Default::default(),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        approval_policy: Default::default(),
        approval_handler: None,
        session_id: None,
        session: None,
        workspace_root: None,
        sandbox_provider: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        retry_mode: Default::default(),
        model_health: Default::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        #[cfg(feature = "agent")]
        human_interaction_coordinator: None,
        context_budget: None,
        chat: Default::default(),
        #[cfg(feature = "agent")]
        subagents: None,
    }
}

fn make_supervisor() -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let sup_config = SubagentSupervisorConfig::default();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

fn make_supervisor_with_config(sup_config: SubagentSupervisorConfig) -> SubagentSupervisor {
    let registry = Arc::new(ProviderRegistry::new());
    let roci_config = RociConfig::default();
    let base_config = make_base_config();
    let profile_registry = SubagentProfileRegistry::with_builtins();
    SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        sup_config,
        profile_registry,
    )
}

struct RecordingProviderFactory {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    response_text: String,
    tool_calls: Vec<String>,
}

impl RecordingProviderFactory {
    fn new(requests: Arc<Mutex<Vec<ProviderRequest>>>, response_text: impl Into<String>) -> Self {
        Self {
            requests,
            response_text: response_text.into(),
            tool_calls: Vec::new(),
        }
    }
}

impl ProviderFactory for RecordingProviderFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(RecordingProvider {
            provider_key: "test".into(),
            model_id: model_id.to_string(),
            requests: self.requests.clone(),
            response_text: self.response_text.clone(),
            tool_calls: self.tool_calls.clone(),
            capabilities: ModelCapabilities {
                supports_streaming: false,
                input: ModelInputCapabilities::default(),
                ..ModelCapabilities::default()
            },
        }))
    }
}

struct RecordingProvider {
    provider_key: String,
    model_id: String,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
    response_text: String,
    tool_calls: Vec<String>,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for RecordingProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "recording test provider uses stream_text".into(),
        ))
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.requests
            .lock()
            .expect("request capture lock should not be poisoned")
            .push(request.clone());

        if !self.tool_calls.is_empty()
            && !request
                .messages
                .iter()
                .any(|message| message.role == Role::Tool)
        {
            let mut deltas = self
                .tool_calls
                .iter()
                .map(|name| {
                    Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(crate::types::AgentToolCall {
                            id: format!("call-{name}"),
                            name: name.clone(),
                            arguments: serde_json::json!({}),
                            called_as: None,
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })
                })
                .collect::<Vec<_>>();
            deltas.push(Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }));
            return Ok(Box::pin(stream::iter(deltas)));
        }

        Ok(Box::pin(futures::stream::iter(vec![
            Ok(TextStreamDelta {
                text: self.response_text.clone(),
                event_type: StreamEventType::TextDelta,
                tool_call: None,
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ])))
    }
}

fn make_recording_supervisor(
    response_text: &str,
) -> (SubagentSupervisor, Arc<Mutex<Vec<ProviderRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(RecordingProviderFactory::new(
        requests.clone(),
        response_text,
    )));

    let roci_config = RociConfig::default();
    roci_config.set_api_key("test", "test-key".into());

    let mut profile_registry = SubagentProfileRegistry::with_builtins();
    profile_registry
        .register(SubagentProfile {
            name: "test:dev".into(),
            system_prompt: Some("You are a test sub-agent.".into()),
            models: vec![ModelCandidate {
                provider: "test".into(),
                model: "test-model".into(),
                reasoning_effort: None,
            }],
            ..Default::default()
        })
        .unwrap();

    let supervisor = SubagentSupervisor::new(
        Arc::new(registry),
        roci_config,
        make_base_config(),
        SubagentSupervisorConfig::default(),
        profile_registry,
    );
    (supervisor, requests)
}

fn make_tool_policy_supervisor(
    mut base_config: AgentConfig,
    mut profile: SubagentProfile,
    attempted_tools: &[&str],
) -> (SubagentSupervisor, Arc<Mutex<Vec<ProviderRequest>>>) {
    base_config.approval_policy = crate::agent_loop::ApprovalPolicy::always();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut factory = RecordingProviderFactory::new(requests.clone(), "child finished");
    factory.tool_calls = attempted_tools
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(factory));
    let roci_config = RociConfig::default();
    roci_config.set_api_key("test", "test-key".into());
    profile.name = "test:policy".into();
    profile.models = vec![ModelCandidate {
        provider: "test".into(),
        model: "test-model".into(),
        reasoning_effort: None,
    }];
    let mut profiles = SubagentProfileRegistry::new();
    profiles.register(profile).unwrap();
    (
        SubagentSupervisor::new(
            Arc::new(registry),
            roci_config,
            base_config,
            SubagentSupervisorConfig::default(),
            profiles,
        ),
        requests,
    )
}

fn counting_tool(name: &str, calls: &Arc<Mutex<Vec<String>>>) -> Arc<dyn Tool> {
    let calls = calls.clone();
    let tool_name = name.to_string();
    Arc::new(
        AgentTool::new(
            name,
            "records execution",
            AgentToolParameters::empty(),
            move |_, _| {
                let calls = calls.clone();
                let name = tool_name.clone();
                async move {
                    calls.lock().unwrap().push(name.clone());
                    Ok(serde_json::json!({ "executed": name }))
                }
            },
        )
        .with_static_safety(
            ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
            ToolSafetySummary {
                read_only_by_default: true,
                destructive_by_default: false,
                concurrency_safe_by_default: true,
                approval_kind: ToolSafetyKind::Read,
            },
        ),
    )
}

async fn run_tool_policy_child(
    supervisor: &SubagentSupervisor,
) -> crate::agent::subagents::types::SubagentRunResult {
    let handle = supervisor
        .spawn(SubagentSpec {
            profile: "test:policy".into(),
            label: None,
            input: SubagentInput::Prompt {
                task: "exercise the tools".into(),
            },
            overrides: Default::default(),
        })
        .await
        .expect("child should launch");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
        .await
        .expect("scripted child should finish");
    assert_eq!(
        result.status,
        SubagentStatus::Completed,
        "{:?}",
        result.messages
    );
    result
}

fn advertised_tool_names(requests: &Arc<Mutex<Vec<ProviderRequest>>>) -> Vec<String> {
    requests.lock().unwrap()[0]
        .tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

fn assert_tool_rejected(messages: &[ModelMessage], name: &str) {
    let result = messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|part| match part {
            crate::types::ContentPart::ToolResult(result)
                if result.tool_call_id == format!("call-{name}") =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("forced tool call should receive a result");
    assert!(result.is_error, "hidden tool must return an error");
    assert!(result.result["error"]
        .as_str()
        .unwrap_or_default()
        .contains("not found"));
}

#[tokio::test]
async fn child_profile_exclusions_hide_schema_and_reject_forced_dispatch() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut config = make_base_config();
    config.tools = vec![
        counting_tool("allowed", &calls),
        counting_tool("excluded", &calls),
    ];
    let (supervisor, requests) = make_tool_policy_supervisor(
        config,
        SubagentProfile {
            excluded_tools: vec!["excluded".into()],
            ..Default::default()
        },
        &["excluded", "allowed"],
    );

    let result = run_tool_policy_child(&supervisor).await;

    assert_eq!(advertised_tool_names(&requests), ["allowed"]);
    assert_eq!(*calls.lock().unwrap(), ["allowed"]);
    assert_tool_rejected(&result.messages, "excluded");
}

#[tokio::test]
async fn child_profile_main_only_exclusion_keeps_child_tool_callable() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut config = make_base_config();
    config.tools = vec![counting_tool("child_allowed", &calls)];
    let (supervisor, requests) = make_tool_policy_supervisor(
        config,
        SubagentProfile {
            default_agent_excluded_tools: vec!["child_allowed".into()],
            ..Default::default()
        },
        &["child_allowed"],
    );

    run_tool_policy_child(&supervisor).await;

    assert_eq!(advertised_tool_names(&requests), ["child_allowed"]);
    assert_eq!(*calls.lock().unwrap(), ["child_allowed"]);
}

#[tokio::test]
async fn child_profile_cannot_widen_host_tool_visibility() {
    for host_policy in [
        ToolVisibilityPolicy::exclude(["host_denied"]),
        ToolVisibilityPolicy::allow_only(["allowed"]),
        ToolVisibilityPolicy::no_tools(),
    ] {
        let no_tools = host_policy.is_no_tools();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut config = make_base_config();
        config.tools = vec![
            counting_tool("allowed", &calls),
            counting_tool("host_denied", &calls),
        ];
        config.tool_visibility_policy = host_policy;
        let (supervisor, requests) = make_tool_policy_supervisor(
            config,
            SubagentProfile::default(),
            &["host_denied", "allowed"],
        );

        let result = run_tool_policy_child(&supervisor).await;

        let expected = if no_tools { vec![] } else { vec!["allowed"] };
        assert_eq!(advertised_tool_names(&requests), expected);
        assert_eq!(*calls.lock().unwrap(), expected);
        assert_tool_rejected(&result.messages, "host_denied");
        if no_tools {
            assert_tool_rejected(&result.messages, "allowed");
        }
    }
}

#[tokio::test]
async fn child_profile_explicit_host_forbidden_tool_fails_before_provider_call() {
    for tools in [
        ToolPolicy::Replace {
            tools: vec!["host_denied".into()],
        },
        ToolPolicy::InheritWithOverrides {
            add: vec!["host_denied".into()],
            remove: Vec::new(),
        },
    ] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut config = make_base_config();
        config.tools = vec![counting_tool("host_denied", &calls)];
        config.tool_visibility_policy = ToolVisibilityPolicy::exclude(["host_denied"]);
        let (supervisor, requests) = make_tool_policy_supervisor(
            config,
            SubagentProfile {
                tools,
                ..Default::default()
            },
            &["host_denied"],
        );

        let error = supervisor
            .spawn(SubagentSpec {
                profile: "test:policy".into(),
                label: None,
                input: SubagentInput::Prompt {
                    task: "exercise the tools".into(),
                },
                overrides: Default::default(),
            })
            .await
            .err()
            .expect("host-forbidden explicit tool should reject child configuration");

        assert!(
            matches!(error, RociError::Configuration(message) if message.contains("host_denied"))
        );
        assert!(requests.lock().unwrap().is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn child_profile_empty_native_selection_rejects_forced_dispatch() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut config = make_base_config();
    config.tools = vec![counting_tool("native", &calls)];
    let (supervisor, requests) = make_tool_policy_supervisor(
        config,
        SubagentProfile {
            tools: ToolPolicy::Replace { tools: Vec::new() },
            ..Default::default()
        },
        &["native"],
    );

    let result = run_tool_policy_child(&supervisor).await;

    assert!(advertised_tool_names(&requests).is_empty());
    assert!(calls.lock().unwrap().is_empty());
    assert_tool_rejected(&result.messages, "native");
}

struct ScopedPolicyTools {
    calls: Arc<Mutex<Vec<String>>>,
    tools: Vec<(String, DynamicTool)>,
}

fn scoped_policy_tools(
    calls: &Arc<Mutex<Vec<String>>>,
    tools: &[(&str, &str, Option<&str>)],
) -> Arc<dyn DynamicToolProvider> {
    Arc::new(ScopedPolicyTools {
        calls: calls.clone(),
        tools: tools
            .iter()
            .map(|(server_id, name, alias)| {
                let mut tool = DynamicTool::new(*name, "server tool", AgentToolParameters::empty())
                    .with_safety(
                        ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
                        ToolSafetySummary {
                            read_only_by_default: true,
                            destructive_by_default: false,
                            concurrency_safe_by_default: true,
                            approval_kind: ToolSafetyKind::Read,
                        },
                    );
                tool.aliases = alias.iter().map(|alias| (*alias).to_string()).collect();
                ((*server_id).to_string(), tool)
            })
            .collect(),
    })
}

#[async_trait]
impl DynamicToolProvider for ScopedPolicyTools {
    fn server_ids(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|(server_id, _)| server_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        self.list_tools_for_servers(&self.server_ids()).await
    }

    async fn list_tools_for_servers(
        &self,
        server_ids: &[String],
    ) -> Result<Vec<DynamicTool>, RociError> {
        Ok(self
            .tools
            .iter()
            .filter(|(server_id, _)| server_ids.contains(server_id))
            .map(|(_, tool)| tool.clone())
            .collect())
    }

    async fn execute_tool(
        &self,
        name: &str,
        _args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        self.calls.lock().unwrap().push(name.to_string());
        Ok(serde_json::json!({ "executed": name }))
    }

    async fn execute_tool_for_servers(
        &self,
        server_ids: &[String],
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        if !self.tools.iter().any(|(server_id, tool)| {
            server_ids.contains(server_id)
                && (tool.name == name || tool.aliases.iter().any(|alias| alias == name))
        }) {
            return Err(RociError::InvalidState(
                "tool is outside server selection".into(),
            ));
        }
        self.execute_tool(name, args, ctx).await
    }
}

#[tokio::test]
async fn child_profile_scopes_mcp_without_reenabling_empty_native_selection() {
    for mcp_servers in [vec!["alpha".to_string()], Vec::new()] {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut config = make_base_config();
        config.tools = vec![counting_tool("native", &calls)];
        config.dynamic_tool_providers = vec![scoped_policy_tools(
            &calls,
            &[("alpha", "alpha_tool", None), ("beta", "beta_tool", None)],
        )];
        let has_mcp = !mcp_servers.is_empty();
        let (supervisor, requests) = make_tool_policy_supervisor(
            config,
            SubagentProfile {
                tools: ToolPolicy::Replace { tools: Vec::new() },
                mcp_servers,
                ..Default::default()
            },
            &["native", "alpha_tool", "beta_tool"],
        );

        let result = run_tool_policy_child(&supervisor).await;

        let expected = if has_mcp { vec!["alpha_tool"] } else { vec![] };
        assert_eq!(advertised_tool_names(&requests), expected);
        assert_eq!(*calls.lock().unwrap(), expected);
        assert_tool_rejected(&result.messages, "native");
        assert_tool_rejected(&result.messages, "beta_tool");
        if !has_mcp {
            assert_tool_rejected(&result.messages, "alpha_tool");
        }
    }
}

#[tokio::test]
async fn child_profile_excluded_native_name_cannot_be_reclaimed_by_mcp() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut config = make_base_config();
    config.tools = vec![counting_tool("shell", &calls)];
    config.dynamic_tool_providers = vec![scoped_policy_tools(
        &calls,
        &[("alpha", "shell", None), ("alpha", "allowed_remote", None)],
    )];
    let (supervisor, requests) = make_tool_policy_supervisor(
        config,
        SubagentProfile {
            excluded_tools: vec!["shell".into()],
            mcp_servers: vec!["alpha".into()],
            ..Default::default()
        },
        &["shell", "allowed_remote"],
    );

    let result = run_tool_policy_child(&supervisor).await;

    assert_eq!(advertised_tool_names(&requests), ["allowed_remote"]);
    assert_eq!(*calls.lock().unwrap(), ["allowed_remote"]);
    assert_tool_rejected(&result.messages, "shell");
}

#[tokio::test]
async fn child_profile_mcp_alias_collision_with_excluded_native_fails_before_provider() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut config = make_base_config();
    config.tools = vec![counting_tool("shell", &calls)];
    config.dynamic_tool_providers = vec![scoped_policy_tools(
        &calls,
        &[
            ("alpha", "remote_shell", Some("shell")),
            ("alpha", "allowed_remote", None),
        ],
    )];
    let (supervisor, requests) = make_tool_policy_supervisor(
        config,
        SubagentProfile {
            excluded_tools: vec!["shell".into()],
            mcp_servers: vec!["alpha".into()],
            ..Default::default()
        },
        &["shell", "remote_shell", "allowed_remote"],
    );

    let handle = supervisor
        .spawn(SubagentSpec {
            profile: "test:policy".into(),
            label: None,
            input: SubagentInput::Prompt {
                task: "exercise the tools".into(),
            },
            overrides: Default::default(),
        })
        .await
        .expect("child runtime should launch before discovering MCP tools");
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait())
        .await
        .expect("child should reject alias collision without hanging");

    assert_eq!(result.status, SubagentStatus::Failed);
    let error = result
        .error
        .as_deref()
        .expect("alias collision should explain child failure");
    assert!(
        error.contains("shell") && error.contains("collides"),
        "{error}"
    );
    assert!(requests.lock().unwrap().is_empty());
    assert!(calls.lock().unwrap().is_empty());
}

struct BlockingAskUserFactory;

impl ProviderFactory for BlockingAskUserFactory {
    fn provider_keys(&self) -> &[&str] {
        &["test"]
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(BlockingAskUserProvider {
            model_id: model_id.to_string(),
            capabilities: ModelCapabilities {
                supports_streaming: false,
                input: ModelInputCapabilities::default(),
                ..ModelCapabilities::default()
            },
        }))
    }
}

struct BlockingAskUserProvider {
    model_id: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for BlockingAskUserProvider {
    fn provider_name(&self) -> &str {
        "test"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "blocking ask_user test provider uses stream_text".into(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        Ok(Box::pin(stream::iter(vec![
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::ToolCallDelta,
                tool_call: Some(crate::types::AgentToolCall {
                    id: "ask-user-call-1".into(),
                    name: "ask_user".into(),
                    arguments: serde_json::json!({
                        "questions": [
                            {
                                "id": "abort_unit",
                                "text": "Abort me?"
                            }
                        ]
                    }),
                    called_as: None,
                    recipient: None,
                }),
                finish_reason: None,
                usage: None,
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
            Ok(TextStreamDelta {
                text: String::new(),
                event_type: StreamEventType::Done,
                tool_call: None,
                finish_reason: None,
                usage: Some(Usage::default()),
                reasoning: None,
                reasoning_signature: None,
                reasoning_type: None,
            }),
        ])))
    }
}

fn make_blocking_ask_user_supervisor() -> SubagentSupervisor {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(BlockingAskUserFactory));
    let roci_config = RociConfig::default();
    roci_config.set_api_key("test", "test-key".into());

    let ask_user_tool: Arc<dyn Tool> = Arc::new(
        AgentTool::new(
            "ask_user",
            "ask user test tool",
            AgentToolParameters::empty(),
            |_args, ctx| async move {
                let callback = ctx.request_user_input.clone().ok_or_else(|| {
                    RociError::InvalidState("missing request_user_input".to_string())
                })?;
                let response = callback(UserInputRequest {
                    request_id: uuid::Uuid::new_v4(),
                    tool_call_id: "ask-user-call-1".to_string(),
                    prompt: AskUserPrompt::Question {
                        id: "abort_unit".to_string(),
                        question: "Abort me?".to_string(),
                        placeholder: None,
                        default: None,
                        multiline: false,
                    },
                    timeout_ms: None,
                })
                .await
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
                let UserInputResult::Question { answer } = response.result else {
                    return Err(RociError::InvalidState(
                        "expected question answer".to_string(),
                    ));
                };
                Ok(serde_json::json!({ "answer": answer }))
            },
        )
        .with_static_safety(ToolSafetyPlan::host_input(), host_input_safety_summary()),
    );

    let mut base_config = make_base_config();
    base_config.candidates = vec![LanguageModel::Known {
        provider_key: "test".into(),
        model_id: "test-model".into(),
    }];
    base_config.tools = vec![ask_user_tool];

    let mut profile_registry = SubagentProfileRegistry::with_builtins();
    profile_registry
        .register(SubagentProfile {
            name: "test:dev".into(),
            system_prompt: Some("You are a test sub-agent.".into()),
            models: vec![ModelCandidate {
                provider: "test".into(),
                model: "test-model".into(),
                reasoning_effort: None,
            }],
            ..Default::default()
        })
        .unwrap();

    SubagentSupervisor::new(
        Arc::new(registry),
        roci_config,
        base_config,
        SubagentSupervisorConfig::default(),
        profile_registry,
    )
}

fn captured_request_messages(requests: &Arc<Mutex<Vec<ProviderRequest>>>) -> Vec<ModelMessage> {
    let requests = requests
        .lock()
        .expect("request capture lock should not be poisoned");
    assert_eq!(requests.len(), 1, "expected exactly one provider request");
    requests[0].messages.clone()
}

fn assert_test_system_prompt(message: &ModelMessage) {
    let preamble = SubagentPromptPolicy::default_child_preamble();
    assert_eq!(message.role, Role::System);
    assert!(
        message.text().starts_with(preamble),
        "child system prompt should start with the default preamble"
    );
    assert!(
        message.text().contains("You are a test sub-agent."),
        "child system prompt should include the profile prompt"
    );
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

#[test]
fn supervisor_construction_with_default_config() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
}

#[test]
fn supervisor_construction_with_custom_config() {
    let config = SubagentSupervisorConfig {
        max_concurrent: 2,
        max_active_children: Some(10),
        default_input_timeout_ms: Some(60_000),
        abort_on_drop: false,
    };
    let _supervisor = make_supervisor_with_config(config);
}

// ---------------------------------------------------------------------------
// list_active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_active_empty_on_fresh_supervisor() {
    let supervisor = make_supervisor();
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

#[test]
fn subscribe_returns_a_receiver() {
    let supervisor = make_supervisor();
    let _rx = supervisor.subscribe();
    let _rx2 = supervisor.subscribe();
}

#[test]
fn subscribe_receiver_is_empty_initially() {
    let supervisor = make_supervisor();
    let mut rx = supervisor.subscribe();
    assert!(rx.try_recv().is_err());
}

// ---------------------------------------------------------------------------
// wait_any / wait_all with no children
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wait_any_returns_none_when_no_active_children() {
    let supervisor = make_supervisor();
    assert!(supervisor.wait_any().await.is_none());
}

#[tokio::test]
async fn wait_all_returns_empty_when_no_active_children() {
    let supervisor = make_supervisor();
    let results = supervisor.wait_all().await;
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// max_active_children cap (via spawn rejection)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_rejected_when_max_active_children_zero() {
    // max_active_children = 0 means no children can ever be spawned
    let config = SubagentSupervisorConfig {
        max_active_children: Some(0),
        ..Default::default()
    };
    let supervisor = make_supervisor_with_config(config);

    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("test".into()),
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("max active children"));
}

// ---------------------------------------------------------------------------
// spawn with unknown profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_with_unknown_profile_returns_error() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "nonexistent-profile".into(),
        label: None,
        input: SubagentInput::Prompt {
            task: "test".into(),
        },
        overrides: Default::default(),
    };
    let result = supervisor.spawn(spec).await;
    let err = result.err().expect("expected spawn to fail");
    assert!(err.to_string().contains("not found"));
}

// ---------------------------------------------------------------------------
// shutdown on empty supervisor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_on_empty_supervisor_is_fine() {
    let supervisor = make_supervisor();
    supervisor.shutdown().await;
    let active = supervisor.list_active().await;
    assert!(active.is_empty());
}

// ---------------------------------------------------------------------------
// abort / wait on unknown child
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abort_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.abort(Uuid::new_v4()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn wait_unknown_child_returns_error() {
    let supervisor = make_supervisor();
    let result = supervisor.wait(Uuid::new_v4()).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Runtime regression coverage for child-input seeding and transcript shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_with_context_prompt_only_runs_from_seeded_history() {
    let (supervisor, requests) = make_recording_supervisor("prompt-only complete");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("prompt-only".into()),
        input: SubagentInput::Prompt {
            task: "fix the bug".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 2);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].role, Role::User);
    assert_eq!(request_messages[1].text(), "fix the bug");

    assert_eq!(result.messages.len(), 3);
    assert_test_system_prompt(&result.messages[0]);
    assert_eq!(result.messages[1].text(), "fix the bug");
    assert_eq!(result.messages[2].role, Role::Assistant);
    assert_eq!(result.messages[2].text(), "prompt-only complete");
}

#[tokio::test]
async fn spawn_with_context_snapshot_only_uses_supplied_context_without_task() {
    let (supervisor, requests) = make_recording_supervisor("snapshot-only complete");
    let context = SubagentContext {
        summary: Some("parent did X".into()),
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("snapshot-only".into()),
        input: SubagentInput::Snapshot {
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 3);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].role, Role::User);
    assert_eq!(
        request_messages[1].text(),
        "Parent context summary:\nparent did X"
    );
    assert_eq!(request_messages[2].role, Role::User);
    assert!(request_messages[2].text().contains("read-only snapshot"));
}

#[tokio::test]
async fn spawn_with_context_prompt_plus_snapshot_preserves_order_end_to_end() {
    let (supervisor, requests) = make_recording_supervisor("prompt+snapshot complete");
    let context = SubagentContext {
        summary: Some("summary of conversation".into()),
        selected_messages: vec![
            ModelMessage::assistant("prior answer"),
            ModelMessage::user("follow-up from parent"),
        ],
        ..Default::default()
    };
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("prompt+snapshot".into()),
        input: SubagentInput::PromptWithSnapshot {
            task: "implement feature Y".into(),
            mode: SnapshotMode::SummaryOnly,
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn_with_context(spec, context).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(
        request_messages
            .iter()
            .map(|message| (message.role, message.text()))
            .collect::<Vec<_>>(),
        vec![
            (Role::System, request_messages[0].text(),),
            (
                Role::User,
                "Parent context summary:\nsummary of conversation".into()
            ),
            (Role::Assistant, "prior answer".into()),
            (Role::User, "follow-up from parent".into()),
            (Role::User, "implement feature Y".into()),
        ]
    );
    assert_test_system_prompt(&request_messages[0]);

    assert_eq!(
        result
            .messages
            .iter()
            .map(|message| (message.role, message.text()))
            .collect::<Vec<_>>(),
        vec![
            (Role::System, result.messages[0].text()),
            (
                Role::User,
                "Parent context summary:\nsummary of conversation".into()
            ),
            (Role::Assistant, "prior answer".into()),
            (Role::User, "follow-up from parent".into()),
            (Role::User, "implement feature Y".into()),
            (Role::Assistant, "prompt+snapshot complete".into()),
        ]
    );
    assert_test_system_prompt(&result.messages[0]);
}

#[tokio::test]
async fn spawn_default_context_path_still_runs() {
    let (supervisor, requests) = make_recording_supervisor("spawn complete");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("default-context".into()),
        input: SubagentInput::Prompt {
            task: "test default context".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn(spec).await.unwrap();
    let result = handle.wait().await;

    assert_eq!(result.status, SubagentStatus::Completed);
    let request_messages = captured_request_messages(&requests);
    assert_eq!(request_messages.len(), 2);
    assert_test_system_prompt(&request_messages[0]);
    assert_eq!(request_messages[1].text(), "test default context");
    assert_eq!(result.messages[2].text(), "spawn complete");
}

#[tokio::test]
async fn child_runtime_does_not_duplicate_system_prompt() {
    let (supervisor, requests) = make_recording_supervisor("no duplicate system");
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("system-once".into()),
        input: SubagentInput::Prompt {
            task: "hello".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor
        .spawn_with_context(spec, SubagentContext::default())
        .await
        .unwrap();
    let result = handle.wait().await;

    let request_messages = captured_request_messages(&requests);
    assert_eq!(
        request_messages
            .iter()
            .filter(|message| message.role == Role::System)
            .count(),
        1,
        "provider request should include exactly one system message"
    );
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .count(),
        1,
        "final child transcript should include exactly one system message"
    );
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn abort_while_child_waits_for_user_input_completes() {
    let supervisor = make_blocking_ask_user_supervisor();
    let mut events = supervisor.subscribe();
    let spec = SubagentSpec {
        profile: "test:dev".into(),
        label: Some("abort-ask-user".into()),
        input: SubagentInput::Prompt {
            task: "use ask_user and then wait".into(),
        },
        overrides: Default::default(),
    };

    let handle = supervisor.spawn(spec).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            match events.recv().await {
                Ok(SubagentEvent::AgentEvent { event, .. }) => {
                    if let AgentEvent::HumanInteractionRequested { request } = *event {
                        let request = request
                            .to_user_input()
                            .expect("human interaction should be ask_user");
                        assert_eq!(request.prompt.id(), "abort_unit");
                        break;
                    }
                }
                Ok(_) => {}
                Err(err) => panic!("event stream closed unexpectedly: {err}"),
            }
        }
    })
    .await
    .expect("child should request user input");

    assert!(supervisor.abort(handle.id()).await.unwrap());

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        handle.wait().await
    })
    .await
    .expect("aborted child should complete promptly");

    assert_eq!(result.status, SubagentStatus::Aborted);
}

// ---------------------------------------------------------------------------
// submit_user_input with unknown request
// ---------------------------------------------------------------------------

#[cfg(feature = "agent")]
#[tokio::test]
async fn submit_user_input_unknown_request_returns_error() {
    use crate::tools::UserInputResponse;

    let supervisor = make_supervisor();
    let response = UserInputResponse {
        request_id: Uuid::nil(),
        result: crate::tools::UserInputResult::Question {
            answer: "C".to_string(),
        },
    };
    let result = supervisor.submit_user_input(response).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Spawn tests that need a real provider (ignored)
// ---------------------------------------------------------------------------

/// Full spawn -> wait lifecycle test. Requires API keys and a configured
/// provider with valid credentials.
#[ignore = "requires API keys and a configured provider"]
#[tokio::test]
async fn spawn_and_wait_full_lifecycle() {
    let supervisor = make_supervisor();
    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("live-test".into()),
        input: SubagentInput::Prompt {
            task: "say hello".into(),
        },
        overrides: Default::default(),
    };
    let handle = supervisor.spawn(spec).await.unwrap();
    let result = handle.wait().await;
    assert!(matches!(
        result.status,
        SubagentStatus::Completed | SubagentStatus::Failed
    ));
}
