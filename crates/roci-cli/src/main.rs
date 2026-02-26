//! Roci CLI binary entry point.

mod cli;
mod errors;

use std::sync::Arc;

use clap::Parser;
use roci::agent_loop::{
    AgentEvent, ApprovalPolicy, LoopRunner, PreToolUseHookResult, RunEventPayload, RunHooks,
    RunLifecycle, RunRequest, RunStatus, Runner,
};
use roci::config::RociConfig;
use roci::resource::{ContextFileResource, ResourceBundle, SkillResourceOptions};
use roci::skills::{
    merge_system_prompt_with_skills, ManagedSkillScope, ManagedSkillSourceKind, SkillManager,
    SkillSource,
};
use roci::types::{ModelMessage, StreamEventType};

use cli::{AuthCommands, ChatArgs, Cli, Commands, SkillsArgs, SkillsCommands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Auth(auth_args) => match auth_args.command {
            AuthCommands::Login(args) => cli::auth::handle_login(&args.provider).await,
            AuthCommands::Status => cli::auth::handle_status().await,
            AuthCommands::Logout(args) => cli::auth::handle_logout(&args.provider).await,
        },
        Commands::Chat(chat_args) => handle_chat(chat_args).await,
        Commands::Skills(skills_args) => handle_skills(skills_args).await,
    };

    if let Err(e) = result {
        // Try to downcast to RociError for actionable help text
        let message = if let Some(roci_err) = e.downcast_ref::<roci::error::RociError>() {
            errors::format_error_help(roci_err)
        } else {
            format!("{e}")
        };
        eprintln!("Error: {message}");
        std::process::exit(1);
    }
}

async fn handle_chat(args: ChatArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prompt = match args.prompt {
        Some(p) => p,
        None => {
            eprintln!("Usage: roci-agent chat \"your prompt here\"");
            std::process::exit(1);
        }
    };

    let model: roci::models::LanguageModel = args.model.parse().map_err(|_| {
        format!(
            "Invalid model format: '{}'. Use provider:model (e.g. openai:gpt-4o)",
            args.model
        )
    })?;

    let config = RociConfig::from_env();
    let registry = Arc::new(roci::default_registry());
    let runner = LoopRunner::with_registry(config, registry);
    let cwd = std::env::current_dir()?;
    let mut skill_options = SkillResourceOptions::default();
    skill_options.enabled = !args.no_skills;
    skill_options.explicit_paths = args.skill_path.clone();
    skill_options.extra_roots = args.skill_root.clone();

    let resources = roci::resource::DefaultResourceLoader::new()
        .with_skill_options(skill_options)
        .load(&cwd)?;
    print_resource_diagnostics(&resources);
    let prompt = expand_chat_prompt(&prompt, &resources);

    let resource_system_prompt = build_resource_system_prompt(args.system, &resources);
    let system_prompt =
        merge_system_prompt_with_skills(resource_system_prompt, &resources.skills.skills);

    let mut messages = Vec::new();
    if let Some(system) = system_prompt {
        messages.push(ModelMessage::system(system));
    }
    messages.push(ModelMessage::user(prompt));

    let mut settings = roci::types::GenerationSettings::default();
    if let Some(t) = args.temperature {
        settings.temperature = Some(t);
    }
    if let Some(max) = args.max_tokens {
        settings.max_tokens = Some(max);
    }

    // Stream run-level failures to terminal
    let sink = Arc::new(|event: roci::agent_loop::RunEvent| match &event.payload {
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Failed { error },
        } => {
            eprintln!("\n❌ {error}");
        }
        _ => {}
    });

    // Stream richer agent events to terminal
    let agent_sink = Arc::new(|event: AgentEvent| {
        use std::io::Write;
        match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => match assistant_message_event.event_type {
                StreamEventType::TextDelta => {
                    if !assistant_message_event.text.is_empty() {
                        print!("{}", assistant_message_event.text);
                        let _ = std::io::stdout().flush();
                    }
                }
                StreamEventType::Reasoning => {
                    if let Some(reasoning) = assistant_message_event.reasoning {
                        if !reasoning.is_empty() {
                            eprintln!("\n💭 {}", truncate_preview(&reasoning, 120));
                        }
                    }
                }
                _ => {}
            },
            AgentEvent::ToolExecutionStart {
                tool_name,
                tool_call_id,
                ..
            } => {
                eprintln!("\n⚡ {tool_name} ({tool_call_id})");
            }
            AgentEvent::ToolExecutionUpdate {
                tool_name,
                partial_result,
                ..
            } => {
                let preview = if let Some(text) = partial_result.content.iter().find_map(|part| {
                    if let roci::types::ContentPart::Text { text } = part {
                        Some(text.as_str())
                    } else {
                        None
                    }
                }) {
                    truncate_preview(text, 80)
                } else {
                    truncate_preview(&partial_result.details.to_string(), 80)
                };
                eprintln!("  … {tool_name}: {preview}");
            }
            AgentEvent::ToolExecutionEnd { result, .. } => {
                let preview = truncate_preview(&result.result.to_string(), 200);
                if result.is_error {
                    eprintln!("  ❌ {preview}");
                } else {
                    eprintln!("  ✅ {preview}");
                }
            }
            _ => {}
        }
    });

    // Register built-in tools
    let tools = roci_tools::builtin::all_tools();

    let mut request = RunRequest::new(model, messages);
    request.settings = settings;
    request.event_sink = Some(sink);
    request.agent_event_sink = Some(agent_sink);
    request.tools = tools;
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|call, _cancel| {
            demo_pre_tool_use_hook(&call.name, &call.id);
            Box::pin(async { Ok(PreToolUseHookResult::Continue) })
        })),
        post_tool_use: Some(Arc::new(|call, result| {
            demo_post_tool_use_hook(&call.name, &call.id);
            Box::pin(async move { Ok(result) })
        })),
    };
    request.approval_policy = ApprovalPolicy::Always;

    let handle = runner.start(request).await?;
    let result = handle.wait().await;

    println!(); // newline after streaming

    if result.status == RunStatus::Failed {
        if let Some(err) = result.error {
            return Err(err.into());
        }
    }

    Ok(())
}

async fn handle_skills(args: SkillsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let manager = SkillManager::new();
    let cwd = std::env::current_dir()?;

    match args.command {
        SkillsCommands::Install(args) => {
            let scope = skill_scope(args.local);
            let result = manager.install(&cwd, scope, &args.source)?;
            println!(
                "Installed {} skill(s) in {} scope.",
                result.installed.len(),
                skill_scope_label(scope)
            );
            for record in result.installed {
                println!(
                    "{} [{}] {}",
                    record.name,
                    managed_source_kind_label(record.source.kind),
                    record.source.value
                );
            }
        }
        SkillsCommands::Remove(args) => {
            let scope = skill_scope(args.local);
            let result = manager.remove(&cwd, scope, &args.name)?;
            if let Some(removed) = result.removed {
                println!(
                    "Removed '{}' from {} scope.",
                    removed.name,
                    skill_scope_label(scope)
                );
            } else {
                println!(
                    "No managed skill named '{}' in {} scope.",
                    args.name,
                    skill_scope_label(scope)
                );
            }
        }
        SkillsCommands::Update(args) => {
            let scope = skill_scope(args.local);
            let result = manager.update(&cwd, scope, args.name.as_deref())?;
            println!(
                "Updated {} skill(s) in {} scope.",
                result.updated.len(),
                skill_scope_label(scope)
            );
            for record in result.updated {
                println!(
                    "{} [{}] {}",
                    record.name,
                    managed_source_kind_label(record.source.kind),
                    record.source.value
                );
            }
        }
        SkillsCommands::List => {
            let result = manager.list(&cwd)?;

            println!("Managed skills:");
            if result.managed.is_empty() {
                println!("(none)");
            } else {
                for item in result.managed {
                    let status = if item.exists_on_disk { "ok" } else { "missing" };
                    println!(
                        "{} [{}] {} [{}] {}",
                        item.record.name,
                        skill_scope_label(item.scope),
                        item.install_path.display(),
                        status,
                        item.record.source.value
                    );
                }
            }

            println!();
            println!("Discovered skills:");
            if result.discovered.is_empty() {
                println!("(none)");
            } else {
                for item in result.discovered {
                    let managed_state = if item.managed.is_some() {
                        "managed"
                    } else {
                        "unmanaged"
                    };
                    println!(
                        "{} [{}] {} {}",
                        item.skill.name,
                        managed_state,
                        skill_source_label(item.skill.source),
                        item.skill.file_path.display()
                    );
                }
            }

            for diagnostic in result.diagnostics {
                eprintln!(
                    "Warning: skill {}: {}",
                    diagnostic.path.display(),
                    diagnostic.message
                );
            }
        }
    }

    Ok(())
}

fn skill_scope(local: bool) -> ManagedSkillScope {
    if local {
        ManagedSkillScope::Project
    } else {
        ManagedSkillScope::Global
    }
}

fn skill_scope_label(scope: ManagedSkillScope) -> &'static str {
    match scope {
        ManagedSkillScope::Project => "project",
        ManagedSkillScope::Global => "global",
    }
}

fn managed_source_kind_label(kind: ManagedSkillSourceKind) -> &'static str {
    match kind {
        ManagedSkillSourceKind::LocalPath => "local",
        ManagedSkillSourceKind::GitUrl => "git",
    }
}

fn skill_source_label(source: SkillSource) -> &'static str {
    match source {
        SkillSource::Explicit => "explicit",
        SkillSource::ProjectRoci => "project-roci",
        SkillSource::ProjectAgents => "project-agents",
        SkillSource::GlobalRoci => "global-roci",
        SkillSource::GlobalAgents => "global-agents",
    }
}

fn expand_chat_prompt(prompt: &str, resources: &ResourceBundle) -> String {
    resources.prompt_templates.expand_input(prompt)
}

fn demo_pre_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] preToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn demo_post_tool_use_hook(tool_name: &str, tool_call_id: &str) {
    eprintln!("[hook] postToolUse called (tool={tool_name}, id={tool_call_id})");
}

fn build_resource_system_prompt(
    base: Option<String>,
    resources: &ResourceBundle,
) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(base_prompt) = base.or_else(|| resources.context.system_prompt.clone()) {
        let trimmed = base_prompt.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }

    for append in &resources.context.append_system_prompts {
        let trimmed = append.trim();
        if !trimmed.is_empty() {
            sections.push(trimmed.to_string());
        }
    }

    if let Some(project_context) = render_project_context_section(&resources.context.context_files)
    {
        sections.push(project_context);
    }

    if sections.is_empty() {
        return None;
    }

    Some(sections.join("\n\n"))
}

fn render_project_context_section(context_files: &[ContextFileResource]) -> Option<String> {
    if context_files.is_empty() {
        return None;
    }

    let mut section = String::from("## Project Context");
    for file in context_files {
        section.push_str("\n\n### ");
        section.push_str(&file.path.display().to_string());
        section.push('\n');
        section.push_str(file.content.trim());
    }

    Some(section)
}

fn print_resource_diagnostics(resources: &ResourceBundle) {
    for warning in collect_resource_diagnostic_messages(resources) {
        eprintln!("⚠️  {warning}");
    }
}

fn collect_resource_diagnostic_messages(resources: &ResourceBundle) -> Vec<String> {
    let mut messages = Vec::new();

    for diagnostic in &resources.context.diagnostics {
        messages.push(format!(
            "resource file {}: {}",
            diagnostic.path.display(),
            diagnostic.message
        ));
    }

    for diagnostic in resources.prompt_templates.diagnostics() {
        let mut message = format!(
            "prompt template {}: {}",
            diagnostic.path.display(),
            diagnostic.message
        );
        if let Some(collision) = &diagnostic.collision {
            message.push_str(&format!(" (replaces {})", collision.display()));
        }
        messages.push(message);
    }

    for diagnostic in &resources.skills.diagnostics {
        let mut message = format!(
            "skill {}: {}",
            diagnostic.path.display(),
            diagnostic.message
        );
        if let Some(collision) = &diagnostic.collision {
            message.push_str(&format!(
                " (collides with {})",
                collision.winner_path.display()
            ));
        }
        messages.push(message);
    }

    messages
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        build_resource_system_prompt, collect_resource_diagnostic_messages, expand_chat_prompt,
    };
    use roci::resource::{
        ContextFileResource, ContextPromptResources, PromptTemplateLoader, ResourceBundle,
        ResourceDiagnostic, ResourceSettings,
    };

    #[test]
    fn system_prompt_uses_cli_base_then_appends_append_prompt_and_project_context() {
        let resources = ResourceBundle {
            settings: ResourceSettings::default(),
            context: ContextPromptResources {
                context_files: vec![
                    ContextFileResource {
                        path: PathBuf::from("/repo/AGENTS.md"),
                        content: "agent context".to_string(),
                    },
                    ContextFileResource {
                        path: PathBuf::from("/repo/CLAUDE.md"),
                        content: "claude context".to_string(),
                    },
                ],
                system_prompt: Some("system from file".to_string()),
                append_system_prompts: vec!["append instructions".to_string()],
                diagnostics: Vec::new(),
            },
            prompt_templates: Default::default(),
            skills: Default::default(),
        };

        let assembled = build_resource_system_prompt(Some("cli system".to_string()), &resources)
            .expect("assembled system prompt should exist");

        assert!(assembled.starts_with("cli system"));
        assert!(assembled.contains("append instructions"));
        assert_eq!(assembled.matches("## Project Context").count(), 1);
        assert!(assembled.contains("agent context"));
        assert!(assembled.contains("claude context"));
    }

    #[test]
    fn system_prompt_falls_back_to_resource_system_when_cli_system_is_missing() {
        let resources = ResourceBundle {
            settings: ResourceSettings::default(),
            context: ContextPromptResources {
                context_files: Vec::new(),
                system_prompt: Some("system from file".to_string()),
                append_system_prompts: Vec::new(),
                diagnostics: Vec::new(),
            },
            prompt_templates: Default::default(),
            skills: Default::default(),
        };

        let assembled = build_resource_system_prompt(None, &resources);
        assert_eq!(assembled.as_deref(), Some("system from file"));
    }

    #[test]
    fn chat_prompt_expands_slash_templates_before_execution() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let prompt_dir = cwd.join(".roci/prompts");

        fs::create_dir_all(&home).expect("home dir should be created");
        fs::create_dir_all(&prompt_dir).expect("prompt dir should be created");
        fs::write(prompt_dir.join("summarize.md"), "summary=$ARGUMENTS")
            .expect("template should be written");

        let prompt_templates = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("prompt templates should load");

        let resources = ResourceBundle {
            settings: ResourceSettings::default(),
            context: ContextPromptResources::default(),
            prompt_templates,
            skills: Default::default(),
        };

        assert_eq!(
            expand_chat_prompt("/summarize release notes", &resources),
            "summary=release notes".to_string()
        );
        assert_eq!(
            expand_chat_prompt("regular prompt", &resources),
            "regular prompt".to_string()
        );
    }

    #[test]
    fn diagnostics_include_context_and_prompt_template_warnings() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let prompt_dir = cwd.join(".roci/prompts");

        fs::create_dir_all(&home).expect("home dir should be created");
        fs::create_dir_all(&prompt_dir).expect("prompt dir should be created");
        fs::write(
            cwd.join(".roci/settings.json"),
            r#"{ "prompts": ["./missing.md"] }"#,
        )
        .expect("settings should be written");

        let prompt_templates = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("prompt templates should load");

        let resources = ResourceBundle {
            settings: ResourceSettings::default(),
            context: ContextPromptResources {
                diagnostics: vec![ResourceDiagnostic {
                    path: cwd.join("AGENTS.md"),
                    message: "Unable to read resource file".to_string(),
                }],
                ..ContextPromptResources::default()
            },
            prompt_templates,
            skills: Default::default(),
        };

        let diagnostics = collect_resource_diagnostic_messages(&resources);

        assert!(diagnostics
            .iter()
            .any(|entry| entry.contains("resource file")));
        assert!(diagnostics
            .iter()
            .any(|entry| entry.contains("prompt template")));
    }
}
