use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::RociError;

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDirectories {
    pub agent_dir: PathBuf,
    pub project_dir: PathBuf,
}

impl Default for ResourceDirectories {
    fn default() -> Self {
        Self {
            agent_dir: PathBuf::from("~/.roci/agent"),
            project_dir: PathBuf::from(".roci"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedResourceDirectories {
    pub agent_dir: PathBuf,
    pub project_dir: PathBuf,
}

impl ResourceDirectories {
    pub fn resolve(&self, cwd: &Path) -> Result<ResolvedResourceDirectories, RociError> {
        self.resolve_with_home(cwd, std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }

    pub fn resolve_with_home(
        &self,
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> Result<ResolvedResourceDirectories, RociError> {
        Ok(ResolvedResourceDirectories {
            agent_dir: resolve_path(&self.agent_dir.to_string_lossy(), cwd, home_dir)?,
            project_dir: resolve_path(&self.project_dir.to_string_lossy(), cwd, home_dir)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceSettings {
    pub prompts: Vec<PathBuf>,
    pub no_prompt_templates: bool,
    pub no_context_files: bool,
    pub compaction: CompactionSettings,
    pub branch_summary: BranchSummarySettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
    pub model: Option<String>,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            model: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSummarySettings {
    pub reserve_tokens: usize,
    pub model: Option<String>,
}

impl Default for BranchSummarySettings {
    fn default() -> Self {
        Self {
            reserve_tokens: 16_384,
            model: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceSettingsLoader {
    directories: ResourceDirectories,
}

impl Default for ResourceSettingsLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceSettingsLoader {
    pub fn new() -> Self {
        Self {
            directories: ResourceDirectories::default(),
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.directories = directories;
        self
    }

    pub fn directories(&self) -> &ResourceDirectories {
        &self.directories
    }

    pub fn load(&self, cwd: &Path) -> Result<ResourceSettings, RociError> {
        self.load_with_home(cwd, std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }

    pub fn load_with_home(
        &self,
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> Result<ResourceSettings, RociError> {
        let resolved_dirs = self.directories.resolve_with_home(cwd, home_dir)?;

        let mut merged = Value::Object(Default::default());

        if let Some(global_value) = load_scope_settings(&resolved_dirs.agent_dir, home_dir)? {
            deep_merge(&mut merged, global_value);
        }

        if let Some(project_value) = load_scope_settings(&resolved_dirs.project_dir, home_dir)? {
            deep_merge(&mut merged, project_value);
        }

        let parsed: ResourceSettingsSerde = serde_json::from_value(merged)?;

        Ok(ResourceSettings {
            prompts: parsed.prompts.into_iter().map(PathBuf::from).collect(),
            no_prompt_templates: parsed.no_prompt_templates,
            no_context_files: parsed.no_context_files,
            compaction: parsed.compaction.into(),
            branch_summary: parsed.branch_summary.into(),
        })
    }
}

#[derive(Debug, Deserialize, Default)]
struct ResourceSettingsSerde {
    #[serde(default)]
    prompts: Vec<String>,
    #[serde(default)]
    no_prompt_templates: bool,
    #[serde(default)]
    no_context_files: bool,
    #[serde(default)]
    compaction: CompactionSettingsSerde,
    #[serde(default)]
    branch_summary: BranchSummarySettingsSerde,
}

#[derive(Debug, Deserialize)]
struct CompactionSettingsSerde {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_reserve_tokens")]
    reserve_tokens: usize,
    #[serde(default = "default_keep_recent_tokens")]
    keep_recent_tokens: usize,
    #[serde(default)]
    model: Option<String>,
}

impl Default for CompactionSettingsSerde {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            reserve_tokens: default_reserve_tokens(),
            keep_recent_tokens: default_keep_recent_tokens(),
            model: None,
        }
    }
}

impl From<CompactionSettingsSerde> for CompactionSettings {
    fn from(value: CompactionSettingsSerde) -> Self {
        Self {
            enabled: value.enabled,
            reserve_tokens: value.reserve_tokens,
            keep_recent_tokens: value.keep_recent_tokens,
            model: value.model,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BranchSummarySettingsSerde {
    #[serde(default = "default_reserve_tokens")]
    reserve_tokens: usize,
    #[serde(default)]
    model: Option<String>,
}

impl Default for BranchSummarySettingsSerde {
    fn default() -> Self {
        Self {
            reserve_tokens: default_reserve_tokens(),
            model: None,
        }
    }
}

impl From<BranchSummarySettingsSerde> for BranchSummarySettings {
    fn from(value: BranchSummarySettingsSerde) -> Self {
        Self {
            reserve_tokens: value.reserve_tokens,
            model: value.model,
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_reserve_tokens() -> usize {
    16_384
}

const fn default_keep_recent_tokens() -> usize {
    20_000
}

fn load_scope_settings(
    scope_dir: &Path,
    home_dir: Option<&Path>,
) -> Result<Option<Value>, RociError> {
    let path = scope_dir.join(SETTINGS_FILE_NAME);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(RociError::Io(err)),
    };

    let mut value: Value = serde_json::from_str(&raw)?;
    if !value.is_object() {
        return Err(RociError::Configuration(format!(
            "Settings file {} must contain a JSON object",
            path.display()
        )));
    }

    resolve_prompts_in_scope(&mut value, scope_dir, home_dir)?;

    Ok(Some(value))
}

fn resolve_prompts_in_scope(
    value: &mut Value,
    scope_dir: &Path,
    home_dir: Option<&Path>,
) -> Result<(), RociError> {
    let Some(obj) = value.as_object_mut() else {
        return Ok(());
    };

    let Some(prompts_value) = obj.get_mut("prompts") else {
        return Ok(());
    };

    let Some(prompts_array) = prompts_value.as_array_mut() else {
        return Err(RociError::Configuration(format!(
            "prompts in {} must be an array of strings",
            scope_dir.join(SETTINGS_FILE_NAME).display()
        )));
    };

    let mut resolved = Vec::with_capacity(prompts_array.len());
    for prompt in prompts_array.iter() {
        let prompt = prompt.as_str().ok_or_else(|| {
            RociError::Configuration(format!(
                "prompts in {} must be an array of strings",
                scope_dir.join(SETTINGS_FILE_NAME).display()
            ))
        })?;
        let resolved_path = resolve_path(prompt, scope_dir, home_dir)?;
        resolved.push(Value::String(resolved_path.to_string_lossy().into_owned()));
    }

    *prompts_array = resolved;
    Ok(())
}

pub(crate) fn resolve_path(
    raw: &str,
    base_dir: &Path,
    home_dir: Option<&Path>,
) -> Result<PathBuf, RociError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RociError::Configuration(
            "Resource setting paths must not be empty".to_string(),
        ));
    }

    if trimmed == "~" || trimmed.starts_with("~/") {
        let home = home_dir.ok_or_else(|| {
            RociError::Configuration(format!(
                "Cannot resolve home-relative path '{trimmed}' because HOME is not set",
            ))
        })?;
        if trimmed == "~" {
            return Ok(home.to_path_buf());
        }
        return Ok(home.join(trimmed.trim_start_matches("~/")));
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return Ok(path);
    }

    Ok(base_dir.join(path))
}

fn deep_merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_obj), Value::Object(overlay_obj)) => {
            for (key, overlay_value) in overlay_obj {
                if let Some(base_value) = base_obj.get_mut(&key) {
                    deep_merge(base_value, overlay_value);
                } else {
                    base_obj.insert(key, overlay_value);
                }
            }
        }
        (base_value, overlay_value) => *base_value = overlay_value,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{ResourceDirectories, ResourceSettingsLoader};

    #[test]
    fn default_directories_resolve_to_expected_global_and_project_paths() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");

        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&cwd).expect("workspace should be created");

        let resolved = ResourceDirectories::default()
            .resolve_with_home(&cwd, Some(&home_dir))
            .expect("default directories should resolve");

        assert_eq!(resolved.agent_dir, home_dir.join(".roci/agent"));
        assert_eq!(resolved.project_dir, cwd.join(".roci"));
    }

    #[test]
    fn loader_resolves_prompt_paths_for_tilde_and_scope_relative_paths() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&cwd).expect("workspace should be created");

        let global_dir = home_dir.join(".roci/agent");
        let project_dir = cwd.join(".roci");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::create_dir_all(&project_dir).expect("project dir should be created");

        fs::write(
            global_dir.join("settings.json"),
            r#"{ "prompts": ["global.md"] }"#,
        )
        .expect("global settings should be written");

        fs::write(
            project_dir.join("settings.json"),
            r#"{ "prompts": ["~/shared/prompt.md", "./local.md"] }"#,
        )
        .expect("project settings should be written");

        let loader = ResourceSettingsLoader::new();
        let settings = loader
            .load_with_home(&cwd, Some(&home_dir))
            .expect("settings should load");

        assert_eq!(
            settings.prompts,
            vec![
                home_dir.join("shared/prompt.md"),
                project_dir.join("local.md"),
            ],
        );
        assert!(settings.compaction.enabled);
        assert_eq!(settings.compaction.reserve_tokens, 16_384);
        assert_eq!(settings.compaction.keep_recent_tokens, 20_000);
        assert_eq!(settings.compaction.model, None);
        assert_eq!(settings.branch_summary.reserve_tokens, 16_384);
        assert_eq!(settings.branch_summary.model, None);
    }

    #[test]
    fn project_settings_override_global_settings_with_deep_merge_precedence() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&cwd).expect("workspace should be created");

        let global_dir = home_dir.join(".roci/agent");
        let project_dir = cwd.join(".roci");
        fs::create_dir_all(&global_dir).expect("global dir should be created");
        fs::create_dir_all(&project_dir).expect("project dir should be created");

        fs::write(
            global_dir.join("settings.json"),
            r#"{
                "prompts": ["global.md"],
                "no_prompt_templates": true,
                "no_context_files": false,
                "compaction": {
                    "enabled": false,
                    "reserve_tokens": 8192,
                    "keep_recent_tokens": 9000,
                    "model": "anthropic:claude-3-5-haiku"
                },
                "branch_summary": {
                    "reserve_tokens": 2048,
                    "model": "openai:gpt-4o-mini"
                }
            }"#,
        )
        .expect("global settings should be written");

        fs::write(
            project_dir.join("settings.json"),
            r#"{
                "no_prompt_templates": false,
                "no_context_files": true,
                "compaction": {
                    "keep_recent_tokens": 12000
                },
                "branch_summary": {
                    "model": "openai:gpt-4.1-mini"
                }
            }"#,
        )
        .expect("project settings should be written");

        let loader = ResourceSettingsLoader::new();
        let settings = loader
            .load_with_home(&cwd, Some(&home_dir))
            .expect("settings should load");

        assert_eq!(settings.prompts, vec![global_dir.join("global.md")]);
        assert!(!settings.no_prompt_templates);
        assert!(settings.no_context_files);
        assert!(!settings.compaction.enabled);
        assert_eq!(settings.compaction.reserve_tokens, 8192);
        assert_eq!(settings.compaction.keep_recent_tokens, 12_000);
        assert_eq!(
            settings.compaction.model.as_deref(),
            Some("anthropic:claude-3-5-haiku")
        );
        assert_eq!(settings.branch_summary.reserve_tokens, 2048);
        assert_eq!(
            settings.branch_summary.model.as_deref(),
            Some("openai:gpt-4.1-mini")
        );
    }

    #[test]
    fn loader_allows_overriding_agent_and_project_directories() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&cwd).expect("workspace should be created");

        let custom_agent = home_dir.join("custom-agent");
        let custom_project = cwd.join("config").join("resource");
        fs::create_dir_all(&custom_agent).expect("custom agent dir should be created");
        fs::create_dir_all(&custom_project).expect("custom project dir should be created");

        fs::write(
            custom_agent.join("settings.json"),
            r#"{
                "prompts": ["agent.md"],
                "no_prompt_templates": true,
                "compaction": {
                    "enabled": false,
                    "reserve_tokens": 2048
                }
            }"#,
        )
        .expect("agent settings should be written");

        fs::write(
            custom_project.join("settings.json"),
            r#"{
                "prompts": ["project.md"],
                "branch_summary": {
                    "reserve_tokens": 4096
                }
            }"#,
        )
        .expect("project settings should be written");

        let loader = ResourceSettingsLoader::new().with_directories(ResourceDirectories {
            agent_dir: PathBuf::from("~/custom-agent"),
            project_dir: PathBuf::from("config/resource"),
        });

        let settings = loader
            .load_with_home(&cwd, Some(&home_dir))
            .expect("settings should load");

        assert_eq!(settings.prompts, vec![custom_project.join("project.md")]);
        assert!(settings.no_prompt_templates);
        assert!(!settings.no_context_files);
        assert!(!settings.compaction.enabled);
        assert_eq!(settings.compaction.reserve_tokens, 2048);
        assert_eq!(settings.compaction.keep_recent_tokens, 20_000);
        assert_eq!(settings.compaction.model, None);
        assert_eq!(settings.branch_summary.reserve_tokens, 4096);
        assert_eq!(settings.branch_summary.model, None);
    }

    #[test]
    fn loader_uses_default_compaction_and_branch_summary_settings_when_files_are_missing() {
        let temp = tempdir().expect("temp dir should be created");
        let home_dir = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&home_dir).expect("home dir should be created");
        fs::create_dir_all(&cwd).expect("workspace should be created");

        let loader = ResourceSettingsLoader::new();
        let settings = loader
            .load_with_home(&cwd, Some(&home_dir))
            .expect("settings should load");

        assert!(settings.prompts.is_empty());
        assert!(!settings.no_prompt_templates);
        assert!(!settings.no_context_files);
        assert!(settings.compaction.enabled);
        assert_eq!(settings.compaction.reserve_tokens, 16_384);
        assert_eq!(settings.compaction.keep_recent_tokens, 20_000);
        assert_eq!(settings.compaction.model, None);
        assert_eq!(settings.branch_summary.reserve_tokens, 16_384);
        assert_eq!(settings.branch_summary.model, None);
    }
}
