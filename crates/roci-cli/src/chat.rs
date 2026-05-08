use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use roci::agent::{AgentConfig, AgentRuntime, HumanInteractionCoordinator, QueueDrainMode};
use roci::agent_loop::{ApprovalPolicy, PreToolUseHookResult, RunStatus};
use roci::attachments::{
    preflight_resolved_attachments, render_prompt_input_text, Attachment, AttachmentResolveOptions,
    AttachmentResolver, DefaultAttachmentResolver, PromptInput, ResolvedAttachment,
};
use roci::config::RociConfig;
use roci::mcp::{merge_mcp_instructions, MCPInstructionMergePolicy};
use roci::models::ModelCapabilities;
use roci::resource::SkillResourceOptions;
use roci::session::{CreateSessionOptions, LocalSessionStore, SessionConfig, SessionId};
use roci::skills::merge_system_prompt_with_skills;
use roci::tools::ToolVisibilityPolicy;
use roci::types::{ContentPart, ImageContent, ModelMessage, Role};

use crate::cli::{ChatApprovalArg, ChatArgs};

mod mcp;
mod resource_prompt;
mod runtime_events;
mod user_input;

use mcp::build_mcp_runtime_wiring;
use resource_prompt::{
    build_resource_system_prompt, expand_chat_prompt, print_resource_diagnostics,
};
use runtime_events::RuntimeEventRenderer;

pub async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let ChatArgs {
        model: model_arg,
        system,
        temperature,
        skill_path,
        skill_root,
        no_skills,
        no_tools,
        tools: allowed_tools,
        exclude_tools,
        max_tokens,
        approval,
        session_root,
        session_id,
        attachments,
        mcp_stdio,
        mcp_sse,
        prompt,
    } = args;

    let prompt = match prompt {
        Some(p) => p,
        None => {
            eprintln!("Usage: roci-agent chat \"your prompt here\"");
            std::process::exit(1);
        }
    };

    let model: roci::models::LanguageModel = model_arg.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            model_arg
        )
    })?;

    let config = RociConfig::from_env();
    let registry = Arc::new(roci::default_registry());
    let cwd = std::env::current_dir()?;

    let skill_options = SkillResourceOptions {
        enabled: !no_skills,
        explicit_paths: skill_path,
        extra_roots: skill_root,
    };

    let resources = roci::resource::ResourceLoader::new()
        .with_skill_options(skill_options)
        .load(&cwd)?;
    print_resource_diagnostics(&resources);

    let prompt = expand_chat_prompt(&prompt, &resources);
    let attachment_message = if attachments.is_empty() {
        None
    } else {
        let provider =
            registry.create_provider(model.provider_name(), model.model_id(), &config)?;
        Some(build_attachment_prompt_message(
            prompt.clone(),
            &attachments,
            provider.capabilities(),
        )?)
    };
    let resource_system_prompt = build_resource_system_prompt(system, &resources);
    let skill_system_prompt =
        merge_system_prompt_with_skills(resource_system_prompt, &resources.skills.skills);
    let mcp_runtime = build_mcp_runtime_wiring(&mcp_stdio, &mcp_sse).await?;
    let system_prompt = merge_mcp_instructions(
        skill_system_prompt.as_deref(),
        &mcp_runtime.instructions,
        MCPInstructionMergePolicy::AppendBlock,
    );

    let mut settings = roci::types::GenerationSettings::default();
    if let Some(t) = temperature {
        settings.temperature = Some(t);
    }
    if let Some(max) = max_tokens {
        settings.max_tokens = Some(max);
    }

    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let mut renderer = RuntimeEventRenderer::spawn(coordinator.clone());
    let approval_policy = approval_policy_from_arg(approval);
    let approval_handler =
        (approval == ChatApprovalArg::Ask).then(|| renderer.build_approval_handler());
    let tool_visibility_policy = tool_visibility_policy_from_args(
        no_tools,
        allowed_tools.iter().map(String::as_str),
        exclude_tools.iter().map(String::as_str),
    );
    let session = session_root
        .map(|root| {
            let id = match session_id {
                Some(id) => SessionId::parse(id),
                None => Ok(SessionId::new_v4()),
            }?;
            Ok::<_, roci::session::SessionError>(SessionConfig::new(id, root))
        })
        .transpose()?;
    let tools = roci_tools::builtin::tool_catalog().resolve(&tool_visibility_policy);
    let agent_config = AgentConfig {
        model,
        system_prompt,
        tools,
        tool_visibility_policy,
        dynamic_tool_providers: mcp_runtime.dynamic_tool_providers,
        settings,
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: Some(renderer.build_agent_sink()),
        approval_policy,
        approval_handler,
        session_id: None,
        session,
        sandbox_provider: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: Default::default(),
        api_key_override: None,
        provider_headers: Default::default(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: Default::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: Some(Arc::new(|call, _cancel| {
            demo_pre_tool_use_hook(&call.name, &call.id);
            Box::pin(async { Ok(PreToolUseHookResult::Continue) })
        })),
        post_tool_use: Some(Arc::new(|call, result| {
            demo_post_tool_use_hook(&call.name, &call.id);
            Box::pin(async move { Ok(result) })
        })),
        user_input_timeout_ms: None,
        context_budget: None,
        chat: Default::default(),
        human_interaction_coordinator: Some(coordinator.clone()),
    };
    let agent = if let Some(session_config) = agent_config.session.clone() {
        let store = LocalSessionStore::new(session_config.root.clone());
        let state = if session_config.conventions().metadata_file().is_file() {
            store.open(session_config.id.clone()).await?
        } else {
            store
                .create(CreateSessionOptions {
                    id: Some(session_config.id.clone()),
                    title: None,
                    host_cwd: Some(cwd.clone()),
                    import_source: None,
                    default_thread_id: agent_config.chat.default_thread_id,
                })
                .await?
        };
        Arc::new(AgentRuntime::resume_session(registry, config, agent_config, state).await?)
    } else {
        Arc::new(AgentRuntime::try_new(registry, config, agent_config)?)
    };

    let subscription = agent.subscribe(None).await;
    renderer.subscribe(subscription, agent.clone());

    let result = if let Some(message) = attachment_message {
        agent.prompt_message(message).await
    } else {
        agent.prompt(prompt).await
    };
    renderer.finish().await;
    let result = result?;
    println!();

    if result.status == RunStatus::Failed {
        if let Some(err) = result.error {
            return Err(err.into());
        }
    }

    Ok(())
}

fn build_attachment_prompt_message(
    prompt: String,
    attachment_paths: &[PathBuf],
    capabilities: &ModelCapabilities,
) -> Result<ModelMessage, Box<dyn std::error::Error>> {
    let attachments = attachment_paths
        .iter()
        .cloned()
        .map(Attachment::file)
        .collect::<Vec<_>>();
    let input = PromptInput::new(prompt).with_attachments(attachments);
    let resolver = DefaultAttachmentResolver;
    let resolved = resolver.resolve_prompt_input(&input, &AttachmentResolveOptions::default())?;
    preflight_resolved_attachments(&resolved, capabilities)?;
    Ok(prompt_message_from_resolved(input, &resolved))
}

fn prompt_message_from_resolved(
    input: PromptInput,
    resolved: &[ResolvedAttachment],
) -> ModelMessage {
    let mut content = vec![ContentPart::Text {
        text: render_prompt_input_text(&input, resolved),
    }];

    for attachment in resolved {
        let ResolvedAttachment::Image { data, metadata } = attachment else {
            continue;
        };
        let mime_type = metadata
            .mime_type
            .as_deref()
            .map(normalize_mime_type)
            .unwrap_or_else(|| "application/octet-stream".to_string());
        content.push(ContentPart::Image(ImageContent {
            data: BASE64_STANDARD.encode(data),
            mime_type,
        }));
    }

    ModelMessage {
        role: Role::User,
        content,
        name: None,
        timestamp: Some(chrono::Utc::now()),
    }
}

fn normalize_mime_type(mime_type: &str) -> String {
    mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_ascii_lowercase()
}

fn demo_pre_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] preToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn demo_post_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] postToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn approval_policy_from_arg(arg: ChatApprovalArg) -> ApprovalPolicy {
    match arg {
        ChatApprovalArg::Ask => ApprovalPolicy::Ask,
        ChatApprovalArg::Always => ApprovalPolicy::Always,
        ChatApprovalArg::Never => ApprovalPolicy::Never,
    }
}

fn tool_visibility_policy_from_args<'a>(
    no_tools: bool,
    allowed_tools: impl IntoIterator<Item = &'a str>,
    excluded_tools: impl IntoIterator<Item = &'a str>,
) -> ToolVisibilityPolicy {
    let mut policy = ToolVisibilityPolicy::default();
    policy.set_no_tools(no_tools);
    policy.extend_allow(allowed_tools);
    policy.extend_exclude(excluded_tools);
    policy
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use roci::attachments::{
        preflight_resolved_attachments, render_prompt_input_text, Attachment, AttachmentMetadata,
        AttachmentPreflightError, AttachmentResolveOptions, AttachmentResolver, AttachmentSource,
        DefaultAttachmentResolver, PromptInput, ResolvedAttachment,
    };
    use roci::models::{ModelCapabilities, ModelInputCapabilities};
    use roci::types::ContentPart;

    use super::prompt_message_from_resolved;

    #[test]
    fn copilot_provider_available_in_default_registry() {
        let registry = roci::default_registry();
        assert!(
            registry.has_provider("github-copilot"),
            "expected github-copilot provider to be registered in default roci-cli builds"
        );
    }

    #[test]
    fn resolve_file_attachments_and_preflight_by_model_caps() {
        let workspace = tempfile::tempdir().expect("temporary workspace");

        let text_path = workspace.path().join("notes.txt");
        let image_path = workspace.path().join("diagram.png");
        fs::write(&text_path, "first line\nsecond line").expect("text fixture");
        fs::write(&image_path, [137, 80, 78, 71, 13, 10, 26, 10]).expect("image fixture");

        let attachments = vec![Attachment::file(text_path), Attachment::file(image_path)];
        let resolved = DefaultAttachmentResolver
            .resolve_attachments(&attachments, &AttachmentResolveOptions::default())
            .expect("attachments should resolve");

        assert_eq!(resolved.len(), 2);

        assert!(matches!(
            preflight_resolved_attachments(&resolved, &ModelCapabilities::default()),
            Err(AttachmentPreflightError::ImageUnsupported)
        ));

        let image_caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities::from_vision_support(true),
            ..ModelCapabilities::default()
        };
        let report = preflight_resolved_attachments(&resolved, &image_caps)
            .expect("vision model should accept image attachment");

        assert_eq!(report.total_attachments, 2);
        assert_eq!(report.text_attachments, 1);
        assert_eq!(report.image_attachments, 1);
    }

    #[test]
    fn render_prompt_text_adds_only_text_attachments() {
        let mut input = PromptInput::new("Describe this");

        let text_meta = AttachmentMetadata {
            source: AttachmentSource::File {
                path: PathBuf::from("notes.txt"),
            },
            name: Some("notes.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 12,
        };
        let image_meta = AttachmentMetadata {
            source: AttachmentSource::Blob,
            name: Some("diagram.png".to_string()),
            mime_type: Some("image/png".to_string()),
            size_bytes: 4,
        };

        let resolved = vec![
            ResolvedAttachment::Text {
                text: "line one\nline two".to_string(),
                metadata: text_meta,
            },
            ResolvedAttachment::Image {
                data: vec![1, 2, 3, 4],
                metadata: image_meta,
            },
        ];
        input.text = input.text.trim_end().to_string();

        let rendered = render_prompt_input_text(&input, &resolved);

        assert_eq!(
            rendered,
            [
                "Describe this",
                "",
                "--- Attachment: notes.txt (text/plain) ---",
                "line one",
                "line two",
                "--- End attachment ---",
            ]
            .join("\n")
        );
    }

    #[test]
    fn build_image_content_part_uses_base64_when_images_supported() {
        let input = PromptInput::new("Analyze image");
        let attachment = ResolvedAttachment::Image {
            data: vec![1, 2, 3, 4],
            metadata: AttachmentMetadata {
                source: AttachmentSource::Blob,
                name: Some("frame.bin".to_string()),
                mime_type: Some("image/png".to_string()),
                size_bytes: 4,
            },
        };

        let image_caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities::from_vision_support(true),
            ..ModelCapabilities::default()
        };
        let report = preflight_resolved_attachments(std::slice::from_ref(&attachment), &image_caps)
            .expect("vision model should accept image attachment");
        assert_eq!(report.image_attachments, 1);

        let message = prompt_message_from_resolved(input, std::slice::from_ref(&attachment));
        let model_part = match &message.content[1] {
            ContentPart::Image(content) => content.clone(),
            _ => panic!("expected image content part"),
        };

        assert_eq!(model_part.data, "AQIDBA==");
        assert_eq!(model_part.mime_type, "image/png");
    }
}
