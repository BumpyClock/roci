use std::path::{Path, PathBuf};

use crate::error::RociError;

use crate::skills::{
    default_skill_roots, load_skills, LoadSkillsOptions, LoadSkillsResult, SkillRoot, SkillSource,
};

use super::{
    ContextPromptLoader, ContextPromptResources, LoadedPromptTemplates, PromptTemplateLoader,
    ResourceDirectories, ResourceSettings, ResourceSettingsLoader,
};

/// Aggregated resources loaded from settings, context files, prompt templates, and skills.
#[derive(Debug, Clone)]
pub struct ResourceBundle {
    pub settings: ResourceSettings,
    pub context: ContextPromptResources,
    pub prompt_templates: LoadedPromptTemplates,
    pub skills: LoadSkillsResult,
}

/// Loader for settings, context files, prompt templates, and skills.
#[derive(Debug, Clone)]
pub struct ResourceLoader {
    settings_loader: ResourceSettingsLoader,
    context_loader: ContextPromptLoader,
    prompt_loader: PromptTemplateLoader,
    skill_options: SkillResourceOptions,
}

pub type DefaultResourceLoader = ResourceLoader;

/// Controls skill discovery when loading resources.
#[derive(Debug, Clone)]
pub struct SkillResourceOptions {
    pub enabled: bool,
    pub explicit_paths: Vec<PathBuf>,
    pub extra_roots: Vec<PathBuf>,
}

impl Default for SkillResourceOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            explicit_paths: Vec::new(),
            extra_roots: Vec::new(),
        }
    }
}

impl Default for ResourceLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader {
    pub fn new() -> Self {
        Self {
            settings_loader: ResourceSettingsLoader::new(),
            context_loader: ContextPromptLoader::new(),
            prompt_loader: PromptTemplateLoader::new(),
            skill_options: SkillResourceOptions::default(),
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.settings_loader = self.settings_loader.with_directories(directories.clone());
        self.context_loader = self.context_loader.with_directories(directories.clone());
        self.prompt_loader = self.prompt_loader.with_directories(directories);
        self
    }

    pub fn with_skill_options(mut self, options: SkillResourceOptions) -> Self {
        self.skill_options = options;
        self
    }

    pub fn load(&self, cwd: &Path) -> Result<ResourceBundle, RociError> {
        self.load_with_home(cwd, std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }

    pub fn load_with_home(
        &self,
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> Result<ResourceBundle, RociError> {
        let settings = self.settings_loader.load_with_home(cwd, home_dir)?;
        let mut context = self.context_loader.load_with_home(cwd, home_dir)?;
        let prompt_templates = if settings.no_prompt_templates {
            LoadedPromptTemplates::default()
        } else {
            self.prompt_loader.load_with_home(cwd, home_dir)?
        };

        if settings.no_context_files {
            context.context_files.clear();
        }

        let skills =
            if self.skill_options.enabled {
                let resolved = self
                    .settings_loader
                    .directories()
                    .resolve_with_home(cwd, home_dir)?;
                let mut roots = default_skill_roots(&resolved);
                roots.extend(self.skill_options.extra_roots.iter().cloned().map(|path| {
                    SkillRoot {
                        path,
                        source: SkillSource::Explicit,
                    }
                }));
                let options = LoadSkillsOptions {
                    roots,
                    explicit_paths: self.skill_options.explicit_paths.clone(),
                    follow_symlinks: true,
                };
                load_skills(&options)
            } else {
                LoadSkillsResult::default()
            };

        Ok(ResourceBundle {
            settings,
            context,
            prompt_templates,
            skills,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{ResourceLoader, SkillResourceOptions};

    #[test]
    fn loader_aggregates_settings_context_and_prompt_templates() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");

        fs::create_dir_all(home.join(".roci/agent")).expect("agent dir should be created");
        fs::create_dir_all(cwd.join(".roci/prompts"))
            .expect("project prompt dir should be created");

        fs::write(home.join(".roci/agent/AGENTS.md"), "global context")
            .expect("global context should be written");
        fs::write(cwd.join("AGENTS.md"), "project context")
            .expect("project context should be written");
        fs::write(cwd.join(".roci/SYSTEM.md"), "project system")
            .expect("project system should be written");
        fs::write(cwd.join(".roci/prompts/plan.md"), "plan body")
            .expect("template should be written");

        let bundle = ResourceLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("bundle should load");

        assert_eq!(
            bundle.context.system_prompt,
            Some("project system".to_string())
        );
        assert_eq!(bundle.context.context_files.len(), 2);
        assert_eq!(
            bundle.prompt_templates.expand_input("/plan"),
            "plan body".to_string()
        );
    }

    #[test]
    fn loader_respects_no_context_files_and_no_prompt_templates_settings() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");

        fs::create_dir_all(home.join(".roci/agent/prompts"))
            .expect("agent prompt dir should be created");
        fs::create_dir_all(cwd.join(".roci/prompts"))
            .expect("project prompt dir should be created");

        fs::write(home.join(".roci/agent/AGENTS.md"), "global context")
            .expect("global context should be written");
        fs::write(cwd.join("AGENTS.md"), "project context")
            .expect("project context should be written");
        fs::write(cwd.join(".roci/prompts/plan.md"), "plan body")
            .expect("template should be written");
        fs::write(
            cwd.join(".roci/settings.json"),
            r#"{ "no_context_files": true, "no_prompt_templates": true }"#,
        )
        .expect("settings should be written");

        let bundle = ResourceLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("bundle should load");

        assert!(bundle.context.context_files.is_empty());
        assert!(bundle.prompt_templates.templates().is_empty());
    }

    #[test]
    fn it_loads_skills_from_default_roots() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let skill_dir = cwd.join(".roci/skills/sample-skill");

        fs::create_dir_all(&home).expect("home dir should be created");
        fs::create_dir_all(&skill_dir).expect("skill dir should be created");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: sample-skill\ndescription: Sample skill\n---\n",
        )
        .expect("skill file should be written");

        let bundle = ResourceLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("bundle should load");

        assert_eq!(bundle.skills.skills.len(), 1);
        assert_eq!(bundle.skills.skills[0].name, "sample-skill");
    }

    #[test]
    fn it_skips_skill_loading_when_disabled() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let skill_dir = cwd.join(".roci/skills/disabled-skill");

        fs::create_dir_all(&home).expect("home dir should be created");
        fs::create_dir_all(&skill_dir).expect("skill dir should be created");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: disabled-skill\ndescription: Disabled skill\n---\n",
        )
        .expect("skill file should be written");

        let bundle = ResourceLoader::new()
            .with_skill_options(SkillResourceOptions {
                enabled: false,
                explicit_paths: Vec::new(),
                extra_roots: Vec::new(),
            })
            .load_with_home(&cwd, Some(&home))
            .expect("bundle should load");

        assert!(bundle.skills.skills.is_empty());
    }
}
