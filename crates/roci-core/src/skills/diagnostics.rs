//! Diagnostics emitted while loading skill definitions.

use std::path::PathBuf;

/// Severity level for a skill diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillDiagnosticLevel {
    /// A non-fatal issue that still allows the skill to load.
    Warning,
    /// A hard collision that prevents one skill from loading.
    Collision,
}

/// Represents a skill name collision between two skill sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCollision {
    /// The colliding skill name.
    pub name: String,
    /// The winning skill definition path.
    pub winner_path: PathBuf,
    /// The losing skill definition path.
    pub loser_path: PathBuf,
}

/// A diagnostic reported while parsing or loading a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDiagnostic {
    /// Severity/meaning of this diagnostic.
    pub level: SkillDiagnosticLevel,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Path associated with the diagnostic.
    pub path: PathBuf,
    /// Optional collision details when this is a collision diagnostic.
    pub collision: Option<SkillCollision>,
}
