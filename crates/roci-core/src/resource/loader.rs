use std::path::{Path, PathBuf};

use crate::error::RociError;

use super::{
    ContextPromptLoader, ContextPromptResources, LoadedPromptTemplates, PromptTemplateLoader,
    ResourceDirectories, ResourceSettings, ResourceSettingsLoader,
};

#[derive(Debug, Clone)]
pub struct ResourceBundle {
    pub settings: ResourceSettings,
    pub context: ContextPromptResources,
    pub prompt_templates: LoadedPromptTemplates,
}

#[derive(Debug, Clone)]
pub struct ResourceLoader {
    settings_loader: ResourceSettingsLoader,
    context_loader: ContextPromptLoader,
    prompt_loader: PromptTemplateLoader,
}

pub type DefaultResourceLoader = ResourceLoader;

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
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.settings_loader = self.settings_loader.with_directories(directories.clone());
        self.context_loader = self.context_loader.with_directories(directories.clone());
        self.prompt_loader = self.prompt_loader.with_directories(directories);
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

        Ok(ResourceBundle {
            settings,
            context,
            prompt_templates,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::ResourceLoader;

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
}
