//! Skill loading and prompt formatting.

pub mod diagnostics;
pub mod format;
pub mod loader;
pub mod manager;
pub mod model;

mod frontmatter;

pub use diagnostics::{SkillCollision, SkillDiagnostic, SkillDiagnosticLevel};
pub use format::{format_skills_for_prompt, merge_system_prompt_with_skills};
pub use loader::{
    default_skill_roots, load_skills, LoadSkillsOptions, LoadSkillsResult, SkillRoot,
};
pub use manager::{
    DiscoveredSkillListItem, InstallManagedSkillsResult, ListManagedSkillsResult,
    ManagedSkillListItem, ManagedSkillRecord, ManagedSkillScope, ManagedSkillSource,
    ManagedSkillSourceKind, RemoveManagedSkillResult, SkillManager, UpdateManagedSkillsResult,
};
pub use model::{Skill, SkillSource};
