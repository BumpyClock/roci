//! Sub-agent profile registry: lookup, inheritance, and model resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;

use super::config::{TomlProfile, TomlProfileFile};
use super::types::{SubagentKind, SubagentOverrides, SubagentProfile, ToolPolicy};

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Registry of named sub-agent profiles with inheritance resolution.
#[derive(Debug, Clone, Default)]
pub struct SubagentProfileRegistry {
    profiles: HashMap<String, SubagentProfile>,
}

impl SubagentProfileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry pre-loaded with the three built-in profiles.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();
        reg.register(SubagentProfile::builtin_developer());
        reg.register(SubagentProfile::builtin_planner());
        reg.register(SubagentProfile::builtin_explorer());
        reg
    }

    /// Register a profile by name. Overwrites any existing profile with the
    /// same name.
    pub fn register(&mut self, profile: SubagentProfile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    /// Parse a TOML string and register all contained profiles.
    pub fn load_toml(&mut self, toml_str: &str) -> Result<(), RociError> {
        let file = TomlProfileFile::parse(toml_str)
            .map_err(|e| RociError::Configuration(format!("invalid profile TOML: {e}")))?;
        for tp in file.into_profiles() {
            self.register(toml_profile_to_subagent_profile(tp));
        }
        Ok(())
    }

    /// Load profiles from a single TOML file on disk.
    pub fn load_toml_file(&mut self, path: &Path) -> Result<(), RociError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            RociError::Configuration(format!("cannot read profile file {}: {e}", path.display()))
        })?;
        self.load_toml(&content)
    }

    /// Discover and load `subagents/*.toml` from each root directory.
    ///
    /// Later roots override earlier ones (e.g. project root overrides global
    /// config dir). User-defined profiles can override built-ins.
    pub fn load_from_roots(&mut self, roots: &[PathBuf]) -> Result<(), RociError> {
        for root in roots {
            let dir = root.join("subagents");
            if !dir.is_dir() {
                continue;
            }
            let mut entries: Vec<_> = std::fs::read_dir(&dir)
                .map_err(|e| {
                    RociError::Configuration(format!(
                        "cannot read directory {}: {e}",
                        dir.display()
                    ))
                })?
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
                .collect();
            // Sort for deterministic load order within a single root.
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                self.load_toml_file(&entry.path())?;
            }
        }
        Ok(())
    }

    /// Resolve a profile reference, applying single-parent inheritance.
    ///
    /// Returns a fully-merged `SubagentProfile` with the inheritance chain
    /// flattened. Returns an error if the profile is not found or if
    /// circular inheritance is detected.
    pub fn resolve(&self, profile_ref: &str) -> Result<SubagentProfile, RociError> {
        let mut chain = Vec::new();
        self.collect_chain(profile_ref, &mut chain)?;
        Ok(merge_chain(&chain))
    }

    /// Resolve a profile and then layer per-spawn overrides on top.
    pub fn resolve_effective(
        &self,
        profile_ref: &str,
        overrides: &SubagentOverrides,
    ) -> Result<SubagentProfile, RociError> {
        let mut profile = self.resolve(profile_ref)?;
        apply_overrides(&mut profile, overrides);
        Ok(profile)
    }

    /// Pick the first viable model candidate from a resolved profile.
    ///
    /// A candidate is viable when the provider is registered **and**
    /// credentials are configured. This is intended for launch-time
    /// selection only (no runtime fallback).
    pub fn resolve_model(
        &self,
        profile: &SubagentProfile,
        registry: &ProviderRegistry,
        config: &RociConfig,
    ) -> Result<LanguageModel, RociError> {
        for candidate in &profile.models {
            if registry.has_provider(&candidate.provider)
                && config.has_credentials(&candidate.provider)
            {
                return Ok(LanguageModel::Known {
                    provider_key: candidate.provider.clone(),
                    model_id: candidate.model.clone(),
                });
            }
        }
        Err(RociError::Configuration("no viable model candidate".into()))
    }

    // -- internal -----------------------------------------------------------

    /// Walk the `inherits` chain, collecting profiles from child to root.
    fn collect_chain(&self, name: &str, chain: &mut Vec<SubagentProfile>) -> Result<(), RociError> {
        if chain.iter().any(|p| p.name == name) {
            return Err(RociError::Configuration(format!(
                "circular inheritance detected for profile '{name}'"
            )));
        }
        let profile = self
            .profiles
            .get(name)
            .ok_or_else(|| RociError::Configuration(format!("profile '{name}' not found")))?;
        chain.push(profile.clone());
        if let Some(parent_ref) = &profile.inherits {
            self.collect_chain(parent_ref, chain)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Inheritance merge
// ---------------------------------------------------------------------------

/// Merge an inheritance chain (child-first, root-last) into a single profile.
///
/// Rules:
/// - Child scalar fields replace parent scalar fields when present.
/// - `models` replaces wholesale (not merged).
/// - `tools` uses the child's policy directly (ToolPolicy handles semantics).
/// - `metadata` is merged with child keys winning.
fn merge_chain(chain: &[SubagentProfile]) -> SubagentProfile {
    debug_assert!(!chain.is_empty());

    // Start from the root (last element) and layer children on top.
    let mut result = chain.last().unwrap().clone();
    // Clear inherits on the final merged result.
    result.inherits = None;

    for child in chain.iter().rev().skip(1) {
        // Name always comes from the requested (first) profile.
        result.name = child.name.clone();
        if child.description.is_some() {
            result.description.clone_from(&child.description);
        }
        if child.kind.is_some() {
            result.kind.clone_from(&child.kind);
        }
        if child.system_prompt.is_some() {
            result.system_prompt.clone_from(&child.system_prompt);
        }
        if !child.models.is_empty() {
            result.models.clone_from(&child.models);
        }
        if child.tools != ToolPolicy::Inherit {
            result.tools = child.tools.clone();
        }
        if child.default_timeout_ms.is_some() {
            result.default_timeout_ms = child.default_timeout_ms;
        }
        if !child.metadata.is_empty() {
            for (k, v) in &child.metadata {
                result.metadata.insert(k.clone(), v.clone());
            }
        }
        result.version = child.version;
    }
    result
}

// ---------------------------------------------------------------------------
// Overrides
// ---------------------------------------------------------------------------

fn apply_overrides(profile: &mut SubagentProfile, overrides: &SubagentOverrides) {
    if let Some(prompt) = &overrides.system_prompt {
        profile.system_prompt = Some(prompt.clone());
    }
    if let Some(model) = &overrides.model {
        profile.models = vec![model.clone()];
    }
    if let Some(tools) = &overrides.tools {
        profile.tools = tools.clone();
    }
    if let Some(timeout) = overrides.timeout_ms {
        profile.default_timeout_ms = Some(timeout);
    }
}

// ---------------------------------------------------------------------------
// TOML -> SubagentProfile conversion
// ---------------------------------------------------------------------------

fn toml_profile_to_subagent_profile(tp: TomlProfile) -> SubagentProfile {
    SubagentProfile {
        name: tp.name,
        description: tp.description,
        kind: tp.kind.map(|k| match k.as_str() {
            "developer" => SubagentKind::Developer,
            "planner" => SubagentKind::Planner,
            "explorer" => SubagentKind::Explorer,
            other => SubagentKind::Custom(other.to_string()),
        }),
        system_prompt: tp.system_prompt,
        tools: tp.tools.unwrap_or_default(),
        models: tp.models,
        inherits: tp.inherits,
        default_timeout_ms: tp.default_timeout_ms,
        metadata: tp.metadata,
        version: tp.version.unwrap_or(1),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagents::types::ModelCandidate;

    // -- TOML roundtrip -----------------------------------------------------

    #[test]
    fn toml_parse_single_profile_roundtrip() {
        let toml_str = r#"
name = "my-dev"
description = "Custom developer"
inherits = "builtin:developer"

[[models]]
provider = "anthropic"
model = "claude-sonnet-4.5"
reasoning_effort = "medium"

[tools]
mode = "inherit"
"#;
        let mut reg = SubagentProfileRegistry::with_builtins();
        reg.load_toml(toml_str).unwrap();
        let profile = reg.profiles.get("my-dev").unwrap();
        assert_eq!(profile.name, "my-dev");
        assert_eq!(profile.inherits.as_deref(), Some("builtin:developer"));
        assert_eq!(profile.models.len(), 1);
        assert_eq!(profile.models[0].provider, "anthropic");
    }

    #[test]
    fn toml_parse_multi_profile_roundtrip() {
        let toml_str = r#"
[[profiles]]
name = "alpha"

[[profiles.models]]
provider = "openai"
model = "gpt-4o"

[[profiles]]
name = "beta"
description = "Beta agent"
"#;
        let mut reg = SubagentProfileRegistry::new();
        reg.load_toml(toml_str).unwrap();
        assert!(reg.profiles.contains_key("alpha"));
        assert!(reg.profiles.contains_key("beta"));
        assert_eq!(
            reg.profiles.get("beta").unwrap().description.as_deref(),
            Some("Beta agent")
        );
    }

    // -- Builtin lookup -----------------------------------------------------

    #[test]
    fn builtin_lookup() {
        let reg = SubagentProfileRegistry::with_builtins();
        let dev = reg.resolve("builtin:developer").unwrap();
        assert_eq!(dev.name, "builtin:developer");
        assert!(dev.system_prompt.is_some());

        let planner = reg.resolve("builtin:planner").unwrap();
        assert_eq!(planner.name, "builtin:planner");

        let explorer = reg.resolve("builtin:explorer").unwrap();
        assert_eq!(explorer.name, "builtin:explorer");
    }

    #[test]
    fn missing_profile_returns_error() {
        let reg = SubagentProfileRegistry::new();
        let err = reg.resolve("nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // -- Single-parent inheritance ------------------------------------------

    #[test]
    fn single_parent_inheritance_merge() {
        let mut reg = SubagentProfileRegistry::with_builtins();
        reg.load_toml(
            r#"
name = "child"
inherits = "builtin:developer"
description = "Child description"

[[models]]
provider = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();

        let resolved = reg.resolve("child").unwrap();
        // Child scalar replaces parent
        assert_eq!(resolved.description.as_deref(), Some("Child description"));
        // Models replace wholesale
        assert_eq!(resolved.models.len(), 1);
        assert_eq!(resolved.models[0].provider, "openai");
        // Inherits system_prompt from parent
        assert!(resolved.system_prompt.is_some());
        assert!(resolved.system_prompt.unwrap().contains("coding"));
        // inherits field is cleared on resolved profile
        assert!(resolved.inherits.is_none());
    }

    #[test]
    fn recursive_inheritance() {
        let mut reg = SubagentProfileRegistry::with_builtins();
        reg.load_toml(
            r#"
name = "mid"
inherits = "builtin:developer"
description = "Middle layer"
"#,
        )
        .unwrap();
        reg.load_toml(
            r#"
name = "leaf"
inherits = "mid"

[[models]]
provider = "anthropic"
model = "claude-opus-4"
"#,
        )
        .unwrap();

        let resolved = reg.resolve("leaf").unwrap();
        assert_eq!(resolved.name, "leaf");
        // description from mid (leaf has none)
        assert_eq!(resolved.description.as_deref(), Some("Middle layer"));
        // models from leaf
        assert_eq!(resolved.models[0].model, "claude-opus-4");
        // system_prompt from builtin:developer
        assert!(resolved.system_prompt.is_some());
    }

    #[test]
    fn circular_inheritance_detected() {
        let mut reg = SubagentProfileRegistry::new();
        reg.register(SubagentProfile {
            name: "a".into(),
            inherits: Some("b".into()),
            ..Default::default()
        });
        reg.register(SubagentProfile {
            name: "b".into(),
            inherits: Some("a".into()),
            ..Default::default()
        });
        let err = reg.resolve("a").unwrap_err();
        assert!(err.to_string().contains("circular"));
    }

    // -- Model candidate fallback -------------------------------------------

    #[test]
    fn model_candidate_fallback() {
        let reg = SubagentProfileRegistry::new();
        let profile = SubagentProfile {
            name: "test".into(),
            models: vec![
                ModelCandidate {
                    provider: "unavailable".into(),
                    model: "model-a".into(),
                    reasoning_effort: None,
                },
                ModelCandidate {
                    provider: "anthropic".into(),
                    model: "claude-sonnet-4.5".into(),
                    reasoning_effort: None,
                },
            ],
            ..Default::default()
        };

        // Use real ProviderRegistry and RociConfig.
        // The default ones have no providers/credentials, so we test the
        // error path.
        let provider_reg = ProviderRegistry::new();
        let config = RociConfig::default();

        let err = reg.resolve_model(&profile, &provider_reg, &config);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("no viable model"));
    }

    #[test]
    fn empty_models_returns_error() {
        let reg = SubagentProfileRegistry::new();
        let profile = SubagentProfile {
            name: "empty".into(),
            ..Default::default()
        };
        let provider_reg = ProviderRegistry::new();
        let config = RociConfig::default();
        let err = reg.resolve_model(&profile, &provider_reg, &config);
        assert!(err.is_err());
    }

    // -- Profile versioning -------------------------------------------------

    #[test]
    fn profile_version_defaults_to_1() {
        let mut reg = SubagentProfileRegistry::new();
        reg.load_toml(r#"name = "v-test""#).unwrap();
        let profile = reg.profiles.get("v-test").unwrap();
        assert_eq!(profile.version, 1);
    }

    #[test]
    fn profile_version_from_toml() {
        let mut reg = SubagentProfileRegistry::new();
        reg.load_toml(
            r#"
name = "v2-test"
version = 2
"#,
        )
        .unwrap();
        let profile = reg.profiles.get("v2-test").unwrap();
        assert_eq!(profile.version, 2);
    }

    // -- Discovery root precedence ------------------------------------------

    #[test]
    fn load_from_roots_later_overrides_earlier() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        let sa_dir1 = dir1.path().join("subagents");
        let sa_dir2 = dir2.path().join("subagents");
        std::fs::create_dir_all(&sa_dir1).unwrap();
        std::fs::create_dir_all(&sa_dir2).unwrap();

        std::fs::write(
            sa_dir1.join("test.toml"),
            r#"name = "shared"
description = "from root1"
"#,
        )
        .unwrap();
        std::fs::write(
            sa_dir2.join("test.toml"),
            r#"name = "shared"
description = "from root2"
"#,
        )
        .unwrap();

        let mut reg = SubagentProfileRegistry::new();
        reg.load_from_roots(&[dir1.path().to_path_buf(), dir2.path().to_path_buf()])
            .unwrap();
        let profile = reg.profiles.get("shared").unwrap();
        assert_eq!(profile.description.as_deref(), Some("from root2"));
    }

    #[test]
    fn load_from_roots_skips_missing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // No subagents/ subdir
        let mut reg = SubagentProfileRegistry::new();
        reg.load_from_roots(&[dir.path().to_path_buf()]).unwrap();
        assert!(reg.profiles.is_empty());
    }

    #[test]
    fn user_profile_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let sa_dir = dir.path().join("subagents");
        std::fs::create_dir_all(&sa_dir).unwrap();
        std::fs::write(
            sa_dir.join("override.toml"),
            r#"name = "builtin:developer"
description = "Custom developer override"
system_prompt = "Custom prompt"
"#,
        )
        .unwrap();

        let mut reg = SubagentProfileRegistry::with_builtins();
        reg.load_from_roots(&[dir.path().to_path_buf()]).unwrap();

        let profile = reg.profiles.get("builtin:developer").unwrap();
        assert_eq!(
            profile.description.as_deref(),
            Some("Custom developer override")
        );
    }

    // -- resolve_effective --------------------------------------------------

    #[test]
    fn resolve_effective_applies_overrides() {
        let reg = SubagentProfileRegistry::with_builtins();
        let overrides = SubagentOverrides {
            system_prompt: Some("Override prompt".into()),
            model: Some(ModelCandidate {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                reasoning_effort: None,
            }),
            tools: Some(ToolPolicy::Replace {
                tools: vec!["read".into()],
            }),
            timeout_ms: Some(30_000),
        };
        let profile = reg
            .resolve_effective("builtin:developer", &overrides)
            .unwrap();
        assert_eq!(profile.system_prompt.as_deref(), Some("Override prompt"));
        assert_eq!(profile.models.len(), 1);
        assert_eq!(profile.models[0].provider, "openai");
        assert_eq!(
            profile.tools,
            ToolPolicy::Replace {
                tools: vec!["read".into()]
            }
        );
        assert_eq!(profile.default_timeout_ms, Some(30_000));
    }

    #[test]
    fn resolve_effective_with_empty_overrides_is_identity() {
        let reg = SubagentProfileRegistry::with_builtins();
        let base = reg.resolve("builtin:developer").unwrap();
        let effective = reg
            .resolve_effective("builtin:developer", &SubagentOverrides::default())
            .unwrap();
        assert_eq!(base, effective);
    }
}
