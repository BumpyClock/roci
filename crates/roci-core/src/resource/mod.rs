pub mod context;
pub mod loader;
pub mod prompts;
pub mod settings;

pub use context::{
    ContextFileResource, ContextPromptLoader, ContextPromptResources, ResourceDiagnostic,
};
pub use prompts::{
    LoadedPromptTemplates, PromptDiagnostic, PromptDiagnosticLevel, PromptTemplate,
    PromptTemplateLoader,
};
pub use settings::{
    BranchSummarySettings, CompactionSettings, ResourceDirectories, ResourceSettings,
    ResourceSettingsLoader,
};

pub use loader::{DefaultResourceLoader, ResourceBundle, ResourceLoader, SkillResourceOptions};
