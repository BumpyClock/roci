use roci::resource::{ContextFileResource, ResourceBundle};

pub(crate) fn expand_chat_prompt(prompt: &str, resources: &ResourceBundle) -> String {
    resources.prompt_templates.expand_input(prompt)
}

pub(crate) fn build_resource_system_prompt(
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

pub(crate) fn render_project_context_section(
    context_files: &[ContextFileResource],
) -> Option<String> {
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

pub(crate) fn print_resource_diagnostics(resources: &ResourceBundle) {
    for warning in collect_resource_diagnostic_messages(resources) {
        eprintln!("⚠️  {warning}");
    }
}

pub(crate) fn collect_resource_diagnostic_messages(resources: &ResourceBundle) -> Vec<String> {
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

pub(crate) fn truncate_preview(value: &str, max_chars: usize) -> String {
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
