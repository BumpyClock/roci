//! Skill loading and prompt formatting.

pub mod diagnostics;
pub mod format;
pub mod loader;
pub mod model;

mod frontmatter;

pub use diagnostics::{SkillCollision, SkillDiagnostic, SkillDiagnosticLevel};
pub use format::{format_skills_for_prompt, merge_system_prompt_with_skills};
pub use loader::{
    default_skill_roots, load_skills, LoadSkillsOptions, LoadSkillsResult, SkillRoot,
};
pub use model::{Skill, SkillSource};
