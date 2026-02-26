//! Data types describing a loaded skill definition.

use std::path::PathBuf;

/// Represents a parsed and loadable skill definition.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Human-readable skill name used for lookup.
    pub name: String,
    /// Short skill description used in prompts.
    pub description: String,
    /// Absolute or relative path to the skill source file.
    pub file_path: PathBuf,
    /// The directory that contains the skill definition file.
    pub base_dir: PathBuf,
    /// Whether the model must skip invoking this skill.
    pub disable_model_invocation: bool,
    /// Source priority bucket for the skill.
    pub source: SkillSource,
}

/// Indicates where the skill definition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// Explicit skill path passed by user configuration.
    Explicit,
    /// Skill discovered under `.roci/skills` in the project.
    ProjectRoci,
    /// Skill discovered under `.agents/skills` in the project.
    ProjectAgents,
    /// Skill discovered under the global Roci directory.
    GlobalRoci,
    /// Skill discovered under the global Agents directory.
    GlobalAgents,
}
