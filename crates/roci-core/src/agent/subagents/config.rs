//! TOML configuration shapes for sub-agent profile files.

use std::collections::{BTreeMap, HashMap};

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
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<SubagentProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_agent_excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
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
    #[serde(default)]
    pub default: bool,
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
// Canonical subagents TOML shape
// ---------------------------------------------------------------------------

/// Canonical public profile shape under `[subagents.<id>]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlSubagentProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_agent_excluded_tools: Vec<String>,
    #[serde(default)]
    pub default: bool,
}

/// Wrapper for canonical `[subagents.<id>]` TOML files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TomlSubagentsFile {
    pub subagents: BTreeMap<String, TomlSubagentProfile>,
}

// ---------------------------------------------------------------------------
// Top-level file: single or multi
// ---------------------------------------------------------------------------

/// Represents a complete TOML profile file.
///
/// A file may be either:
/// - A single profile (all fields at top level, including `name`)
/// - Multiple profiles under `[[profiles]]`
/// - Canonical public profiles under `[subagents.<id>]`
#[derive(Debug, Clone)]
pub enum TomlProfileFile {
    Subagents(Vec<TomlProfile>),
    Single(Box<TomlProfile>),
    Multi(Vec<TomlProfile>),
}

impl TomlProfileFile {
    /// Parse a TOML string into a profile file.
    ///
    /// Tries canonical subagent profiles first, then multi-profile
    /// (`[[profiles]]`), then single-profile (top-level fields).
    pub fn parse(toml_str: &str) -> Result<Self, toml::de::Error> {
        let table = toml::from_str::<toml::Table>(toml_str)?;
        if table.contains_key("subagents") {
            let canonical = toml::from_str::<TomlSubagentsFile>(toml_str)?;
            let profiles = canonical
                .subagents
                .into_iter()
                .map(|(name, profile)| canonical_to_legacy_toml_profile(name, profile))
                .collect();
            return Ok(Self::Subagents(profiles));
        }

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
            Self::Subagents(ps) => ps,
            Self::Single(p) => vec![*p],
            Self::Multi(ps) => ps,
        }
    }
}

fn canonical_to_legacy_toml_profile(name: String, profile: TomlSubagentProfile) -> TomlProfile {
    let models = profile
        .model
        .map(|model| {
            let (provider, model) = model.split_once(':').unwrap_or(("", model.as_str()));
            ModelCandidate {
                provider: provider.to_string(),
                model: model.to_string(),
                reasoning_effort: None,
            }
        })
        .into_iter()
        .collect();

    TomlProfile {
        name,
        display_name: profile.display_name,
        description: None,
        kind: None,
        infer: profile.infer,
        system_prompt: profile.prompt,
        inherits: None,
        skills: profile.skills,
        mcp_servers: profile.mcp_servers,
        default_agent_excluded_tools: profile.default_agent_excluded_tools,
        excluded_tools: profile.excluded_tools,
        models,
        tools: profile.tools.map(|tools| ToolPolicy::Replace { tools }),
        default_timeout_ms: None,
        metadata: HashMap::new(),
        version: None,
        default: profile.default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_canonical_subagents_table() {
        let toml_str = r#"
[subagents.scout]
display_name = "Scout"
infer = "Use for repo search"
model = "openai:gpt-4o"
tools = ["grep", "read_file"]
excluded_tools = ["shell"]
default_agent_excluded_tools = ["apply_patch"]
skills = ["rust-skills"]
mcp_servers = ["github"]
default = true
prompt = """
You are Scout.
Do not edit files.
"""
"#;

        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "scout");
        assert_eq!(profiles[0].display_name.as_deref(), Some("Scout"));
        assert_eq!(profiles[0].infer.as_deref(), Some("Use for repo search"));
        assert_eq!(
            profiles[0].system_prompt.as_deref(),
            Some("You are Scout.\nDo not edit files.\n")
        );
        assert_eq!(profiles[0].models.len(), 1);
        assert_eq!(profiles[0].models[0].provider, "openai");
        assert_eq!(profiles[0].models[0].model, "gpt-4o");
        assert_eq!(
            profiles[0].tools,
            Some(ToolPolicy::Replace {
                tools: vec!["grep".into(), "read_file".into()],
            })
        );
        assert_eq!(profiles[0].excluded_tools, vec!["shell"]);
        assert_eq!(
            profiles[0].default_agent_excluded_tools,
            vec!["apply_patch"]
        );
        assert_eq!(profiles[0].skills, vec!["rust-skills"]);
        assert_eq!(profiles[0].mcp_servers, vec!["github"]);
        assert!(profiles[0].default);
    }

    #[test]
    fn parse_canonical_multiple_subagents_deterministically() {
        let toml_str = r#"
[subagents.zeta]
display_name = "Zeta"

[subagents.alpha]
display_name = "Alpha"
"#;

        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "alpha");
        assert_eq!(profiles[1].name, "zeta");
    }

    #[test]
    fn parse_malformed_canonical_subagents_returns_canonical_error() {
        let toml_str = r#"
[subagents.scout]
tools = "grep"
"#;

        let err = TomlProfileFile::parse(toml_str).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("tools"));
        assert!(!message.contains("missing field `name`"));
    }

    #[test]
    fn parse_canonical_rejects_unknown_field() {
        let toml_str = r#"
[subagents.scout]
prompt_file = "scout.md"
"#;

        let err = TomlProfileFile::parse(toml_str).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("prompt_file"));
    }

    #[test]
    fn parse_canonical_rejects_legacy_description_field() {
        let toml_str = r#"
[subagents.scout]
description = "Legacy field"
"#;

        let err = TomlProfileFile::parse(toml_str).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("description"));
    }

    #[test]
    fn parse_canonical_rejects_mixed_legacy_profiles() {
        let toml_str = r#"
[subagents.scout]
display_name = "Scout"

[[profiles]]
name = "legacy"
"#;

        let err = TomlProfileFile::parse(toml_str).unwrap_err();
        let message = err.to_string();

        assert!(message.contains("profiles"));
    }

    #[test]
    fn parse_single_profile_toml() {
        let toml_str = r#"
name = "developer"
display_name = "Developer"
description = "General coding agent"
inherits = "builtin:developer"
infer = "Use for coding work"
skills = ["rust-skills", "programming"]
mcp_servers = ["github"]
default_agent_excluded_tools = ["delete_repo"]

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
        assert_eq!(profiles[0].display_name.as_deref(), Some("Developer"));
        assert_eq!(profiles[0].inherits.as_deref(), Some("builtin:developer"));
        assert_eq!(profiles[0].infer.as_deref(), Some("Use for coding work"));
        assert_eq!(
            profiles[0].skills,
            vec!["rust-skills".to_string(), "programming".to_string()]
        );
        assert_eq!(profiles[0].mcp_servers, vec!["github"]);
        assert_eq!(
            profiles[0].default_agent_excluded_tools,
            vec!["delete_repo"]
        );
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
display_name = "Custom Dev"
inherits = "builtin:developer"
infer = "Use for implementation"
skills = ["programming"]
mcp_servers = ["filesystem"]
default_agent_excluded_tools = ["ask_user"]

[[profiles.models]]
provider = "anthropic"
model = "claude-sonnet-4.5"

[[profiles]]
name = "custom-planner"
inherits = "builtin:planner"
description = "My planner"
skills = ["ux-designer"]
"#;
        let file = TomlProfileFile::parse(toml_str).unwrap();
        let profiles = file.into_profiles();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "custom-dev");
        assert_eq!(profiles[0].display_name.as_deref(), Some("Custom Dev"));
        assert_eq!(profiles[0].infer.as_deref(), Some("Use for implementation"));
        assert_eq!(profiles[0].skills, vec!["programming"]);
        assert_eq!(profiles[0].mcp_servers, vec!["filesystem"]);
        assert_eq!(profiles[0].default_agent_excluded_tools, vec!["ask_user"]);
        assert_eq!(profiles[1].name, "custom-planner");
        assert_eq!(profiles[1].description.as_deref(), Some("My planner"));
        assert_eq!(profiles[1].skills, vec!["ux-designer"]);
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
        assert!(profiles[0].display_name.is_none());
        assert!(profiles[0].infer.is_none());
        assert!(profiles[0].skills.is_empty());
        assert!(profiles[0].mcp_servers.is_empty());
        assert!(profiles[0].default_agent_excluded_tools.is_empty());
        assert!(profiles[0].models.is_empty());
        assert!(profiles[0].tools.is_none());
    }
}
