//! Approval types for tool execution policies.

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Tool approval policy for a run.
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

    #[must_use]
    pub fn evaluate(&self, context: &ApprovalContext) -> ApprovalEvaluation {
        evaluate_approval(self, context)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
    All {
        matchers: Vec<ApprovalMatcher>,
    },
    Any {
        matchers: Vec<ApprovalMatcher>,
    },
    ToolName {
        name: String,
    },
    ToolKind {
        kind: crate::tools::ToolSafetyKind,
    },
    CommandExecutable {
        executable: String,
    },
    CommandCategory {
        category: crate::security::command::CommandCategory,
    },
    CommandPattern {
        pattern: String,
    },
    FilesystemPath {
        operation: crate::security::filesystem::PathOperation,
        path: PathBuf,
    },
    FilesystemBoundary {
        operation: crate::security::filesystem::PathOperation,
        boundary: crate::security::filesystem::PathBoundary,
    },
    McpServer {
        server: String,
    },
    McpTool {
        server: Option<String>,
        tool: String,
    },
    SandboxRequirement {
        requirement: String,
    },
    SandboxResult {
        result: String,
    },
    NetworkRequirement {
        requirement: String,
    },
    Metadata {
        key: String,
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalContext {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<crate::tools::ToolSafetyKind>,
    #[serde(default)]
    pub preview: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<crate::security::command::CommandInsight>,
    #[serde(default)]
    pub filesystem: Vec<ApprovalFilesystemAccess>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_floor: Option<ApprovalSafetyFloor>,
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
pub struct ApprovalFilesystemAccess {
    pub operation: crate::security::filesystem::PathOperation,
    pub decision: crate::security::filesystem::FilesystemDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpApprovalMetadata {
    pub server: String,
    pub tool: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalGrantKey {
    pub permission_kind: crate::human_interaction::ToolPermissionKind,
    pub tool_name: String,
    pub recipient_or_server: Option<String>,
    pub arguments_digest: Option<String>,
    pub tool_provided_key: Option<String>,
}

impl ApprovalGrantKey {
    pub fn new(
        permission_kind: crate::human_interaction::ToolPermissionKind,
        tool_name: impl Into<String>,
        recipient_or_server: Option<String>,
        arguments: Option<serde_json::Value>,
        tool_provided_key: Option<String>,
    ) -> Self {
        let canonical_arguments = arguments.map(canonicalize_json_value);
        let arguments_digest = canonical_arguments
            .as_ref()
            .and_then(|arguments| serde_json::to_vec(arguments).ok())
            .map(|bytes| format!("{:x}", Sha256::digest(bytes)));
        Self {
            permission_kind,
            tool_name: tool_name.into(),
            recipient_or_server,
            arguments_digest,
            tool_provided_key,
        }
    }
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
#[serde(rename_all = "snake_case")]
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
    #[serde(default)]
    pub matched_session_grant: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_grant: Option<ApprovalGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[must_use]
pub fn evaluate_approval(policy: &ApprovalPolicy, context: &ApprovalContext) -> ApprovalEvaluation {
    let floors = active_safety_floors(policy, context);
    let strongest_floor = strongest_floor(&floors);

    let matches = matching_rules(policy, context);
    if let Some(best) = matches.first().cloned() {
        let (action, reason) =
            action_with_floor(best.action, best.reason.clone(), &strongest_floor);
        return ApprovalEvaluation {
            action,
            matched_rules: matches,
            safety_floors: floors,
            matched_session_grant: false,
            suggested_grant: context
                .grant_key
                .clone()
                .map(|key| ApprovalGrant::Exact { key }),
            reason,
        };
    }

    if let Some(key) = &context.grant_key {
        if policy.session_grants.contains_exact(key) {
            let (action, reason) = action_with_floor(
                ApprovalAction::Allow,
                Some("allowed by exact session grant".to_string()),
                &strongest_floor,
            );
            return ApprovalEvaluation {
                action,
                matched_rules: Vec::new(),
                safety_floors: floors,
                matched_session_grant: true,
                suggested_grant: None,
                reason,
            };
        }
    }

    let (action, reason) = action_with_floor(
        policy.default_action,
        Some("policy default action".to_string()),
        &strongest_floor,
    );
    ApprovalEvaluation {
        action,
        matched_rules: Vec::new(),
        safety_floors: floors,
        matched_session_grant: false,
        suggested_grant: context
            .grant_key
            .clone()
            .map(|key| ApprovalGrant::Exact { key }),
        reason,
    }
}

fn action_with_floor(
    action: ApprovalAction,
    reason: Option<String>,
    floor: &Option<ApprovalSafetyFloor>,
) -> (ApprovalAction, Option<String>) {
    match floor {
        Some(floor) if action_rank(floor.effect) > action_rank(action) => {
            (floor.effect, Some(floor.reason.clone()))
        }
        _ => (action, reason),
    }
}

fn active_safety_floors(
    policy: &ApprovalPolicy,
    context: &ApprovalContext,
) -> Vec<ApprovalSafetyFloor> {
    let mut floors = built_in_safety_floors(context);
    floors.extend(
        policy
            .additional_safety_floors
            .floors
            .iter()
            .filter(|floor| floor.effect != ApprovalAction::Allow)
            .cloned(),
    );
    floors
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
    if let Some(floor) = &context.action_floor {
        floors.push(floor.clone());
    } else if let Some(command) = &context.command {
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
    for access in &context.filesystem {
        if !access.decision.allowed {
            floors.push(ApprovalSafetyFloor {
                id: "denied_filesystem_path".to_string(),
                effect: ApprovalAction::Deny,
                reason: access.decision.reason.clone(),
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
        // All fails on any non-match; Any ignores non-matches. Both rank by the
        // most specific matching child so narrow nested rules win ties.
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
        ApprovalMatcher::FilesystemPath { operation, path } => context
            .filesystem
            .iter()
            .filter(|access| access.operation == *operation)
            .filter_map(|access| access.decision.normalized_path.as_ref())
            .find(|candidate| *candidate == path)
            .map(|_| ApprovalSpecificity::Exact),
        ApprovalMatcher::FilesystemBoundary {
            operation,
            boundary,
        } => context
            .filesystem
            .iter()
            .filter(|access| access.operation == *operation)
            .filter_map(|access| access.decision.normalized_path.as_ref())
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
            .filter(|mcp| {
                server.as_ref().is_none_or(|server| server == &mcp.server) && &mcp.tool == tool
            })
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

fn canonicalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_json_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in entries {
                sorted.insert(key, canonicalize_json_value(value));
            }
            serde_json::Value::Object(sorted)
        }
        other => other,
    }
}

/// Approval request type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
    Other,
}

/// An approval request emitted by the agent loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub kind: ApprovalKind,
    #[serde(default = "default_approval_request_allow_session")]
    pub allow_session: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_policy_change: Option<ExecPolicyUpdate>,
}

fn default_approval_request_allow_session() -> bool {
    true
}

/// Optional execpolicy update suggestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecPolicyUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub argv: Vec<String>,
}

/// Approval decision for a request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

/// Async approval handler callback.
pub type ApprovalHandler =
    Arc<dyn Fn(ApprovalRequest) -> BoxFuture<'static, ApprovalDecision> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ApprovalContext {
        ApprovalContext {
            tool_call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
            tool_kind: Some(crate::tools::ToolSafetyKind::CommandExecution),
            preview: serde_json::json!({"command": "echo hi"}),
            metadata: serde_json::Value::Null,
            command: None,
            filesystem: Vec::new(),
            action_floor: None,
            sandbox: None,
            mcp: None,
            network: None,
            grant_key: None,
        }
    }

    #[test]
    fn approval_policy_presets_map_to_default_actions() {
        assert_eq!(ApprovalPolicy::ask().default_action, ApprovalAction::Ask);
        assert_eq!(
            ApprovalPolicy::always().default_action,
            ApprovalAction::Allow
        );
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
                    ApprovalMatcher::ToolName {
                        name: "shell".into(),
                    },
                ),
                ApprovalRule::new(
                    "ask-shell",
                    ApprovalAction::Ask,
                    ApprovalMatcher::ToolName {
                        name: "shell".into(),
                    },
                ),
                ApprovalRule::new(
                    "deny-shell",
                    ApprovalAction::Deny,
                    ApprovalMatcher::ToolName {
                        name: "shell".into(),
                    },
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
    fn deny_shell_command_rule_beats_ask_and_allow_matches() {
        let mut ctx = context();
        ctx.command = Some(crate::security::command::CommandInsight {
            normalized_command: "git status --short".to_string(),
            primary_executable: Some("git".to_string()),
            categories: vec![crate::security::command::CommandCategory::ReadOnly],
            reasons: vec!["read-only command".to_string()],
            confidence: crate::security::command::CommandConfidence::High,
        });
        let policy = ApprovalPolicy {
            default_action: ApprovalAction::Allow,
            rules: vec![
                ApprovalRule::new(
                    "allow-shell",
                    ApprovalAction::Allow,
                    ApprovalMatcher::ToolName {
                        name: "shell".into(),
                    },
                ),
                ApprovalRule::new(
                    "ask-git-status",
                    ApprovalAction::Ask,
                    ApprovalMatcher::CommandPattern {
                        pattern: "status".into(),
                    },
                ),
                ApprovalRule::new(
                    "deny-git",
                    ApprovalAction::Deny,
                    ApprovalMatcher::CommandExecutable {
                        executable: "git".into(),
                    },
                ),
            ],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::default(),
        };

        let evaluation = policy.evaluate(&ctx);

        assert_eq!(evaluation.action, ApprovalAction::Deny);
        assert_eq!(
            evaluation
                .matched_rules
                .iter()
                .map(|rule| rule.rule_id.as_str())
                .collect::<Vec<_>>(),
            vec!["deny-git", "ask-git-status", "allow-shell"]
        );
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
                        kind: crate::tools::ToolSafetyKind::CommandExecution,
                    },
                ),
                ApprovalRule::new(
                    "ask-tool",
                    ApprovalAction::Ask,
                    ApprovalMatcher::ToolName {
                        name: "shell".into(),
                    },
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
    fn destructive_command_safety_floor_beats_broad_shell_allow_rule() {
        let mut ctx = context();
        ctx.command = Some(crate::security::command::CommandInsight {
            normalized_command: "rm -rf target".to_string(),
            primary_executable: Some("rm".to_string()),
            categories: vec![crate::security::command::CommandCategory::DestructiveDelete],
            reasons: vec!["destructive delete".to_string()],
            confidence: crate::security::command::CommandConfidence::High,
        });
        let policy = ApprovalPolicy {
            default_action: ApprovalAction::Ask,
            rules: vec![ApprovalRule::new(
                "allow-shell",
                ApprovalAction::Allow,
                ApprovalMatcher::ToolName {
                    name: "shell".into(),
                },
            )],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::default(),
        };

        let evaluation = policy.evaluate(&ctx);

        assert_eq!(evaluation.action, ApprovalAction::Ask);
        assert_eq!(evaluation.matched_rules[0].rule_id, "allow-shell");
        assert_eq!(evaluation.safety_floors[0].id, "destructive_command");
    }

    #[test]
    fn ask_floor_does_not_weaken_default_deny() {
        let mut ctx = context();
        ctx.command = Some(crate::security::command::CommandInsight {
            normalized_command: "rm -rf target".to_string(),
            primary_executable: Some("rm".to_string()),
            categories: vec![crate::security::command::CommandCategory::DestructiveDelete],
            reasons: vec!["destructive delete".to_string()],
            confidence: crate::security::command::CommandConfidence::High,
        });

        let evaluation = ApprovalPolicy::never().evaluate(&ctx);
        assert_eq!(evaluation.action, ApprovalAction::Deny);
        assert_eq!(evaluation.safety_floors[0].effect, ApprovalAction::Ask);
    }

    #[test]
    fn approval_matcher_serializes_nested_security_enums_as_snake_case() {
        let matcher = ApprovalMatcher::CommandCategory {
            category: crate::security::command::CommandCategory::DestructiveDelete,
        };
        let json = serde_json::to_value(&matcher).expect("matcher serializes");

        assert_eq!(
            json,
            serde_json::json!({
                "type": "command_category",
                "category": "destructive_delete",
            })
        );

        let operation = serde_json::to_value(crate::security::filesystem::PathOperation::Write)
            .expect("operation serializes");
        assert_eq!(operation, serde_json::json!("write"));
    }

    #[test]
    fn exact_session_grant_beats_default_ask_but_not_explicit_ask_rule() {
        let mut ctx = context();
        ctx.grant_key = Some(ApprovalGrantKey {
            permission_kind: crate::human_interaction::ToolPermissionKind::Shell,
            tool_name: "shell".to_string(),
            recipient_or_server: None,
            arguments_digest: Some(
                "1eab1ef18bb109ae99f48bfe7efaba9dde08e27bdb46438214bd6c65eb3ff2ff".to_string(),
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
                ApprovalMatcher::ToolName {
                    name: "shell".into(),
                },
            )],
            additional_safety_floors: ApprovalSafetyFloors::default(),
            session_grants: ApprovalGrantSet::from_grants(vec![ApprovalGrant::Exact { key }]),
        };
        assert_eq!(ask_policy.evaluate(&ctx).action, ApprovalAction::Ask);
    }
}
