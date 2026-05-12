use std::collections::BTreeSet;
use std::error::Error;
use std::sync::{Arc, Mutex};

use roci::agent::{AgentConfig, AgentRuntime};
use roci::agent_loop::{ApprovalPolicy, RetryMode, RunStatus};
use roci::attachments::PromptInput;
use roci::config::RociConfig;
use roci::models::LanguageModel;
use roci::tools::{
    AgentTool, AgentToolParameters, ToolPromptMetadata, ToolResultSizePolicy, ToolSafetyKind,
    ToolSafetyPlan, ToolSafetySummary,
};
use roci::types::{ContentPart, GenerationSettings, ModelMessage, Role};
use serde::Serialize;
use serde_json::Value;

use crate::cli::{ToolContractsSmokeArgs, ToolContractsSmokeCaseArg};

const RESULT_TOOL: &str = "oversized_result";
const PROMPT_TOOL: &str = "prompt_metadata_probe";
const ALIAS_TOOL: &str = "canonical_echo";
const LEGACY_ALIAS: &str = "legacy_echo";
const PROMPT_DESCRIPTION: &str =
    "Provider prompt description marker: prompt_metadata_probe_custom_prompt_8421";
const PROMPT_GUIDELINE: &str =
    "Guideline marker: prompt_metadata_probe_guideline_8421 must be visible in available_tools";
const RESULT_CAP_BYTES: usize = 256;

pub async fn handle_tool_contracts_smoke(
    args: ToolContractsSmokeArgs,
) -> Result<(), Box<dyn Error>> {
    let model: LanguageModel = args.model.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            args.model
        )
    })?;
    let provider = model.provider_name().to_string();
    let endpoint_display = args.endpoint.as_deref().unwrap_or("<config/env>");

    let config = RociConfig::from_env();
    if let Some(endpoint) = &args.endpoint {
        config.set_base_url(&provider, endpoint.clone());
    }
    if let Some(api_key) = &args.api_key {
        config.set_api_key(&provider, api_key.clone());
    }

    println!("provider: {provider}");
    println!("model: {}", model.model_id());
    println!("endpoint: {endpoint_display}");
    println!("api_key: {}", credential_status(args.api_key.as_deref()));
    println!("command: tool-contracts-smoke");
    println!("case: {:?}", args.case);

    let payloads = Arc::new(Mutex::new(Vec::<Value>::new()));
    let payloads_for_callback = payloads.clone();
    let callback = Arc::new(move |payload: Value| {
        if let Ok(mut payloads) = payloads_for_callback.lock() {
            payloads.push(payload);
        }
    });

    let settings = GenerationSettings {
        temperature: Some(0.0),
        max_tokens: Some(1024),
        ..GenerationSettings::default()
    };

    let tools = smoke_tools();
    let agent_config = AgentConfig {
        candidates: vec![model.clone()],
        system_prompt: Some(system_prompt()),
        tools,
        settings,
        approval_policy: ApprovalPolicy::always(),
        provider_payload_callback: Some(callback),
        retry_mode: Some(RetryMode::Bounded { max_attempts: 1 }),
        ..AgentConfig::default()
    };

    let agent = AgentRuntime::try_new(Arc::new(roci::default_registry()), config, agent_config)?;
    let result = agent
        .prompt(PromptInput::new(prompt_for_case(args.case)))
        .await?;

    let response_text = assistant_response_text(&result.messages);
    println!("response_text: {response_text}");

    let payloads = payloads
        .lock()
        .map_err(|_| "provider payload callback lock poisoned")?
        .clone();
    let evidence = SmokeEvidence::from_run(&provider, args.case, &payloads, &result.messages);
    println!(
        "json_evidence: {}",
        serde_json::to_string_pretty(&evidence)?
    );

    if result.status == RunStatus::Failed {
        return Err(result
            .error
            .unwrap_or_else(|| "tool-contracts-smoke run failed".to_string())
            .into());
    }

    evidence.assert_required()?;
    Ok(())
}

fn credential_status(cli_api_key: Option<&str>) -> &'static str {
    if cli_api_key.is_some() {
        "cli-override-present"
    } else {
        "config-or-env"
    }
}

fn system_prompt() -> String {
    format!(
        "You are running a smoke test. Call requested tools exactly. \
         If asked for {LEGACY_ALIAS}, call it even if the schema lists {ALIAS_TOOL}. \
         After tool results, reply with a concise confirmation."
    )
}

fn prompt_for_case(case: ToolContractsSmokeCaseArg) -> String {
    match case {
        ToolContractsSmokeCaseArg::All => format!(
            "Call these tools before answering: {RESULT_TOOL}, {PROMPT_TOOL}, and {LEGACY_ALIAS} \
             with message 'alias smoke'. Then reply with 'tool contracts smoke observed'."
        ),
        ToolContractsSmokeCaseArg::Result => {
            format!("Call {RESULT_TOOL}. Then reply with 'result smoke observed'.")
        }
        ToolContractsSmokeCaseArg::Prompt => {
            format!("Call {PROMPT_TOOL}. Then reply with 'prompt smoke observed'.")
        }
        ToolContractsSmokeCaseArg::Alias => format!(
            "Call {LEGACY_ALIAS} with message 'alias smoke'. Then reply with 'alias smoke observed'."
        ),
    }
}

fn smoke_tools() -> Vec<Arc<dyn roci::tools::Tool>> {
    vec![
        Arc::new(
            AgentTool::new(
                RESULT_TOOL,
                "Return an intentionally oversized JSON payload.",
                AgentToolParameters::empty(),
                |_args, _ctx| async {
                    Ok(serde_json::json!({
                        "kind": "oversized",
                        "payload": "x".repeat(2048),
                    }))
                },
            )
            .with_result_policy(ToolResultSizePolicy {
                max_result_size_bytes: Some(RESULT_CAP_BYTES),
            })
            .with_static_safety(read_only_plan(), read_only_summary()),
        ),
        Arc::new(
            AgentTool::new(
                PROMPT_TOOL,
                "Short UI description, not provider prompt.",
                AgentToolParameters::empty(),
                |_args, _ctx| async {
                    Ok(serde_json::json!({
                        "prompt_metadata_probe": "ok",
                    }))
                },
            )
            .with_prompt(PROMPT_DESCRIPTION)
            .with_prompt_metadata(ToolPromptMetadata {
                guidelines: vec![PROMPT_GUIDELINE.to_string()],
                search_hint: Some("search_hint_should_not_render".to_string()),
            })
            .with_static_safety(read_only_plan(), read_only_summary()),
        ),
        Arc::new(
            AgentTool::new(
                ALIAS_TOOL,
                "Echo a short smoke-test message.",
                AgentToolParameters::object()
                    .string("message", "Message to echo.", true)
                    .build(),
                |args, ctx| async move {
                    let message = args.get_str("message")?.to_string();
                    Ok(serde_json::json!({
                        "echo": message,
                        "tool_name": ctx.tool_name,
                    }))
                },
            )
            .with_aliases([LEGACY_ALIAS])
            .with_static_safety(read_only_plan(), read_only_summary()),
        ),
    ]
}

fn read_only_plan() -> ToolSafetyPlan {
    ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read)
}

fn read_only_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: true,
        destructive_by_default: false,
        concurrency_safe_by_default: true,
        approval_kind: ToolSafetyKind::Read,
    }
}

fn assistant_response_text(messages: &[ModelMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant && !message.text().is_empty())
        .map(ModelMessage::text)
        .unwrap_or_default()
}

#[derive(Debug, Serialize)]
struct SmokeEvidence {
    provider: String,
    case: String,
    provider_payload_seen: bool,
    provider_payload_count: usize,
    bounded_result_envelope: bool,
    bounded_result: Option<Value>,
    canonical_schema_names_only: Option<bool>,
    schema_tool_names: Vec<String>,
    prompt_schema_description: Option<bool>,
    available_tools_metadata: Option<bool>,
    available_tools_guideline: Option<bool>,
    available_tools_search_hint_absent: Option<bool>,
    alias_tool_executed: bool,
    alias_normalized_with_called_as: bool,
    tool_calls: Vec<ToolCallEvidence>,
    tool_results_seen: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ToolCallEvidence {
    id: String,
    name: String,
    called_as: Option<String>,
}

impl SmokeEvidence {
    fn from_run(
        provider: &str,
        case: ToolContractsSmokeCaseArg,
        payloads: &[Value],
        messages: &[ModelMessage],
    ) -> Self {
        let tool_calls = extract_tool_calls(messages);
        let tool_results = extract_tool_results(messages);
        let bounded_result = tool_results.iter().find_map(|(id, result)| {
            result
                .get("truncated")
                .and_then(Value::as_bool)
                .filter(|truncated| *truncated)
                .map(|_| serde_json::json!({ "tool_call_id": id, "result": result }))
        });
        let tool_results_seen = tool_results
            .iter()
            .map(|(id, _result)| id.clone())
            .collect::<Vec<_>>();

        let schema_tool_names = schema_tool_names(payloads);
        let canonical_schema_names_only = (!payloads.is_empty()).then(|| {
            !schema_tool_names.is_empty()
                && schema_tool_names.iter().all(|name| {
                    [RESULT_TOOL, PROMPT_TOOL, ALIAS_TOOL].contains(&name.as_str())
                        && name != LEGACY_ALIAS
                })
        });
        let prompt_schema_description = (!payloads.is_empty()).then(|| {
            schema_descriptions(payloads)
                .iter()
                .any(|desc| desc == PROMPT_DESCRIPTION)
        });
        let payload_text = payloads
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let available_tools_metadata =
            (!payloads.is_empty()).then(|| payload_text.contains("<available_tools>"));
        let available_tools_guideline =
            (!payloads.is_empty()).then(|| payload_text.contains(PROMPT_GUIDELINE));
        let available_tools_search_hint_absent =
            (!payloads.is_empty()).then(|| !payload_text.contains("search_hint_should_not_render"));

        Self {
            provider: provider.to_string(),
            case: format!("{case:?}"),
            provider_payload_seen: !payloads.is_empty(),
            provider_payload_count: payloads.len(),
            bounded_result_envelope: bounded_result.is_some(),
            bounded_result,
            canonical_schema_names_only,
            schema_tool_names,
            prompt_schema_description,
            available_tools_metadata,
            available_tools_guideline,
            available_tools_search_hint_absent,
            alias_tool_executed: tool_calls.iter().any(|call| call.name == ALIAS_TOOL),
            alias_normalized_with_called_as: tool_calls.iter().any(|call| {
                call.name == ALIAS_TOOL && call.called_as.as_deref() == Some(LEGACY_ALIAS)
            }),
            tool_calls,
            tool_results_seen,
        }
    }

    fn assert_required(&self) -> Result<(), Box<dyn Error>> {
        if self.case_includes_result() && !self.bounded_result_envelope {
            return Err("missing bounded result envelope evidence".into());
        }
        if self.case_includes_prompt() {
            if !self.provider_payload_seen {
                return Err("missing provider payload evidence".into());
            }
            if self.canonical_schema_names_only != Some(true) {
                return Err("provider payload did not expose canonical schema names only".into());
            }
            if self.prompt_schema_description != Some(true) {
                return Err("provider payload did not use prompt() schema description".into());
            }
            if self.available_tools_metadata != Some(true)
                || self.available_tools_guideline != Some(true)
                || self.available_tools_search_hint_absent != Some(true)
            {
                return Err("provider payload missing expected available_tools metadata".into());
            }
        }
        if self.case_includes_alias() && !self.alias_tool_executed {
            return Err("missing alias/canonical echo tool execution evidence".into());
        }
        if self.case_includes_alias() && !self.alias_normalized_with_called_as {
            return Err("missing alias called_as normalization evidence".into());
        }
        Ok(())
    }

    fn case_includes_result(&self) -> bool {
        matches!(self.case.as_str(), "All" | "Result")
    }

    fn case_includes_prompt(&self) -> bool {
        matches!(self.case.as_str(), "All" | "Prompt")
    }

    fn case_includes_alias(&self) -> bool {
        matches!(self.case.as_str(), "All" | "Alias")
    }
}

fn extract_tool_calls(messages: &[ModelMessage]) -> Vec<ToolCallEvidence> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::ToolCall(call) => Some(ToolCallEvidence {
                id: call.id.clone(),
                name: call.name.clone(),
                called_as: call.called_as.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn extract_tool_results(messages: &[ModelMessage]) -> Vec<(String, Value)> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::ToolResult(result) => {
                Some((result.tool_call_id.clone(), result.result.clone()))
            }
            _ => None,
        })
        .collect()
}

fn schema_tool_names(payloads: &[Value]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for payload in payloads {
        for tool in payload
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = tool_name_from_payload_tool(tool) {
                names.insert(name.to_string());
            }
        }
    }
    names.into_iter().collect()
}

fn schema_descriptions(payloads: &[Value]) -> Vec<String> {
    let mut descriptions = Vec::new();
    for payload in payloads {
        for tool in payload
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(description) = tool_description_from_payload_tool(tool) {
                descriptions.push(description.to_string());
            }
        }
    }
    descriptions
}

fn tool_name_from_payload_tool(tool: &Value) -> Option<&str> {
    tool.get("name")
        .and_then(Value::as_str)
        .or_else(|| tool.pointer("/function/name").and_then(Value::as_str))
}

fn tool_description_from_payload_tool(tool: &Value) -> Option<&str> {
    tool.get("description").and_then(Value::as_str).or_else(|| {
        tool.pointer("/function/description")
            .and_then(Value::as_str)
    })
}

#[cfg(test)]
mod tests {
    use roci::types::{AgentToolCall, AgentToolResult, ContentPart, ModelMessage};

    use super::*;

    #[test]
    fn tool_contracts_smoke_tools_encode_contract_metadata() {
        let tools = smoke_tools();

        let result = tools
            .iter()
            .find(|tool| tool.name() == RESULT_TOOL)
            .unwrap();
        assert_eq!(
            result.result_policy().max_result_size_bytes,
            Some(RESULT_CAP_BYTES)
        );

        let prompt = tools
            .iter()
            .find(|tool| tool.name() == PROMPT_TOOL)
            .unwrap();
        assert_eq!(prompt.prompt(), PROMPT_DESCRIPTION);
        assert!(prompt
            .prompt_metadata()
            .guidelines
            .contains(&PROMPT_GUIDELINE.to_string()));

        let alias = tools.iter().find(|tool| tool.name() == ALIAS_TOOL).unwrap();
        assert_eq!(alias.aliases(), &[LEGACY_ALIAS.to_string()]);
    }

    #[test]
    fn tool_contracts_smoke_evidence_reads_payload_and_messages() {
        let payload = serde_json::json!({
            "messages": [{"role": "system", "content": format!("<available_tools>{PROMPT_GUIDELINE}</available_tools>")}],
            "tools": [
                {"type": "function", "function": {"name": RESULT_TOOL, "description": "result"}},
                {"type": "function", "function": {"name": PROMPT_TOOL, "description": PROMPT_DESCRIPTION}},
                {"type": "function", "function": {"name": ALIAS_TOOL, "description": "alias"}}
            ]
        });
        let messages = vec![
            ModelMessage {
                role: roci::types::Role::Assistant,
                content: vec![ContentPart::ToolCall(AgentToolCall {
                    id: "call-1".to_string(),
                    name: ALIAS_TOOL.to_string(),
                    arguments: serde_json::json!({"message": "alias smoke"}),
                    called_as: Some(LEGACY_ALIAS.to_string()),
                    recipient: None,
                })],
                name: None,
                timestamp: None,
                metadata: None,
            },
            ModelMessage {
                role: roci::types::Role::Tool,
                content: vec![ContentPart::ToolResult(AgentToolResult {
                    tool_call_id: "call-2".to_string(),
                    result: serde_json::json!({
                        "truncated": true,
                        "reason": "tool_result_size_limit_exceeded",
                    }),
                    is_error: false,
                })],
                name: None,
                timestamp: None,
                metadata: None,
            },
        ];

        let evidence = SmokeEvidence::from_run(
            "openai-compatible",
            ToolContractsSmokeCaseArg::All,
            &[payload],
            &messages,
        );

        assert!(evidence.bounded_result_envelope);
        assert_eq!(evidence.canonical_schema_names_only, Some(true));
        assert_eq!(evidence.prompt_schema_description, Some(true));
        assert_eq!(evidence.available_tools_metadata, Some(true));
        assert_eq!(evidence.available_tools_guideline, Some(true));
        assert_eq!(evidence.available_tools_search_hint_absent, Some(true));
        assert!(evidence.alias_tool_executed);
        assert!(evidence.alias_normalized_with_called_as);
        assert!(evidence.assert_required().is_ok());
    }

    #[test]
    fn tool_contracts_smoke_requires_called_as_for_alias_case() {
        let payload = serde_json::json!({
            "messages": [{"role": "system", "content": format!("<available_tools>{PROMPT_GUIDELINE}</available_tools>")}],
            "tools": [
                {"type": "function", "function": {"name": RESULT_TOOL, "description": "result"}},
                {"type": "function", "function": {"name": PROMPT_TOOL, "description": PROMPT_DESCRIPTION}},
                {"type": "function", "function": {"name": ALIAS_TOOL, "description": "alias"}}
            ]
        });
        let messages = vec![ModelMessage {
            role: roci::types::Role::Assistant,
            content: vec![ContentPart::ToolCall(AgentToolCall {
                id: "call-1".to_string(),
                name: ALIAS_TOOL.to_string(),
                arguments: serde_json::json!({"message": "alias smoke"}),
                called_as: None,
                recipient: None,
            })],
            name: None,
            timestamp: None,
            metadata: None,
        }];

        let evidence = SmokeEvidence::from_run(
            "openai",
            ToolContractsSmokeCaseArg::Alias,
            &[payload],
            &messages,
        );

        assert!(evidence.alias_tool_executed);
        assert!(!evidence.alias_normalized_with_called_as);
        assert!(evidence.assert_required().is_err());
    }
}
