//! Tests for sub-agent profile resolution, inheritance, and TOML loading.

use crate::agent::subagents::profiles::SubagentProfileRegistry;
use crate::agent::subagents::types::{
    ModelCandidate, SubagentKind, SubagentOverrides, SubagentProfile, ToolPolicy,
};
use crate::config::RociConfig;
use crate::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// Profile resolution: builtin lookup
// ---------------------------------------------------------------------------

#[test]
fn resolve_finds_builtin_developer() {
    let reg = SubagentProfileRegistry::with_builtins();
    let profile = reg.resolve("builtin:developer").unwrap();
    assert_eq!(profile.name, "builtin:developer");
    assert!(profile.system_prompt.is_some());
    assert_eq!(profile.kind, Some(SubagentKind::Developer));
}

#[test]
fn resolve_finds_builtin_planner() {
    let reg = SubagentProfileRegistry::with_builtins();
    let profile = reg.resolve("builtin:planner").unwrap();
    assert_eq!(profile.name, "builtin:planner");
    assert_eq!(profile.kind, Some(SubagentKind::Planner));
}

#[test]
fn resolve_finds_builtin_explorer() {
    let reg = SubagentProfileRegistry::with_builtins();
    let profile = reg.resolve("builtin:explorer").unwrap();
    assert_eq!(profile.name, "builtin:explorer");
    assert_eq!(profile.kind, Some(SubagentKind::Explorer));
}

#[test]
fn resolve_missing_profile_errors() {
    let reg = SubagentProfileRegistry::new();
    let err = reg.resolve("does-not-exist").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// ---------------------------------------------------------------------------
// TOML override
// ---------------------------------------------------------------------------

#[test]
fn toml_override_replaces_builtin_field() {
    let mut reg = SubagentProfileRegistry::with_builtins();
    reg.load_toml(
        r#"
name = "builtin:developer"
description = "Custom developer"
system_prompt = "Custom prompt"
"#,
    )
    .unwrap();

    let profile = reg.resolve("builtin:developer").unwrap();
    assert_eq!(profile.description.as_deref(), Some("Custom developer"));
    assert_eq!(profile.system_prompt.as_deref(), Some("Custom prompt"));
}

// ---------------------------------------------------------------------------
// Single-parent inheritance
// ---------------------------------------------------------------------------

#[test]
fn single_parent_inheritance_child_scalars_win() {
    let mut reg = SubagentProfileRegistry::with_builtins();
    reg.load_toml(
        r#"
name = "child"
inherits = "builtin:developer"
description = "Child desc"

[[models]]
provider = "openai"
model = "gpt-4o"
"#,
    )
    .unwrap();

    let resolved = reg.resolve("child").unwrap();
    assert_eq!(resolved.name, "child");
    assert_eq!(resolved.description.as_deref(), Some("Child desc"));
    // Models replace wholesale
    assert_eq!(resolved.models.len(), 1);
    assert_eq!(resolved.models[0].provider, "openai");
    // System prompt inherited from parent
    assert!(resolved.system_prompt.is_some());
    assert!(resolved.system_prompt.unwrap().contains("coding"));
    // inherits field is cleared after resolution
    assert!(resolved.inherits.is_none());
}

// ---------------------------------------------------------------------------
// Recursive inheritance (3 levels)
// ---------------------------------------------------------------------------

#[test]
fn recursive_inheritance_three_levels() {
    let mut reg = SubagentProfileRegistry::with_builtins();
    reg.load_toml(
        r#"
name = "mid"
inherits = "builtin:developer"
description = "Middle layer"
default_timeout_ms = 5000
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
    // system_prompt from builtin:developer (root)
    assert!(resolved.system_prompt.is_some());
    // timeout from mid
    assert_eq!(resolved.default_timeout_ms, Some(5000));
}

// ---------------------------------------------------------------------------
// Circular inheritance
// ---------------------------------------------------------------------------

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

#[test]
fn three_way_circular_inheritance_detected() {
    let mut reg = SubagentProfileRegistry::new();
    reg.register(SubagentProfile {
        name: "x".into(),
        inherits: Some("y".into()),
        ..Default::default()
    });
    reg.register(SubagentProfile {
        name: "y".into(),
        inherits: Some("z".into()),
        ..Default::default()
    });
    reg.register(SubagentProfile {
        name: "z".into(),
        inherits: Some("x".into()),
        ..Default::default()
    });
    let err = reg.resolve("x").unwrap_err();
    assert!(err.to_string().contains("circular"));
}

// ---------------------------------------------------------------------------
// Model candidate fallback
// ---------------------------------------------------------------------------

#[test]
fn no_viable_model_candidate_returns_error() {
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
                provider: "also-unavailable".into(),
                model: "model-b".into(),
                reasoning_effort: None,
            },
        ],
        ..Default::default()
    };
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

// ---------------------------------------------------------------------------
// resolve_effective applies overrides
// ---------------------------------------------------------------------------

#[test]
fn resolve_effective_applies_all_override_fields() {
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
fn resolve_effective_empty_overrides_is_identity() {
    let reg = SubagentProfileRegistry::with_builtins();
    let base = reg.resolve("builtin:developer").unwrap();
    let effective = reg
        .resolve_effective("builtin:developer", &SubagentOverrides::default())
        .unwrap();
    assert_eq!(base, effective);
}

// ---------------------------------------------------------------------------
// Profile version defaults to 1
// ---------------------------------------------------------------------------

#[test]
fn profile_version_defaults_to_1() {
    let mut reg = SubagentProfileRegistry::new();
    reg.load_toml(r#"name = "v-test""#).unwrap();
    let resolved = reg.resolve("v-test").unwrap();
    assert_eq!(resolved.version, 1);
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
    let resolved = reg.resolve("v2-test").unwrap();
    assert_eq!(resolved.version, 2);
}

// ---------------------------------------------------------------------------
// TOML load from roots (later root overrides)
// ---------------------------------------------------------------------------

#[test]
fn load_from_roots_later_root_overrides_earlier() {
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
    let profile = reg.resolve("shared").unwrap();
    assert_eq!(profile.description.as_deref(), Some("from root2"));
}

#[test]
fn load_from_roots_skips_missing_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let mut reg = SubagentProfileRegistry::new();
    reg.load_from_roots(&[dir.path().to_path_buf()]).unwrap();
    // No crash, no profiles loaded
    assert!(reg.resolve("anything").is_err());
}
