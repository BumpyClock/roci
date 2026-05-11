# Subagent Profile Isolation Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `tsq-r0c1agt6.2`: canonical agent profile TOML parsing, validation, projection boundaries, and pure per-subagent tool/MCP isolation contracts.

**Architecture:** Keep public `agent profile` config separate from runtime projections. Extend existing `agent/subagents` profile code instead of adding a parallel system, then add focused projection helpers that downstream controller/runtime/CLI tasks can consume.

**Tech Stack:** Rust, serde/toml, existing `RociError`, existing `SubagentProfileRegistry`, existing `ToolCatalog`/`ToolVisibilityPolicy`, existing MCP server identity types, cargo test/fmt/clippy.

---

## File Structure

- Modify `crates/roci-core/src/agent/subagents/config.rs`
  - Add canonical `[subagents.<id>]` TOML schema.
  - Keep existing single-profile and `[[profiles]]` compatibility.
  - Parse canonical inline `prompt`, `model`, `tools`, `excluded_tools`, `default`.
- Modify `crates/roci-core/src/agent/subagents/types.rs`
  - Add profile fields needed by canonical schema: `excluded_tools`, `default`.
  - Add projection DTOs: `MainAgentProjection`, `SubagentProjection`, `NativeToolProjection`, `McpServerProjection`.
- Modify `crates/roci-core/src/agent/subagents/profiles.rs`
  - Validate canonical profile ids/defaults/tool conflicts/MCP refs.
  - Convert canonical TOML into `SubagentProfile`.
  - Add pure projection methods over fake/native tool catalogs and known MCP server ids.
- Modify `crates/roci-core/src/agent/subagents/mod.rs`
  - Export projection DTOs if public API needs them.
- Do not modify `roci-cli` in this task.
- Do not wire runtime dispatch enforcement in this task; `.4` owns full runtime enforcement.

## Task 1: Canonical TOML Schema Tests

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/config.rs`

- [ ] **Step 1: Add failing tests for `[subagents.<id>]` TOML**

Add tests in `config.rs` test module:

```rust
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
    assert_eq!(profiles[0].default_agent_excluded_tools, vec!["apply_patch"]);
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
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p roci-core --features agent agent::subagents::config::tests::parse_canonical -- --nocapture
```

Expected: FAIL because canonical `[subagents.<id>]` parsing and fields do not exist yet.

## Task 2: Implement Canonical TOML Parsing

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/config.rs`
- Modify: `crates/roci-core/src/agent/subagents/types.rs`

- [ ] **Step 1: Extend `SubagentProfile` fields**

In `types.rs`, add fields to `SubagentProfile`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub excluded_tools: Vec<String>,
#[serde(default)]
pub default: bool,
```

Update `Default for SubagentProfile`:

```rust
excluded_tools: Vec::new(),
default: false,
```

Update `SubagentProfileSummary` only if downstream listing should expose `default`; add:

```rust
pub default: bool,
```

and set:

```rust
default: profile.default,
```

- [ ] **Step 2: Add canonical TOML structs**

In `config.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomlSubagentProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomlSubagentsFile {
    pub subagents: std::collections::BTreeMap<String, TomlSubagentProfile>,
}
```

- [ ] **Step 3: Extend `TomlProfileFile` enum**

Change:

```rust
pub enum TomlProfileFile {
    Single(Box<TomlProfile>),
    Multi(Vec<TomlProfile>),
}
```

to:

```rust
pub enum TomlProfileFile {
    Subagents(Vec<TomlProfile>),
    Single(Box<TomlProfile>),
    Multi(Vec<TomlProfile>),
}
```

- [ ] **Step 4: Parse canonical shape first**

In `TomlProfileFile::parse`, before the current multi-profile parse, add:

```rust
if let Ok(canonical) = toml::from_str::<TomlSubagentsFile>(toml_str) {
    if !canonical.subagents.is_empty() {
        let profiles = canonical
            .subagents
            .into_iter()
            .map(|(name, profile)| canonical_to_legacy_toml_profile(name, profile))
            .collect();
        return Ok(Self::Subagents(profiles));
    }
}
```

Add helper:

```rust
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
        description: profile.description,
        kind: None,
        infer: profile.infer,
        system_prompt: profile.prompt,
        inherits: None,
        skills: profile.skills,
        mcp_servers: profile.mcp_servers,
        default_agent_excluded_tools: profile.default_agent_excluded_tools,
        models,
        tools: profile.tools.map(|tools| ToolPolicy::Replace { tools }),
        excluded_tools: profile.excluded_tools,
        default: profile.default,
        default_timeout_ms: None,
        metadata: HashMap::new(),
        version: Some(1),
    }
}
```

This intentionally lets validation reject malformed `model` later with a better `RociError`.

- [ ] **Step 5: Update `into_profiles`**

Add match arm:

```rust
Self::Subagents(ps) => ps,
```

- [ ] **Step 6: Run canonical parse tests**

Run:

```bash
cargo test -p roci-core --features agent agent::subagents::config::tests::parse_canonical -- --nocapture
```

Expected: PASS.

## Task 3: Profile Validation Tests

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`

- [ ] **Step 1: Add failing validation tests**

Add tests in `profiles.rs` test module:

```rust
#[test]
fn canonical_profile_rejects_tool_conflict() {
    let mut reg = SubagentProfileRegistry::new();
    let err = reg
        .load_toml(
            r#"
[subagents.scout]
tools = ["grep", "read_file"]
excluded_tools = ["grep"]
"#,
        )
        .unwrap_err();

    assert!(err.to_string().contains("tool"));
    assert!(err.to_string().contains("grep"));
}

#[test]
fn canonical_profile_rejects_empty_mcp_server_id() {
    let mut reg = SubagentProfileRegistry::new();
    let err = reg
        .load_toml(
            r#"
[subagents.scout]
mcp_servers = [""]
"#,
        )
        .unwrap_err();

    assert!(err.to_string().contains("mcp_servers"));
}

#[test]
fn canonical_profile_rejects_multiple_defaults() {
    let mut reg = SubagentProfileRegistry::new();
    let err = reg
        .load_toml(
            r#"
[subagents.alpha]
default = true

[subagents.beta]
default = true
"#,
        )
        .unwrap_err();

    assert!(err.to_string().contains("default"));
}

#[test]
fn canonical_profile_rejects_model_without_provider() {
    let mut reg = SubagentProfileRegistry::new();
    let err = reg
        .load_toml(
            r#"
[subagents.scout]
model = "gpt-4o"
"#,
        )
        .unwrap_err();

    assert!(err.to_string().contains("provider:model"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p roci-core --features agent agent::subagents::profiles::tests::canonical_profile_rejects -- --nocapture
```

Expected: FAIL until validation exists.

## Task 4: Implement Profile Validation

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
- Modify: `crates/roci-core/src/agent/subagents/types.rs`
- Modify: `crates/roci-core/src/agent/subagents/config.rs`

- [ ] **Step 1: Preserve canonical fields during conversion**

Add `excluded_tools` and `default` to `TomlProfile` in `config.rs`:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub excluded_tools: Vec<String>,
#[serde(default)]
pub default: bool,
```

Update existing tests that build `TomlProfile` literals if compiler requires it.

In `toml_profile_to_subagent_profile`, set:

```rust
excluded_tools: tp.excluded_tools,
default: tp.default,
```

- [ ] **Step 2: Add validation helper**

In `profiles.rs`, add:

```rust
fn validate_profile(profile: &SubagentProfile) -> Result<(), RociError> {
    if profile.name.trim().is_empty() {
        return Err(RociError::Configuration(
            "subagent profile id must not be empty".into(),
        ));
    }

    let mut seen_mcp = std::collections::BTreeSet::new();
    for server_id in &profile.mcp_servers {
        if server_id.trim().is_empty() {
            return Err(RociError::Configuration(format!(
                "profile '{}' has empty mcp_servers entry",
                profile.name
            )));
        }
        if !seen_mcp.insert(server_id.clone()) {
            return Err(RociError::Configuration(format!(
                "profile '{}' repeats mcp server '{}'",
                profile.name, server_id
            )));
        }
    }

    validate_model_candidates(profile)?;
    validate_tool_conflicts(profile)?;
    Ok(())
}
```

Add model validation:

```rust
fn validate_model_candidates(profile: &SubagentProfile) -> Result<(), RociError> {
    for candidate in &profile.models {
        if candidate.provider.trim().is_empty() || candidate.model.trim().is_empty() {
            return Err(RociError::Configuration(format!(
                "profile '{}' model must use provider:model syntax",
                profile.name
            )));
        }
    }
    Ok(())
}
```

Add conflict validation:

```rust
fn validate_tool_conflicts(profile: &SubagentProfile) -> Result<(), RociError> {
    let excluded = profile
        .excluded_tools
        .iter()
        .collect::<std::collections::BTreeSet<_>>();

    match &profile.tools {
        ToolPolicy::Replace { tools } => {
            for tool in tools {
                if excluded.contains(tool) {
                    return Err(RociError::Configuration(format!(
                        "profile '{}' lists tool '{}' in both tools and excluded_tools",
                        profile.name, tool
                    )));
                }
            }
        }
        ToolPolicy::InheritWithOverrides { add, remove } => {
            for tool in add {
                if remove.contains(tool) || excluded.contains(tool) {
                    return Err(RociError::Configuration(format!(
                        "profile '{}' has conflicting tool rule for '{}'",
                        profile.name, tool
                    )));
                }
            }
        }
        ToolPolicy::Inherit => {}
    }
    Ok(())
}
```

- [ ] **Step 3: Validate during load**

In `SubagentProfileRegistry::load_toml`, replace:

```rust
for tp in file.into_profiles() {
    self.register(toml_profile_to_subagent_profile(tp));
}
```

with:

```rust
let profiles = file
    .into_profiles()
    .into_iter()
    .map(toml_profile_to_subagent_profile)
    .collect::<Vec<_>>();
validate_default_profile_count(&profiles)?;
for profile in profiles {
    validate_profile(&profile)?;
    self.register(profile);
}
```

Add:

```rust
fn validate_default_profile_count(profiles: &[SubagentProfile]) -> Result<(), RociError> {
    let defaults = profiles.iter().filter(|profile| profile.default).count();
    if defaults > 1 {
        return Err(RociError::Configuration(
            "only one subagent profile may set default = true in a profile file".into(),
        ));
    }
    Ok(())
}
```

- [ ] **Step 4: Run validation tests**

Run:

```bash
cargo test -p roci-core --features agent agent::subagents::profiles::tests::canonical_profile_rejects -- --nocapture
```

Expected: PASS.

## Task 5: Projection Contract Tests

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
- Modify: `crates/roci-core/src/agent/subagents/types.rs`

- [ ] **Step 1: Add projection tests**

Add tests in `profiles.rs`:

```rust
#[test]
fn subagent_projection_inherits_native_tools_without_mcp_inheritance() {
    let profile = SubagentProfile {
        name: "scout".into(),
        mcp_servers: vec!["github".into()],
        ..SubagentProfile::default()
    };
    let projection = project_subagent_profile(
        &profile,
        &["read_file", "grep", "shell"],
        &["github", "docs"],
    )
    .unwrap();

    assert_eq!(projection.native_tools.model_visible, vec!["read_file", "grep", "shell"]);
    assert_eq!(projection.native_tools.dispatch, projection.native_tools.model_visible);
    assert_eq!(projection.mcp_servers.server_ids, vec!["github"]);
}

#[test]
fn subagent_projection_allowlist_cannot_grant_parent_hidden_tools() {
    let profile = SubagentProfile {
        name: "scout".into(),
        tools: ToolPolicy::Replace {
            tools: vec!["grep".into(), "shell".into()],
        },
        ..SubagentProfile::default()
    };
    let projection = project_subagent_profile(&profile, &["grep"], &[]).unwrap();

    assert_eq!(projection.native_tools.model_visible, vec!["grep"]);
    assert_eq!(projection.native_tools.dispatch, vec!["grep"]);
}

#[test]
fn subagent_projection_excludes_tools_from_visible_and_dispatch_sets() {
    let profile = SubagentProfile {
        name: "scout".into(),
        excluded_tools: vec!["shell".into()],
        ..SubagentProfile::default()
    };
    let projection = project_subagent_profile(&profile, &["read_file", "shell"], &[]).unwrap();

    assert_eq!(projection.native_tools.model_visible, vec!["read_file"]);
    assert_eq!(projection.native_tools.dispatch, vec!["read_file"]);
}

#[test]
fn main_projection_applies_default_agent_exclusions_but_child_does_not() {
    let profile = SubagentProfile {
        name: "scout".into(),
        default_agent_excluded_tools: vec!["apply_patch".into()],
        ..SubagentProfile::default()
    };

    let main = project_main_agent_profile(&profile, &["read_file", "apply_patch"], &[]).unwrap();
    let child =
        project_subagent_profile(&profile, &["read_file", "apply_patch"], &[]).unwrap();

    assert_eq!(main.native_tools.model_visible, vec!["read_file"]);
    assert_eq!(child.native_tools.model_visible, vec!["read_file", "apply_patch"]);
}

#[test]
fn mcp_projection_rejects_unknown_server_id() {
    let profile = SubagentProfile {
        name: "scout".into(),
        mcp_servers: vec!["github".into()],
        ..SubagentProfile::default()
    };
    let err = project_subagent_profile(&profile, &["read_file"], &["docs"]).unwrap_err();

    assert!(err.to_string().contains("github"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p roci-core --features agent agent::subagents::profiles::tests::subagent_projection agent::subagents::profiles::tests::main_projection agent::subagents::profiles::tests::mcp_projection -- --nocapture
```

Expected: Cargo may reject multiple filters. If so, run:

```bash
cargo test -p roci-core --features agent projection -- --nocapture
```

Expected: FAIL until projection DTOs/functions exist.

## Task 6: Implement Projection DTOs And Helpers

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/types.rs`
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
- Modify: `crates/roci-core/src/agent/subagents/mod.rs`

- [ ] **Step 1: Add projection DTOs**

In `types.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolProjection {
    pub model_visible: Vec<String>,
    pub dispatch: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerProjection {
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainAgentProjection {
    pub profile: SubagentProfileRef,
    pub display_name: Option<String>,
    pub infer: Option<String>,
    pub system_prompt: Option<String>,
    pub models: Vec<ModelCandidate>,
    pub native_tools: NativeToolProjection,
    pub skills: Vec<String>,
    pub mcp_servers: McpServerProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentProjection {
    pub profile: SubagentProfileRef,
    pub display_name: Option<String>,
    pub infer: Option<String>,
    pub system_prompt: Option<String>,
    pub models: Vec<ModelCandidate>,
    pub native_tools: NativeToolProjection,
    pub skills: Vec<String>,
    pub mcp_servers: McpServerProjection,
}
```

- [ ] **Step 2: Export DTOs**

In `mod.rs`, add projection types to the existing `pub use types::{ ... }` list:

```rust
MainAgentProjection, McpServerProjection, NativeToolProjection, SubagentProjection,
```

- [ ] **Step 3: Add projection helpers**

In `profiles.rs`, add public helpers near model resolution methods:

```rust
pub fn project_main_agent_profile(
    profile: &SubagentProfile,
    base_native_tools: &[&str],
    available_mcp_servers: &[&str],
) -> Result<MainAgentProjection, RociError> {
    let native_tools = project_native_tools(
        &profile.name,
        &profile.tools,
        &profile.default_agent_excluded_tools,
        base_native_tools,
    )?;
    let mcp_servers = project_mcp_servers(&profile.name, &profile.mcp_servers, available_mcp_servers)?;
    Ok(MainAgentProjection {
        profile: profile.name.clone(),
        display_name: profile.display_name.clone(),
        infer: profile.infer.clone(),
        system_prompt: profile.system_prompt.clone(),
        models: profile.models.clone(),
        native_tools,
        skills: profile.skills.clone(),
        mcp_servers,
    })
}

pub fn project_subagent_profile(
    profile: &SubagentProfile,
    base_native_tools: &[&str],
    available_mcp_servers: &[&str],
) -> Result<SubagentProjection, RociError> {
    let native_tools = project_native_tools(
        &profile.name,
        &profile.tools,
        &profile.excluded_tools,
        base_native_tools,
    )?;
    let mcp_servers = project_mcp_servers(&profile.name, &profile.mcp_servers, available_mcp_servers)?;
    Ok(SubagentProjection {
        profile: profile.name.clone(),
        display_name: profile.display_name.clone(),
        infer: profile.infer.clone(),
        system_prompt: profile.system_prompt.clone(),
        models: profile.models.clone(),
        native_tools,
        skills: profile.skills.clone(),
        mcp_servers,
    })
}
```

Add private helper:

```rust
fn project_native_tools(
    profile_name: &str,
    policy: &ToolPolicy,
    excluded_tools: &[String],
    base_native_tools: &[&str],
) -> Result<NativeToolProjection, RociError> {
    let base = base_native_tools
        .iter()
        .map(|name| (*name).to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let excluded = excluded_tools
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();

    let mut selected = match policy {
        ToolPolicy::Inherit => base.clone(),
        ToolPolicy::Replace { tools } => {
            for tool in tools {
                if !base.contains(tool) {
                    return Err(RociError::Configuration(format!(
                        "profile '{profile_name}' references unavailable native tool '{tool}'"
                    )));
                }
            }
            tools.iter().cloned().collect()
        }
        ToolPolicy::InheritWithOverrides { add, remove } => {
            let mut selected = base.clone();
            for tool in remove {
                if !base.contains(tool) {
                    return Err(RociError::Configuration(format!(
                        "profile '{profile_name}' references unavailable native tool '{tool}'"
                    )));
                }
                selected.remove(tool);
            }
            for tool in add {
                if !base.contains(tool) {
                    return Err(RociError::Configuration(format!(
                        "profile '{profile_name}' references unavailable native tool '{tool}'"
                    )));
                }
                selected.insert(tool.clone());
            }
            selected
        }
    };

    for tool in &excluded {
        if !base.contains(tool) {
            return Err(RociError::Configuration(format!(
                "profile '{profile_name}' references unavailable native tool '{tool}'"
            )));
        }
        selected.remove(tool);
    }

    let ordered = base_native_tools
        .iter()
        .filter(|tool| selected.contains(**tool))
        .map(|tool| (*tool).to_string())
        .collect::<Vec<_>>();
    Ok(NativeToolProjection {
        model_visible: ordered.clone(),
        dispatch: ordered,
    })
}
```

Add MCP helper:

```rust
fn project_mcp_servers(
    profile_name: &str,
    requested: &[String],
    available_mcp_servers: &[&str],
) -> Result<McpServerProjection, RociError> {
    let available = available_mcp_servers
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut server_ids = Vec::with_capacity(requested.len());
    for server_id in requested {
        if !available.contains(server_id.as_str()) {
            return Err(RociError::Configuration(format!(
                "profile '{profile_name}' references unknown MCP server '{server_id}'"
            )));
        }
        server_ids.push(server_id.clone());
    }
    Ok(McpServerProjection { server_ids })
}
```

- [ ] **Step 4: Import projection DTOs**

At top of `profiles.rs`, extend `use super::types::{ ... }` with:

```rust
MainAgentProjection, McpServerProjection, NativeToolProjection, SubagentProjection,
```

- [ ] **Step 5: Run projection tests**

Run:

```bash
cargo test -p roci-core --features agent projection -- --nocapture
```

Expected: PASS.

## Task 7: Compatibility And Existing Test Repair

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/types.rs`
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
- Modify: `crates/roci-core/src/agent/subagents/config.rs`

- [ ] **Step 1: Run all subagent tests**

Run:

```bash
cargo test -p roci-core --features agent subagents -- --nocapture
```

Expected: Some existing tests may fail to compile because new fields were added to struct literals.

- [ ] **Step 2: Repair struct literals**

For each compile error like:

```text
missing fields `excluded_tools`, `default` in initializer of `SubagentProfile`
```

Patch the literal with:

```rust
..SubagentProfile::default()
```

or explicit fields when the test must assert behavior:

```rust
excluded_tools: Vec::new(),
default: false,
```

- [ ] **Step 3: Preserve legacy TOML tests**

Existing tests for:

```toml
name = "my-dev"
[[profiles]]
[tools]
mode = "inherit"
```

must still pass. Do not remove legacy parsing in this task.

- [ ] **Step 4: Run all subagent tests again**

Run:

```bash
cargo test -p roci-core --features agent subagents -- --nocapture
```

Expected: PASS.

## Task 8: Focused Gates And Spec Sync

**Files:**
- Modify: `docs/superpowers/specs/2026-05-11-subagent-routing-profile-isolation-design.md` only if implementation reveals mismatch.

- [ ] **Step 1: Run task-specific test filters**

Run:

```bash
cargo test -p roci-core --features agent subagent_profile -- --nocapture
cargo test -p roci-core --features agent subagent_projection -- --nocapture
cargo test -p roci-core --features agent subagent_isolation -- --nocapture
```

Expected: PASS. If filters match zero tests, rename at least one relevant test to include each filter.

- [ ] **Step 2: Run broader package checks**

Run:

```bash
cargo fmt --all --check
cargo check -p roci-core --features agent,mcp
cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 3: Re-check tsq spec**

Run:

```bash
tsq spec tsq-r0c1agt6.2 --check
```

Expected:

```text
spec_ok=true
```

- [ ] **Step 4: Record verification note**

Run:

```bash
tsq note tsq-r0c1agt6.2 "Implemented profile/projection/isolation contract and verified with subagent profile/projection/isolation tests, fmt, check, clippy, and tsq spec check."
```

Expected: note added.

---

## Self-Review

- Spec coverage:
  - Canonical TOML shape: Tasks 1-2.
  - Validation conflicts/defaults/MCP/model: Tasks 3-4.
  - Projection boundaries: Tasks 5-6.
  - `.2` scope only: Task 8; no CLI/runtime controller work.
- Placeholder scan:
  - No unresolved filler text.
  - Every code-changing step has concrete snippets or exact repair instructions.
- Type consistency:
  - `SubagentProfile.excluded_tools` and `SubagentProfile.default` are introduced before use.
  - `MainAgentProjection`, `SubagentProjection`, `NativeToolProjection`, and `McpServerProjection` are defined before projection helpers use them.
  - Projection helpers use existing `ToolPolicy` and `RociError`.
