use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::RociError;

use super::settings::{resolve_path, ResourceDirectories, ResourceSettingsLoader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDiagnosticLevel {
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptDiagnostic {
    pub level: PromptDiagnosticLevel,
    pub message: String,
    pub path: PathBuf,
    pub collision: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedPromptTemplates {
    templates: HashMap<String, PromptTemplate>,
    diagnostics: Vec<PromptDiagnostic>,
}

impl LoadedPromptTemplates {
    pub fn templates(&self) -> &HashMap<String, PromptTemplate> {
        &self.templates
    }

    pub fn diagnostics(&self) -> &[PromptDiagnostic] {
        &self.diagnostics
    }

    pub fn get(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.get(name)
    }

    pub fn expand_input(&self, input: &str) -> String {
        let Some((name, arguments)) = parse_command_invocation(input) else {
            return input.to_string();
        };

        let Some(template) = self.templates.get(name) else {
            return input.to_string();
        };

        let args: Vec<&str> = arguments.split_whitespace().collect();
        substitute_template_arguments(&template.body, &args)
    }
}

#[derive(Debug, Clone)]
pub struct PromptTemplateLoader {
    settings_loader: ResourceSettingsLoader,
}

impl Default for PromptTemplateLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptTemplateLoader {
    pub fn new() -> Self {
        Self {
            settings_loader: ResourceSettingsLoader::new(),
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.settings_loader = self.settings_loader.with_directories(directories);
        self
    }

    pub fn load(&self, cwd: &Path) -> Result<LoadedPromptTemplates, RociError> {
        self.load_with_home(cwd, std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }

    pub fn load_with_home(
        &self,
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> Result<LoadedPromptTemplates, RociError> {
        let mut loaded = LoadedPromptTemplates::default();
        let resolved = self
            .settings_loader
            .directories()
            .resolve_with_home(cwd, home_dir)?;
        let settings = self.settings_loader.load_with_home(cwd, home_dir)?;

        load_templates_from_directory(
            &resolved.agent_dir.join("prompts"),
            &mut loaded.templates,
            &mut loaded.diagnostics,
        );
        load_templates_from_directory(
            &resolved.project_dir.join("prompts"),
            &mut loaded.templates,
            &mut loaded.diagnostics,
        );

        for explicit_path in settings.prompts {
            let normalized = resolve_path(&explicit_path.to_string_lossy(), cwd, home_dir)?;
            load_templates_from_path(&normalized, &mut loaded.templates, &mut loaded.diagnostics);
        }

        Ok(loaded)
    }
}

fn warning(path: &Path, message: impl Into<String>) -> PromptDiagnostic {
    PromptDiagnostic {
        level: PromptDiagnosticLevel::Warning,
        message: message.into(),
        path: path.to_path_buf(),
        collision: None,
    }
}

fn collision(path: &Path, previous: &Path, name: &str) -> PromptDiagnostic {
    PromptDiagnostic {
        level: PromptDiagnosticLevel::Warning,
        message: format!(
            "Prompt template command '{name}' overrides template from {}",
            previous.display()
        ),
        path: path.to_path_buf(),
        collision: Some(previous.to_path_buf()),
    }
}

fn load_templates_from_path(
    path: &Path,
    templates: &mut HashMap<String, PromptTemplate>,
    diagnostics: &mut Vec<PromptDiagnostic>,
) {
    if path.is_dir() {
        load_templates_from_directory(path, templates, diagnostics);
        return;
    }

    if !path.exists() {
        diagnostics.push(warning(path, "Prompt path does not exist"));
        return;
    }

    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return;
    }

    load_template_file(path, templates, diagnostics);
}

fn load_templates_from_directory(
    directory: &Path,
    templates: &mut HashMap<String, PromptTemplate>,
    diagnostics: &mut Vec<PromptDiagnostic>,
) {
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            diagnostics.push(warning(
                directory,
                format!("Unable to read prompt directory: {error}"),
            ));
            return;
        }
    };

    let mut files = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(error) => {
                        diagnostics.push(warning(
                            &path,
                            format!("Unable to inspect prompt entry: {error}"),
                        ));
                        continue;
                    }
                };
                if !file_type.is_file() {
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                files.push(path);
            }
            Err(error) => diagnostics.push(warning(
                directory,
                format!("Unable to iterate prompt directory entry: {error}"),
            )),
        }
    }

    files.sort();
    for path in files {
        load_template_file(&path, templates, diagnostics);
    }
}

fn load_template_file(
    path: &Path,
    templates: &mut HashMap<String, PromptTemplate>,
    diagnostics: &mut Vec<PromptDiagnostic>,
) {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            diagnostics.push(warning(
                path,
                format!("Unable to read prompt file: {error}"),
            ));
            return;
        }
    };

    let name = match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) if !stem.is_empty() => stem.to_string(),
        _ => {
            diagnostics.push(warning(
                path,
                "Prompt file name could not be converted into a command",
            ));
            return;
        }
    };

    let (frontmatter, body) = split_frontmatter(&raw);
    let description = frontmatter
        .as_ref()
        .and_then(|frontmatter| frontmatter.description.as_deref())
        .and_then(non_empty_trimmed)
        .or_else(|| first_non_empty_line(&body))
        .unwrap_or_else(|| name.clone());

    let template = PromptTemplate {
        name: name.clone(),
        description,
        body,
        path: path.to_path_buf(),
    };

    if let Some(previous) = templates.insert(name.clone(), template) {
        diagnostics.push(collision(path, &previous.path, &name));
    }
}

#[derive(Debug, Deserialize)]
struct PromptFrontmatter {
    description: Option<String>,
}

fn split_frontmatter(content: &str) -> (Option<PromptFrontmatter>, String) {
    let Some(stripped) = content.strip_prefix("---\n") else {
        return (None, content.to_string());
    };

    let Some(end_index) = stripped.find("\n---") else {
        return (None, content.to_string());
    };

    let frontmatter_text = &stripped[..end_index];
    let remainder = &stripped[end_index + 4..];
    let body = remainder
        .strip_prefix('\n')
        .unwrap_or(remainder)
        .to_string();
    let frontmatter = serde_yaml::from_str::<PromptFrontmatter>(frontmatter_text).ok();

    (frontmatter, body)
}

fn first_non_empty_line(content: &str) -> Option<String> {
    content.lines().find_map(non_empty_trimmed)
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn parse_command_invocation(input: &str) -> Option<(&str, &str)> {
    let command = input.strip_prefix('/')?;
    let command_end = command.find(char::is_whitespace).unwrap_or(command.len());
    if command_end == 0 {
        return None;
    }

    let name = &command[..command_end];
    let remainder = command[command_end..].trim_start();
    Some((name, remainder))
}

fn substitute_template_arguments(template: &str, args: &[&str]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut index = 0;

    while index < template.len() {
        let remaining = &template[index..];

        if remaining.starts_with("$ARGUMENTS") {
            output.push_str(&args.join(" "));
            index += "$ARGUMENTS".len();
            continue;
        }

        if remaining.starts_with("$@") {
            output.push_str(&args.join(" "));
            index += 2;
            continue;
        }

        if remaining.starts_with("${@:") {
            if let Some((consumed, value)) = parse_slice_substitution(remaining, args) {
                output.push_str(&value);
                index += consumed;
                continue;
            }
        }

        if let Some((consumed, value)) = parse_positional_substitution(remaining, args) {
            output.push_str(&value);
            index += consumed;
            continue;
        }

        let ch = remaining
            .chars()
            .next()
            .expect("template index should always point to a valid char");
        output.push(ch);
        index += ch.len_utf8();
    }

    output
}

fn parse_positional_substitution(fragment: &str, args: &[&str]) -> Option<(usize, String)> {
    if !fragment.starts_with('$') {
        return None;
    }

    let digit_count = fragment
        .chars()
        .skip(1)
        .take_while(|ch| ch.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return None;
    }

    let number_text = &fragment[1..1 + digit_count];
    let index = number_text.parse::<usize>().ok()?;
    if index == 0 {
        return Some((1 + digit_count, String::new()));
    }

    let value = args.get(index - 1).copied().unwrap_or("").to_string();
    Some((1 + digit_count, value))
}

fn parse_slice_substitution(fragment: &str, args: &[&str]) -> Option<(usize, String)> {
    let end_brace = fragment.find('}')?;
    let token = &fragment[..=end_brace];
    let inner = &fragment["${@:".len()..end_brace];
    let mut parts = inner.split(':');

    let start = parts.next()?.parse::<usize>().ok()?;
    let length = parts
        .next()
        .map(|part| part.parse::<usize>().ok())
        .flatten();
    if parts.next().is_some() {
        return None;
    }

    if start == 0 {
        return Some((token.len(), String::new()));
    }

    let start_index = start - 1;
    let slice = if let Some(length) = length {
        if length == 0 {
            &args[0..0]
        } else {
            let end_index = start_index.saturating_add(length).min(args.len());
            if start_index >= args.len() {
                &args[0..0]
            } else {
                &args[start_index..end_index]
            }
        }
    } else if start_index >= args.len() {
        &args[0..0]
    } else {
        &args[start_index..]
    };

    Some((token.len(), slice.join(" ")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::PromptTemplateLoader;

    #[test]
    fn project_templates_override_global_templates_and_emit_diagnostics() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let project_resource_dir = cwd.join(".roci");
        let project_prompt_dir = project_resource_dir.join("prompts");
        let global_prompt_dir = home_dir.join(".roci/agent/prompts");

        fs::create_dir_all(&project_prompt_dir).expect("project prompt dir should be created");
        fs::create_dir_all(&global_prompt_dir).expect("global prompt dir should be created");

        fs::write(global_prompt_dir.join("plan.md"), "global plan")
            .expect("global prompt should be written");
        fs::write(project_prompt_dir.join("plan.md"), "project plan")
            .expect("project prompt should be written");

        let explicit_prompt_dir = project_resource_dir.join("extra-prompts");
        fs::create_dir_all(explicit_prompt_dir.join("nested"))
            .expect("explicit nested prompt dir should be created");
        fs::write(explicit_prompt_dir.join("summarize.md"), "summary body")
            .expect("explicit prompt should be written");
        fs::write(
            explicit_prompt_dir.join("nested").join("ignored.md"),
            "ignored prompt",
        )
        .expect("nested explicit prompt should be written");

        let shared_prompt_path = home_dir.join("shared.md");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::write(
            &shared_prompt_path,
            "---\ndescription: Shared prompt\n---\nshared body",
        )
        .expect("shared prompt should be written");

        fs::write(
            project_resource_dir.join("settings.json"),
            r#"{ "prompts": ["./extra-prompts", "~/shared.md", "./missing.md"] }"#,
        )
        .expect("project settings should be written");

        let loaded = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home_dir))
            .expect("prompt templates should load");

        assert_eq!(
            loaded.get("plan").expect("plan template should exist").body,
            "project plan"
        );
        assert!(loaded.get("summarize").is_some());
        assert!(loaded.get("shared").is_some());
        assert!(loaded.get("ignored").is_none());

        let collision_detected = loaded
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.collision.is_some());
        assert!(collision_detected, "expected a collision diagnostic");

        let unreadable_detected = loaded
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("does not exist"));
        assert!(
            unreadable_detected,
            "expected an unreadable path diagnostic"
        );
    }

    #[test]
    fn template_argument_substitution_supports_all_supported_variable_forms() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let prompt_dir = cwd.join(".roci/prompts");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&prompt_dir).expect("prompt dir should be created");

        fs::write(
            prompt_dir.join("compose.md"),
            "one=$1 two=$2 all=$@ args=$ARGUMENTS from2=${@:2} range=${@:2:2} literal=$3",
        )
        .expect("prompt template should be written");

        let loaded = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home_dir))
            .expect("prompt templates should load");

        let expanded = loaded.expand_input("/compose alpha beta $3 delta");
        assert_eq!(
            expanded,
            "one=alpha two=beta all=alpha beta $3 delta args=alpha beta $3 delta from2=beta $3 delta range=beta $3 literal=$3"
        );
    }

    #[test]
    fn prompt_expansion_passes_through_when_input_is_not_a_known_command() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let prompt_dir = cwd.join(".roci/prompts");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&prompt_dir).expect("prompt dir should be created");

        fs::write(prompt_dir.join("known.md"), "known-body").expect("prompt should be written");

        let loaded = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home_dir))
            .expect("prompt templates should load");

        assert_eq!(loaded.expand_input("known"), "known");
        assert_eq!(loaded.expand_input("/unknown alpha"), "/unknown alpha");
        assert_eq!(
            loaded.expand_input("hello /known alpha"),
            "hello /known alpha"
        );
        assert_eq!(loaded.expand_input("/known"), "known-body");
    }

    #[test]
    fn description_uses_frontmatter_then_first_non_empty_line_as_fallback() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let prompt_dir = cwd.join(".roci/prompts");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&prompt_dir).expect("prompt dir should be created");

        fs::write(
            prompt_dir.join("frontmatter.md"),
            "---\ndescription: Frontmatter description\n---\nbody",
        )
        .expect("frontmatter prompt should be written");
        fs::write(
            prompt_dir.join("fallback.md"),
            "\n\nFirst line\nSecond line",
        )
        .expect("fallback prompt should be written");

        let loaded = PromptTemplateLoader::new()
            .load_with_home(&cwd, Some(&home_dir))
            .expect("prompt templates should load");

        assert_eq!(
            loaded
                .get("frontmatter")
                .expect("frontmatter template should exist")
                .description,
            "Frontmatter description"
        );
        assert_eq!(
            loaded
                .get("fallback")
                .expect("fallback template should exist")
                .description,
            "First line"
        );
    }
}
