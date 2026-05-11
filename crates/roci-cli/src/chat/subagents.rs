use std::io::Write;
use std::path::{Path, PathBuf};

use roci::agent::subagents::{SubagentProfileRef, SubagentProfileRegistry, SubagentProfileSummary};
use roci::agent::AgentSubagentConfig;
use roci::error::RociError;
use roci::resource::ResourceDirectories;

#[derive(Debug, Clone)]
pub(crate) struct CliSubagentProfiles {
    pub registry: SubagentProfileRegistry,
    pub selected_profile: Option<SubagentProfileRef>,
}

impl CliSubagentProfiles {
    pub fn into_config(self, enabled: bool) -> Option<AgentSubagentConfig> {
        enabled.then(|| AgentSubagentConfig {
            profiles: self.registry,
            supervisor: Default::default(),
            enabled: true,
            main_profile: self.selected_profile,
        })
    }
}

pub(crate) fn load_cli_subagent_profiles(
    cwd: &Path,
    selected_profile: Option<String>,
) -> Result<CliSubagentProfiles, RociError> {
    load_cli_subagent_profiles_with_home(
        cwd,
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        selected_profile,
    )
}

fn load_cli_subagent_profiles_with_home(
    cwd: &Path,
    home_dir: Option<&Path>,
    selected_profile: Option<String>,
) -> Result<CliSubagentProfiles, RociError> {
    let mut registry = SubagentProfileRegistry::with_builtins();
    let roots = subagent_profile_roots(cwd, home_dir)?;
    registry.load_from_roots(&roots)?;

    if let Some(profile) = selected_profile.as_deref() {
        registry.resolve(profile)?;
    }

    Ok(CliSubagentProfiles {
        registry,
        selected_profile,
    })
}

fn subagent_profile_roots(cwd: &Path, home_dir: Option<&Path>) -> Result<Vec<PathBuf>, RociError> {
    let dirs = ResourceDirectories::default().resolve_with_home(cwd, home_dir)?;
    Ok(vec![
        dirs.agent_dir.clone(),
        global_agents_root(&dirs.agent_dir),
        dirs.project_dir.clone(),
        project_agents_root(&dirs.project_dir),
    ])
}

fn project_agents_root(project_dir: &Path) -> PathBuf {
    project_dir.parent().unwrap_or(project_dir).join(".agents")
}

fn global_agents_root(agent_dir: &Path) -> PathBuf {
    let mut base = agent_dir.parent();
    let agent_name = agent_dir.file_name().and_then(|name| name.to_str());
    if agent_name == Some("agent") {
        base = base.and_then(|dir| dir.parent());
    }
    base.unwrap_or(agent_dir).join(".agents")
}

pub(crate) fn print_agent_profiles(
    registry: &SubagentProfileRegistry,
    writer: &mut impl Write,
) -> Result<(), RociError> {
    let summaries = registry.profile_summaries()?;
    if summaries.is_empty() {
        writeln!(writer, "No agents found").map_err(write_error)?;
        return Ok(());
    }
    writeln!(writer, "Agents").map_err(write_error)?;
    for profile in summaries {
        writeln!(writer, "{}", format_profile_summary(&profile)).map_err(write_error)?;
    }
    Ok(())
}

fn format_profile_summary(profile: &SubagentProfileSummary) -> String {
    let display = profile
        .display_name
        .as_deref()
        .unwrap_or(profile.name.as_str());
    let default_marker = if profile.default { " default" } else { "" };
    let models = if profile.models.is_empty() {
        "models=-".to_string()
    } else {
        format!(
            "models={}",
            profile
                .models
                .iter()
                .map(|model| format!("{}:{}", model.provider, model.model))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let hint = profile
        .description
        .as_deref()
        .or(profile.infer.as_deref())
        .unwrap_or("");
    if hint.is_empty() {
        format!("  {} ({display}){default_marker} {models}", profile.name)
    } else {
        format!(
            "  {} ({display}){default_marker} {models} - {}",
            profile.name,
            truncate(hint, 96)
        )
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let mut truncated = value.chars().take(max_chars - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn write_error(error: std::io::Error) -> RociError {
    RociError::InvalidState(format!("failed to write agent profile list: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn loads_builtins_without_profile_files() {
        let temp = TempDir::new().unwrap();
        let profiles =
            load_cli_subagent_profiles_with_home(temp.path(), Some(temp.path()), None).unwrap();

        let names = profiles.registry.list_profile_refs();

        assert!(names.contains(&"builtin:developer".to_string()));
        assert!(names.contains(&"builtin:planner".to_string()));
        assert!(names.contains(&"builtin:explorer".to_string()));
    }

    #[test]
    fn loads_global_and_project_profiles_with_project_override() {
        let temp = TempDir::new().unwrap();
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(cwd.join(".roci/subagents")).unwrap();
        std::fs::create_dir_all(temp.path().join(".roci/agent/subagents")).unwrap();
        std::fs::write(
            temp.path().join(".roci/agent/subagents/custom.toml"),
            r#"
[[profiles]]
name = "custom"
display_name = "Global Custom"
"#,
        )
        .unwrap();
        std::fs::write(
            cwd.join(".roci/subagents/custom.toml"),
            r#"
[[profiles]]
name = "custom"
display_name = "Project Custom"
"#,
        )
        .unwrap();

        let profiles = load_cli_subagent_profiles_with_home(&cwd, Some(temp.path()), None).unwrap();

        let resolved = profiles.registry.resolve("custom").unwrap();
        assert_eq!(resolved.display_name.as_deref(), Some("Project Custom"));
    }

    #[test]
    fn selected_profile_must_resolve() {
        let temp = TempDir::new().unwrap();

        let err = load_cli_subagent_profiles_with_home(
            temp.path(),
            Some(temp.path()),
            Some("missing".into()),
        )
        .unwrap_err();

        assert!(err.to_string().contains("profile 'missing' not found"));
    }

    #[test]
    fn formats_profile_list() {
        let temp = TempDir::new().unwrap();
        let profiles =
            load_cli_subagent_profiles_with_home(temp.path(), Some(temp.path()), None).unwrap();
        let mut output = Vec::new();

        print_agent_profiles(&profiles.registry, &mut output).unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Agents"));
        assert!(output.contains("builtin:developer"));
    }

    #[test]
    fn truncate_respects_requested_width() {
        assert_eq!(truncate("abcdef", 0), "");
        assert_eq!(truncate("abcdef", 2), "ab");
        assert_eq!(truncate("abcdef", 3), "abc");
        assert_eq!(truncate("abcdef", 4), "a...");
        assert_eq!(truncate("abcdef", 6), "abcdef");
    }

    #[test]
    fn into_config_preserves_selected_profile_when_enabled() {
        let temp = TempDir::new().unwrap();
        let profiles = load_cli_subagent_profiles_with_home(
            temp.path(),
            Some(temp.path()),
            Some("builtin:developer".into()),
        )
        .unwrap();

        let config = profiles.into_config(true).unwrap();

        assert!(config.enabled);
        assert_eq!(config.main_profile.as_deref(), Some("builtin:developer"));
    }

    #[test]
    fn into_config_returns_none_when_disabled() {
        let temp = TempDir::new().unwrap();
        let profiles =
            load_cli_subagent_profiles_with_home(temp.path(), Some(temp.path()), None).unwrap();

        assert!(profiles.into_config(false).is_none());
    }
}
