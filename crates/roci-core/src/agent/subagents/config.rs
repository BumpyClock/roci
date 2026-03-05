//! TOML configuration shapes for sub-agent profile files.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::types::{ModelCandidate, SubagentProfileRef, ToolPolicy};

// ---------------------------------------------------------------------------
// Single-profile TOML shape
// ---------------------------------------------------------------------------

/// A single profile as it appears in a TOML file.
///
/// Supports both top-level (single profile per file) and nested under
/// `[[profiles]]` (multiple profiles per file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomlProfile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<SubagentProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

// ---------------------------------------------------------------------------
// Multi-profile TOML shape
// ---------------------------------------------------------------------------

/// Wrapper for a TOML file that may contain `[[profiles]]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomlProfileFileMulti {
    pub profiles: Vec<TomlProfile>,
}

// ---------------------------------------------------------------------------
// Top-level file: single or multi
// ---------------------------------------------------------------------------

/// Represents a complete TOML profile file.
///
/// A file may be either:
/// - A single profile (all fields at top level, including `name`)
/// - Multiple profiles under `[[profiles]]`
#[derive(Debug, Clone)]
pub enum TomlProfileFile {
    Single(Box<TomlProfile>),
    Multi(Vec<TomlProfile>),
}

impl TomlProfileFile {
    /// Parse a TOML string into a profile file.
    ///
    /// Tries multi-profile (`[[profiles]]`) first, then falls back to
    /// single-profile (top-level fields).
    pub fn parse(toml_str: &str) -> Result<Self, toml::de::Error> {
        // Try multi-profile first
        if let Ok(multi) = toml::from_str::<TomlProfileFileMulti>(toml_str) {
            if !multi.profiles.is_empty() {
                return Ok(Self::Multi(multi.profiles));
            }
        }
        // Fall back to single profile
        let single: TomlProfile = toml::from_str(toml_str)?;
        Ok(Self::Single(Box::new(single)))
    }

    /// Return all profiles contained in this file.
    pub fn into_profiles(self) -> Vec<TomlProfile> {
        match self {
            Self::Single(p) => vec![*p],
            Self::Multi(ps) => ps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_profile_toml() {
        let toml_str = r#"
name = "developer"
description = "General coding agent"
inherits = "builtin:developer"

[[models]]
provider = "anthropic"
model = "claude-sonnet-4.5"
reasoning_effort = "medium"

[tools]
mode = "inherit"
"#;
        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "developer");
        assert_eq!(profiles[0].inherits.as_deref(), Some("builtin:developer"));
        assert_eq!(profiles[0].models.len(), 1);
        assert_eq!(profiles[0].models[0].provider, "anthropic");
        assert_eq!(profiles[0].models[0].model, "claude-sonnet-4.5");
        assert_eq!(
            profiles[0].models[0].reasoning_effort.as_deref(),
            Some("medium")
        );
        assert_eq!(profiles[0].tools, Some(ToolPolicy::Inherit));
    }

    #[test]
    fn parse_multi_profile_toml() {
        let toml_str = r#"
[[profiles]]
name = "custom-dev"
inherits = "builtin:developer"

[[profiles.models]]
provider = "anthropic"
model = "claude-sonnet-4.5"

[[profiles]]
name = "custom-planner"
inherits = "builtin:planner"
description = "My planner"
"#;
        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "custom-dev");
        assert_eq!(profiles[1].name, "custom-planner");
        assert_eq!(profiles[1].description.as_deref(), Some("My planner"));
    }

    #[test]
    fn parse_single_profile_with_replace_tools() {
        let toml_str = r#"
name = "restricted"

[tools]
mode = "replace"
tools = ["read", "write"]
"#;
        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(
            profiles[0].tools,
            Some(ToolPolicy::Replace {
                tools: vec!["read".into(), "write".into()],
            })
        );
    }

    #[test]
    fn parse_minimal_profile() {
        let toml_str = r#"
name = "bare"
"#;
        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "bare");
        assert!(profiles[0].inherits.is_none());
        assert!(profiles[0].models.is_empty());
        assert!(profiles[0].tools.is_none());
    }
}
