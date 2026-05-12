# Approval Policy Ruleset Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace enum-centric approval policy with canonical structured policy types while preserving current user-facing approval behavior through constructors.

**Architecture:** Keep the existing `agent_loop::approvals` public boundary, but change `ApprovalPolicy` from enum to struct. Add pure evaluation types and helpers in `approvals.rs`, then update runner/runtime/CLI call sites to use `ApprovalPolicy::ask()`, `always()`, and `never()`. This task defines the seam only; full finalized-args runtime reordering stays in `tsq-1av9jz0z.1.2`.

**Tech Stack:** Rust, serde, tokio async runner, roci-core, roci-cli, cargo tests, rustfmt, clippy.

---

## File Map

- Modify `crates/roci-core/src/agent_loop/approvals.rs`: structured policy types, evaluator, constructors, unit tests.
- Modify `crates/roci-core/src/agent_loop/runner/control.rs`: map existing approval flow through evaluator and new constructors.
- Modify `crates/roci-core/src/agent_loop/runner.rs`: default policy and builder docs.
- Modify `crates/roci-core/src/agent/core.rs`: constructor docs and defaults.
- Modify `crates/roci-core/src/agent/runtime/config.rs`: default policy and docs.
- Modify `crates/roci-core/src/agent/runtime.rs`, `run_loop.rs`, `mutations.rs`, and chat domain types to replace old `Copy` use with `Clone`.
- Modify `crates/roci-cli/src/chat.rs`: CLI mapping to constructors.
- Modify examples/tests that use `ApprovalPolicy::Ask`, `Always`, or `Never`.
- Modify `docs/agent-runtime-chat.md` and `docs/ARCHITECTURE.md` references if compile/docs text still names enum variants.

## Task 1: Structured Approval Types

**Files:**
- Modify: `crates/roci-core/src/agent_loop/approvals.rs`
- Modify: `crates/roci-core/src/tools/tool.rs`
- Modify: `crates/roci-core/src/security/command.rs`
- Modify: `crates/roci-core/src/security/filesystem.rs`

- [ ] **Step 1: Add serde derives to referenced security enums**

In `crates/roci-core/src/tools/tool.rs`, add the import:

```rust
use serde::{Deserialize, Serialize};
```

Change `ToolApprovalKind` derive to:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalKind {
    CommandExecution,
    FileChange,
    Read,
    Mcp,
    CustomTool,
    Other,
}
```

In `crates/roci-core/src/security/command.rs`, add the import:

```rust
use serde::{Deserialize, Serialize};
```

Add `Serialize, Deserialize` to `ShellKind`, `CommandPlatform`, `CommandCategory`, `CommandConfidence`, and `CommandInsight` derives:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandCategory {
    ReadOnly,
    WritesFilesystem,
    DestructiveDelete,
    PrivilegeEscalation,
    PermissionChange,
    ProcessControl,
    NetworkLikely,
    CodeExecution,
    Unknown,
}
```

Use the same derive form for the other command types.

In `crates/roci-core/src/security/filesystem.rs`, add the import:

```rust
use serde::{Deserialize, Serialize};
```

Add `Serialize, Deserialize` to `PathResolutionMode`, `SymlinkPolicy`, `PathBoundary`, `PathOperation`, `PathAccessRequest`, and `FilesystemDecision` derives. Also expose existing boundary matching inside the crate by changing this signature:

```rust
fn matches(&self, path: &Path) -> bool
```

to:

```rust
pub(crate) fn matches(&self, path: &Path) -> bool
```

- [ ] **Step 2: Write constructor and precedence tests**

Add this test module at the bottom of `approvals.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ApprovalContext {
        ApprovalContext {
            tool_call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
            tool_kind: Some(crate::tools::ToolApprovalKind::CommandExecution),
            preview: serde_json::json!({"command": "echo hi"}),
            metadata: serde_json::Value::Null,
            command: None,
            filesystem: Vec::new(),
            sandbox: None,
            mcp: None,
            network: None,
            grant_key: None,
        }
    }

    #[test]
    fn approval_policy_presets_map_to_default_actions() {
        assert_eq!(ApprovalPolicy::ask().default_action, ApprovalAction::Ask);
        assert_eq!(ApprovalPolicy::always().default_action, ApprovalAction::Allow);
        assert_eq!(ApprovalPolicy::never().default_action, ApprovalAction::Deny);
    }

    #[test]
    fn deny_beats_ask_and_allow() {
        let policy = ApprovalPolicy {
            default_action: ApprovalAction::Allow,
            rules: vec![
                ApprovalRule::new(
                    "allow-shell",
                    ApprovalAction::Allow,
                    ApprovalMatcher::ToolName { name: "shell".into() },
                ),
                ApprovalRule::new(
                    "ask-shell",
                    ApprovalAction::Ask,
                    ApprovalMatcher::ToolName { name: "shell".into() },
                ),
                ApprovalRule::new(
                    "deny-shell",
                    ApprovalAction::Deny,
                    ApprovalMatcher::ToolName { name: "shell".into() },
                ),
            ],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::default(),
        };

        let evaluation = policy.evaluate(&context());
        assert_eq!(evaluation.action, ApprovalAction::Deny);
        assert_eq!(evaluation.matched_rules[0].rule_id, "deny-shell");
    }

    #[test]
    fn exact_tool_match_beats_kind_match_for_same_action() {
        let policy = ApprovalPolicy {
            default_action: ApprovalAction::Allow,
            rules: vec![
                ApprovalRule::new(
                    "ask-kind",
                    ApprovalAction::Ask,
                    ApprovalMatcher::ToolKind {
                        kind: crate::tools::ToolApprovalKind::CommandExecution,
                    },
                ),
                ApprovalRule::new(
                    "ask-tool",
                    ApprovalAction::Ask,
                    ApprovalMatcher::ToolName { name: "shell".into() },
                ),
            ],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::default(),
        };

        let evaluation = policy.evaluate(&context());
        assert_eq!(evaluation.action, ApprovalAction::Ask);
        assert_eq!(evaluation.matched_rules[0].rule_id, "ask-tool");
    }

    #[test]
    fn built_in_floor_beats_broad_allow_even_when_policy_has_no_extra_floors() {
        let mut ctx = context();
        ctx.command = Some(crate::security::command::CommandInsight {
            normalized_command: "rm -rf target".to_string(),
            primary_executable: Some("rm".to_string()),
            categories: vec![crate::security::command::CommandCategory::DestructiveDelete],
            reasons: vec!["destructive delete".to_string()],
            confidence: crate::security::command::CommandConfidence::High,
        });

        let evaluation = ApprovalPolicy::always().evaluate(&ctx);
        assert_eq!(evaluation.action, ApprovalAction::Ask);
        assert_eq!(evaluation.safety_floors[0].effect, ApprovalAction::Ask);
    }

    #[test]
    fn exact_session_grant_beats_default_ask_but_not_explicit_ask_rule() {
        let mut ctx = context();
        ctx.grant_key = Some(ApprovalGrantKey {
            permission_kind: crate::human_interaction::ToolPermissionKind::Shell,
            tool_name: "shell".to_string(),
            recipient_or_server: None,
            arguments_digest: Some(
                "1eab1ef18bb109ae99f48bfe7efaba9dde08e27bdb46438214bd6c65eb3ff2ff"
                    .to_string(),
            ),
            tool_provided_key: None,
        });
        let key = ctx.grant_key.clone().unwrap();

        let allow_policy = ApprovalPolicy {
            default_action: ApprovalAction::Ask,
            rules: Vec::new(),
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::from_grants(vec![ApprovalGrant::Exact {
                key: key.clone(),
            }]),
        };
        assert_eq!(allow_policy.evaluate(&ctx).action, ApprovalAction::Allow);

        let ask_policy = ApprovalPolicy {
            default_action: ApprovalAction::Allow,
            rules: vec![ApprovalRule::new(
                "ask-shell",
                ApprovalAction::Ask,
                ApprovalMatcher::ToolName { name: "shell".into() },
            )],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::from_grants(vec![ApprovalGrant::Exact { key }]),
        };
        assert_eq!(ask_policy.evaluate(&ctx).action, ApprovalAction::Ask);
    }
}
```

- [ ] **Step 3: Run test and verify failure**

Run:

```bash
cargo test -p roci-core --features agent approval_policy_presets_map_to_default_actions
```

Expected: compile failure because `ApprovalContext`, `ApprovalAction`, `ApprovalRule`, `ApprovalMatcher`, `ApprovalSafetyFloors`, `ApprovalGrantSet`, and constructors do not exist yet.

- [ ] **Step 4: Replace enum with structured policy types**

In `approvals.rs`, replace the current `ApprovalPolicy` enum with these types. Keep existing `ApprovalKind`, `ApprovalRequest`, `ExecPolicyUpdate`, `ApprovalDecision`, and `ApprovalHandler`.

```rust
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalPolicy {
    pub default_action: ApprovalAction,
    #[serde(default)]
    pub rules: Vec<ApprovalRule>,
    #[serde(default)]
    pub additional_safety_floors: ApprovalSafetyFloors,
    #[serde(default)]
    pub session_grants: ApprovalGrantSet,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self::ask()
    }
}

impl ApprovalPolicy {
    pub fn ask() -> Self {
        Self {
            default_action: ApprovalAction::Ask,
            rules: Vec::new(),
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::default(),
        }
    }

    pub fn always() -> Self {
        Self {
            default_action: ApprovalAction::Allow,
            ..Self::ask()
        }
    }

    pub fn never() -> Self {
        Self {
            default_action: ApprovalAction::Deny,
            ..Self::ask()
        }
    }

    pub fn evaluate(&self, context: &ApprovalContext) -> ApprovalEvaluation {
        evaluate_approval(self, context)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRule {
    pub id: String,
    pub source: ApprovalRuleSource,
    pub action: ApprovalAction,
    pub matcher: ApprovalMatcher,
    pub scope: ApprovalScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ApprovalRule {
    pub fn new(id: impl Into<String>, action: ApprovalAction, matcher: ApprovalMatcher) -> Self {
        Self {
            id: id.into(),
            source: ApprovalRuleSource::Host,
            action,
            matcher,
            scope: ApprovalScope::Once,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRuleSource {
    BuiltIn,
    Host,
    Session,
    User,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    Once,
    Session,
    PersistentHint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ApprovalMatcher {
    All { matchers: Vec<ApprovalMatcher> },
    Any { matchers: Vec<ApprovalMatcher> },
    ToolName { name: String },
    ToolKind { kind: crate::tools::ToolApprovalKind },
    CommandExecutable { executable: String },
    CommandCategory { category: crate::security::command::CommandCategory },
    CommandPattern { pattern: String },
    FilesystemPath {
        operation: crate::security::filesystem::PathOperation,
        path: PathBuf,
    },
    FilesystemBoundary {
        operation: crate::security::filesystem::PathOperation,
        boundary: crate::security::filesystem::PathBoundary,
    },
    McpServer { server: String },
    McpTool { server: Option<String>, tool: String },
    SandboxRequirement { requirement: String },
    SandboxResult { result: String },
    NetworkRequirement { requirement: String },
    Metadata { key: String, value: serde_json::Value },
}
```

## Task 2: Evaluation Context, Grants, and Algorithm

**Files:**
- Modify: `crates/roci-core/src/agent_loop/approvals.rs`

- [ ] **Step 1: Add context and grant types**

Add below matcher definitions:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalContext {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<crate::tools::ToolApprovalKind>,
    #[serde(default)]
    pub preview: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<crate::security::command::CommandInsight>,
    #[serde(default)]
    pub filesystem: Vec<crate::security::filesystem::FilesystemDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpApprovalMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_key: Option<ApprovalGrantKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpApprovalMetadata {
    pub server: String,
    pub tool: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ApprovalGrantKey {
    pub permission_kind: crate::human_interaction::ToolPermissionKind,
    pub tool_name: String,
    pub recipient_or_server: Option<String>,
    pub arguments_digest: Option<String>,
    pub tool_provided_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ApprovalGrant {
    Exact { key: ApprovalGrantKey },
    Rule { rule: Box<ApprovalRule> },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalGrantSet {
    #[serde(default)]
    pub grants: Vec<ApprovalGrant>,
}

impl ApprovalGrantSet {
    pub fn from_grants(grants: Vec<ApprovalGrant>) -> Self {
        Self { grants }
    }

    fn contains_exact(&self, key: &ApprovalGrantKey) -> bool {
        self.grants.iter().any(|grant| match grant {
            ApprovalGrant::Exact { key: candidate } => candidate == key,
            ApprovalGrant::Rule { .. } => false,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalSafetyFloors {
    #[serde(default)]
    pub floors: Vec<ApprovalSafetyFloor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalSafetyFloor {
    pub id: String,
    pub effect: ApprovalAction,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatchedApprovalRule {
    pub rule_id: String,
    pub action: ApprovalAction,
    pub specificity: ApprovalSpecificity,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApprovalSpecificity {
    Default = 0,
    Metadata = 1,
    CategoryOrKind = 2,
    PrefixOrBoundary = 3,
    Exact = 4,
    ExactInvocation = 5,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalEvaluation {
    pub action: ApprovalAction,
    pub matched_rules: Vec<MatchedApprovalRule>,
    pub safety_floors: Vec<ApprovalSafetyFloor>,
    pub suggested_grant: Option<ApprovalGrant>,
    pub reason: Option<String>,
}
```

- [ ] **Step 2: Add evaluator helpers**

Add these helpers below the type definitions:

```rust
pub fn evaluate_approval(policy: &ApprovalPolicy, context: &ApprovalContext) -> ApprovalEvaluation {
    let floors = built_in_safety_floors(context);
    if let Some(floor) = strongest_floor(&floors) {
        return ApprovalEvaluation {
            action: floor.effect,
            matched_rules: Vec::new(),
            safety_floors: floors,
            suggested_grant: context.grant_key.clone().map(|key| ApprovalGrant::Exact { key }),
            reason: Some(floor.reason),
        };
    }

    let mut matches = matching_rules(policy, context);
    if let Some(best) = matches.first().cloned() {
        return ApprovalEvaluation {
            action: best.action,
            matched_rules: matches,
            safety_floors: floors,
            suggested_grant: context.grant_key.clone().map(|key| ApprovalGrant::Exact { key }),
            reason: best.reason,
        };
    }

    if let Some(key) = &context.grant_key {
        if policy.session_grants.contains_exact(key) {
            return ApprovalEvaluation {
                action: ApprovalAction::Allow,
                matched_rules: Vec::new(),
                safety_floors: floors,
                suggested_grant: None,
                reason: Some("allowed by exact session grant".to_string()),
            };
        }
    }

    ApprovalEvaluation {
        action: policy.default_action,
        matched_rules: Vec::new(),
        safety_floors: floors,
        suggested_grant: context.grant_key.clone().map(|key| ApprovalGrant::Exact { key }),
        reason: Some("policy default action".to_string()),
    }
}

fn strongest_floor(floors: &[ApprovalSafetyFloor]) -> Option<ApprovalSafetyFloor> {
    floors
        .iter()
        .cloned()
        .max_by_key(|floor| action_rank(floor.effect))
}

fn action_rank(action: ApprovalAction) -> u8 {
    match action {
        ApprovalAction::Allow => 0,
        ApprovalAction::Ask => 1,
        ApprovalAction::Deny => 2,
    }
}

fn built_in_safety_floors(context: &ApprovalContext) -> Vec<ApprovalSafetyFloor> {
    let mut floors = Vec::new();
    if let Some(command) = &context.command {
        if command
            .categories
            .contains(&crate::security::command::CommandCategory::DestructiveDelete)
        {
            floors.push(ApprovalSafetyFloor {
                id: "destructive_command".to_string(),
                effect: ApprovalAction::Ask,
                reason: "destructive command requires approval".to_string(),
            });
        }
    }
    for decision in &context.filesystem {
        if !decision.allowed {
            floors.push(ApprovalSafetyFloor {
                id: "denied_filesystem_path".to_string(),
                effect: ApprovalAction::Deny,
                reason: decision.reason.clone(),
            });
        }
    }
    if context.sandbox.as_deref() == Some("required_unavailable") {
        floors.push(ApprovalSafetyFloor {
            id: "required_sandbox_unavailable".to_string(),
            effect: ApprovalAction::Deny,
            reason: "required sandbox unavailable".to_string(),
        });
    }
    floors
}
```

- [ ] **Step 3: Add matcher implementation**

Add:

```rust
fn matching_rules(policy: &ApprovalPolicy, context: &ApprovalContext) -> Vec<MatchedApprovalRule> {
    let mut matches: Vec<_> = policy
        .rules
        .iter()
        .filter_map(|rule| {
            let specificity = matcher_specificity(&rule.matcher, context)?;
            Some(MatchedApprovalRule {
                rule_id: rule.id.clone(),
                action: rule.action,
                specificity,
                reason: rule.reason.clone(),
            })
        })
        .collect();

    matches.sort_by(|left, right| {
        action_rank(right.action)
            .cmp(&action_rank(left.action))
            .then_with(|| right.specificity.cmp(&left.specificity))
    });
    matches
}

fn matcher_specificity(
    matcher: &ApprovalMatcher,
    context: &ApprovalContext,
) -> Option<ApprovalSpecificity> {
    match matcher {
        ApprovalMatcher::All { matchers } => matchers
            .iter()
            .map(|matcher| matcher_specificity(matcher, context))
            .collect::<Option<Vec<_>>>()
            .and_then(|items| items.into_iter().max()),
        ApprovalMatcher::Any { matchers } => matchers
            .iter()
            .filter_map(|matcher| matcher_specificity(matcher, context))
            .max(),
        ApprovalMatcher::ToolName { name } if name == &context.tool_name => {
            Some(ApprovalSpecificity::Exact)
        }
        ApprovalMatcher::ToolKind { kind } if Some(*kind) == context.tool_kind => {
            Some(ApprovalSpecificity::CategoryOrKind)
        }
        ApprovalMatcher::CommandExecutable { executable } => context
            .command
            .as_ref()
            .and_then(|command| command.primary_executable.as_ref())
            .filter(|candidate| *candidate == executable)
            .map(|_| ApprovalSpecificity::Exact),
        ApprovalMatcher::CommandCategory { category } => context
            .command
            .as_ref()
            .filter(|command| command.categories.contains(category))
            .map(|_| ApprovalSpecificity::CategoryOrKind),
        ApprovalMatcher::CommandPattern { pattern } => context
            .command
            .as_ref()
            .filter(|command| command.normalized_command.contains(pattern))
            .map(|_| ApprovalSpecificity::PrefixOrBoundary),
        ApprovalMatcher::FilesystemPath { operation: _, path } => context
            .filesystem
            .iter()
            .filter_map(|decision| decision.normalized_path.as_ref())
            .find(|candidate| *candidate == path)
            .map(|_| ApprovalSpecificity::Exact),
        ApprovalMatcher::FilesystemBoundary { operation: _, boundary } => context
            .filesystem
            .iter()
            .filter_map(|decision| decision.normalized_path.as_ref())
            .find(|candidate| boundary.matches(candidate))
            .map(|_| ApprovalSpecificity::PrefixOrBoundary),
        ApprovalMatcher::McpServer { server } => context
            .mcp
            .as_ref()
            .filter(|mcp| &mcp.server == server)
            .map(|_| ApprovalSpecificity::Exact),
        ApprovalMatcher::McpTool { server, tool } => context
            .mcp
            .as_ref()
            .filter(|mcp| server.as_ref().map_or(true, |server| server == &mcp.server) && &mcp.tool == tool)
            .map(|_| ApprovalSpecificity::Exact),
        ApprovalMatcher::SandboxRequirement { requirement } => context
            .sandbox
            .as_ref()
            .filter(|sandbox| *sandbox == requirement)
            .map(|_| ApprovalSpecificity::CategoryOrKind),
        ApprovalMatcher::SandboxResult { result } => context
            .sandbox
            .as_ref()
            .filter(|sandbox| *sandbox == result)
            .map(|_| ApprovalSpecificity::CategoryOrKind),
        ApprovalMatcher::NetworkRequirement { requirement } => context
            .network
            .as_ref()
            .filter(|network| *network == requirement)
            .map(|_| ApprovalSpecificity::CategoryOrKind),
        ApprovalMatcher::Metadata { key, value } => context
            .metadata
            .get(key)
            .filter(|candidate| *candidate == value)
            .map(|_| ApprovalSpecificity::Metadata),
        _ => None,
    }
}
```

If `PathBoundary::matches` is private, make it `pub(crate)` in `crates/roci-core/src/security/filesystem.rs`.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p roci-core --features agent approval_policy_presets_map_to_default_actions deny_beats_ask_and_allow exact_session_grant_beats_default_ask_but_not_explicit_ask_rule
```

Expected: all pass after adjusting enum constructor syntax and imports.

## Task 3: Runner Compatibility Through New Policy

**Files:**
- Modify: `crates/roci-core/src/agent_loop/runner/control.rs`
- Modify: `crates/roci-core/src/agent_loop/runner.rs`
- Modify: `crates/roci-core/src/agent/core.rs`
- Modify: `crates/roci-core/src/agent/runtime/config.rs`

- [ ] **Step 1: Update tests that construct old variants**

Search:

```bash
rg -n "ApprovalPolicy::(Ask|Always|Never)" crates/roci-core/src
```

Replace:

```rust
ApprovalPolicy::Ask
ApprovalPolicy::Always
ApprovalPolicy::Never
```

with:

```rust
ApprovalPolicy::ask()
ApprovalPolicy::always()
ApprovalPolicy::never()
```

- [ ] **Step 2: Update `resolve_approval` match**

In `control.rs`, replace the old enum match with evaluator-driven default behavior:

```rust
let approval = tool
    .map(Tool::approval)
    .unwrap_or_else(|| ToolApproval::requires_approval(ToolApprovalKind::Other));
let tool_kind = match approval {
    ToolApproval::AutoAccept { .. } => None,
    ToolApproval::RequiresApproval { kind } => Some(kind),
};
let context = ApprovalContext {
    tool_call_id: call.id.clone(),
    tool_name: call.name.clone(),
    tool_kind,
    preview: serde_json::json!({
        "tool_name": call.name.clone(),
        "tool_call_id": call.id.clone(),
    }),
    metadata: serde_json::Value::Null,
    command: None,
    filesystem: Vec::new(),
    sandbox: None,
    mcp: None,
    network: None,
    grant_key: None,
};
let evaluation = policy.evaluate(&context);
match evaluation.action {
    ApprovalAction::Allow => return ApprovalDecision::Accept,
    ApprovalAction::Deny => return ApprovalDecision::Decline,
    ApprovalAction::Ask => {}
}
```

Then keep existing prompt/session handling for `Ask`, but remove raw `arguments` from `ApprovalRequest.payload`:

```rust
payload: serde_json::json!({
    "tool_name": call.name.clone(),
    "tool_call_id": call.id.clone(),
    "preview": context.preview,
    "evaluation": evaluation,
}),
```

Keep current `ToolApproval::AutoAccept` behavior by adding this before constructing a prompt:

```rust
if matches!(approval, ToolApproval::AutoAccept { .. })
    && matches!(policy.default_action, ApprovalAction::Ask)
    && policy.rules.is_empty()
{
    return ApprovalDecision::Accept;
}
```

- [ ] **Step 3: Update defaults/docs in builders**

Replace docs:

```rust
/// Defaults to [`ApprovalPolicy::Ask`]
```

with:

```rust
/// Defaults to [`ApprovalPolicy::ask`]
```

Replace default initialization:

```rust
approval_policy: ApprovalPolicy::Ask,
```

with:

```rust
approval_policy: ApprovalPolicy::ask(),
```

- [ ] **Step 4: Run runner approval tests**

Run:

```bash
cargo test -p roci-core --features agent approval
```

Expected: existing approval lifecycle tests pass, except assertions expecting raw `payload.arguments` must be updated to assert `payload.preview` and `payload.evaluation`.

## Task 4: Runtime, CLI, Docs, and Examples

**Files:**
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`
- Modify: `crates/roci-core/src/agent/runtime/mutations.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/domain.rs`
- Modify: `crates/roci-cli/src/chat.rs`
- Modify: `examples/agent_runtime.rs`
- Modify: `examples/subagent_supervisor.rs`
- Modify: `docs/agent-runtime-chat.md`

- [ ] **Step 1: Remove `Copy` assumptions**

Search:

```bash
rg -n "\\*self\\.approval_policy|approval_policy: \\*|ApprovalPolicy" crates/roci-core/src/agent crates/roci-cli/src examples docs/agent-runtime-chat.md
```

Replace value copies with clones:

```rust
let default_approval_policy = self.approval_policy.lock().await.clone();
```

Keep `AgentConfig` and request types deriving/using `Clone`.

- [ ] **Step 2: Update runtime mutation docs and function body**

Keep signature:

```rust
pub async fn set_approval_policy(&self, policy: ApprovalPolicy) -> Result<(), RociError>
```

Inside function, assignment stays:

```rust
*runtime_policy = policy;
```

- [ ] **Step 3: Update CLI mapping**

In `crates/roci-cli/src/chat.rs`, change:

```rust
fn approval_policy_from_arg(arg: ChatApprovalArg) -> ApprovalPolicy {
    match arg {
        ChatApprovalArg::Ask => ApprovalPolicy::Ask,
        ChatApprovalArg::Always => ApprovalPolicy::Always,
        ChatApprovalArg::Never => ApprovalPolicy::Never,
    }
}
```

to:

```rust
fn approval_policy_from_arg(arg: ChatApprovalArg) -> ApprovalPolicy {
    match arg {
        ChatApprovalArg::Ask => ApprovalPolicy::ask(),
        ChatApprovalArg::Always => ApprovalPolicy::always(),
        ChatApprovalArg::Never => ApprovalPolicy::never(),
    }
}
```

- [ ] **Step 4: Update docs**

In `docs/agent-runtime-chat.md`, replace enum-oriented example:

```rust
pub approval_policy: Option<ApprovalPolicy>,
```

with this note below the snippet:

```md
`ApprovalPolicy` is a structured ruleset. Use `ApprovalPolicy::ask()`,
`ApprovalPolicy::always()`, or `ApprovalPolicy::never()` for preset behavior.
Hosts own approval UI and persistence; core owns policy evaluation.
```

- [ ] **Step 5: Run CLI tests**

Run:

```bash
cargo test -p roci-cli approval
```

Expected: CLI approval arg tests pass with constructor-backed policies. If equality assertions compare whole struct, assert `default_action` instead of old enum variant.

## Task 5: Verification and Follow-Up Note

**Files:**
- No source file changes. Run `tsq note` to record the existing `.1.2` finalized-args gate when `.1.1` keeps runtime ordering unchanged.

- [ ] **Step 1: Run focused compile and tests**

Run:

```bash
cargo fmt --check
cargo test -p roci-core --features agent approval
cargo test -p roci-cli approval
cargo check -p roci-core --features agent
cargo check -p roci-cli
```

Expected: all pass.

- [ ] **Step 2: Run full gate**

Run:

```bash
cargo test
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all pass.

- [ ] **Step 3: Record `.1.2` finalized-args gate if not fixed**

Because `.1.1` defines the seam and current approval still occurs before `pre_tool_use ReplaceArgs`, run:

```bash
tsq note tsq-1av9jz0z.1.2 "Approval evaluation must run after pre_tool_use ReplaceArgs and validation builds the finalized ApprovalContext. Do not approve raw model args before hook rewriting."
```

Expected: `tsq` records note on existing follow-up task.

- [ ] **Step 4: Live smoke through roci-cli**

Use `docs/testing.md` provider setup. Start an interactive tmux session:

```bash
tmux new -s roci-approval-policy-smoke
```

Show attach command to user:

```bash
tmux attach -t roci-approval-policy-smoke
```

Inside tmux, run roci-cli with each preset against a local/OpenAI-compatible model:

```bash
MODEL=$(
  curl -fsS http://framed:4001/v1/models \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"][0]["id"])'
)
OPENAI_API_KEY=sk-local-dummy OPENAI_BASE_URL=http://framed:4001/v1 \
  cargo run -q -p roci-cli -- chat --no-skills --model "openai:${MODEL}" --approval ask --max-tokens 16 "Reply exactly: roci approval ask ok"
OPENAI_API_KEY=sk-local-dummy OPENAI_BASE_URL=http://framed:4001/v1 \
  cargo run -q -p roci-cli -- chat --no-skills --model "openai:${MODEL}" --approval always --max-tokens 16 "Reply exactly: roci approval always ok"
OPENAI_API_KEY=sk-local-dummy OPENAI_BASE_URL=http://framed:4001/v1 \
  cargo run -q -p roci-cli -- chat --no-skills --model "openai:${MODEL}" --approval never --max-tokens 16 "Reply exactly: roci approval never ok"
```

Expected:
- CLI accepts all three approval presets.
- `ask` can prompt for mutating/custom tools.
- `always` does not crash through structured policy.
- `never` declines approval-required tools.

If framed endpoint is unreachable, use this local fallback and report the endpoint fallback:

```bash
MODEL=$(
  curl -fsS http://127.0.0.1:1234/api/v0/models \
    | python3 -c 'import json,sys; data=json.load(sys.stdin)["data"]; print(next(item["id"] for item in data if item.get("state") == "loaded"))'
)
LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
  cargo run -q -p roci-cli --features roci/lmstudio -- chat --no-skills --model "lmstudio:${MODEL}" --approval ask --max-tokens 16 "Reply exactly: roci approval ask ok"
LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
  cargo run -q -p roci-cli --features roci/lmstudio -- chat --no-skills --model "lmstudio:${MODEL}" --approval always --max-tokens 16 "Reply exactly: roci approval always ok"
LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
  cargo run -q -p roci-cli --features roci/lmstudio -- chat --no-skills --model "lmstudio:${MODEL}" --approval never --max-tokens 16 "Reply exactly: roci approval never ok"
```
