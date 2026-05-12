# Approval Policy Ruleset Design

## Context

Task: `tsq-1av9jz0z.1.1` - Define clean approval rule model and breaking API boundary.

Roci currently exposes `ApprovalPolicy::{Never, Ask, Always}` as a flat enum. That is too coarse for the security primitive epic because it cannot express command, filesystem, sandbox, MCP, network, or session-scoped approval decisions. Roci has no external SDK users yet, so breaking the public API is acceptable when it produces the right long-term SDK shape.

Prior art:
- Pi provides useful tool lifecycle hook boundaries, but not a deep security policy model.
- Codex provides the strongest orchestration precedent: presets compile into structured permission profiles, and approval/sandbox/exec decisions are centralized.
- Claude Code provides the strongest rule model precedent: explicit allow/ask/deny rules, safety floors, source-aware suggestions, and UI kept separate from core permission evaluation.

## Decision

Use a contract-first engine seam.

`ApprovalPolicy` remains the public concept name, but changes from enum to structured policy. Old enum variants are removed. Presets become constructors:

```rust
ApprovalPolicy::ask()
ApprovalPolicy::always()
ApprovalPolicy::never()
```

No adapter shim or dual enum/ruleset path should be added. The structured model is canonical.

## Public API Shape

```rust
pub struct ApprovalPolicy {
    pub default_action: ApprovalAction,
    pub rules: Vec<ApprovalRule>,
    pub additional_safety_floors: ApprovalSafetyFloors,
    pub session_grants: ApprovalGrantSet,
}

pub enum ApprovalAction {
    Allow,
    Ask,
    Deny,
}

pub struct ApprovalRule {
    pub id: String,
    pub source: ApprovalRuleSource,
    pub action: ApprovalAction,
    pub matcher: ApprovalMatcher,
    pub scope: ApprovalScope,
    pub reason: Option<String>,
}
```

`ApprovalPolicy::ask()` is the default. `ApprovalPolicy::always()` means broad normal allow, not bypass of hard safety floors. `ApprovalPolicy::never()` means default deny.

Built-in hard floors are evaluator-owned and always applied. `ApprovalPolicy::additional_safety_floors` can only add stricter host floors; an empty field must not disable built-in floors.

Minimal supporting shapes:

```rust
pub enum ApprovalRuleSource {
    BuiltIn,
    Host,
    Session,
    User,
}

pub enum ApprovalScope {
    Once,
    Session,
    PersistentHint,
}

pub enum ApprovalMatcher {
    All(Vec<ApprovalMatcher>),
    Any(Vec<ApprovalMatcher>),
    ToolName(String),
    ToolKind(ToolApprovalKind),
    CommandExecutable(String),
    CommandCategory(CommandCategory),
    CommandPattern(String),
    FilesystemPath { operation: PathOperation, path: PathBuf },
    FilesystemBoundary { operation: PathOperation, boundary: PathBoundary },
    McpServer(String),
    McpTool { server: Option<String>, tool: String },
    SandboxRequirement(String),
    SandboxResult(String),
    NetworkRequirement(String),
    Metadata { key: String, value: serde_json::Value },
}

pub enum ApprovalGrant {
    Exact(ApprovalGrantKey),
    Rule(ApprovalRule),
}

pub struct ApprovalGrantKey {
    pub permission_kind: ToolPermissionKind,
    pub tool_name: String,
    pub recipient_or_server: Option<String>,
    pub arguments_digest: Option<String>,
    pub tool_provided_key: Option<String>,
}
```

`ApprovalGrantKey` excludes tool call id and raw arguments. Argument canonicalization must sort JSON object keys before hashing so equivalent objects produce the same grant key without leaking argument content into approval events.

Existing UI-facing type names remain part of the boundary:
- `ApprovalHandler`
- `ApprovalRequest`
- `ApprovalDecision`

Schema changes are allowed. `ApprovalRequest` should expose a redacted preview, evaluation metadata, reason, and suggested grant. Approval events should not emit raw `payload.arguments` by default.

Host apps render prompts and persist policy. Core owns evaluation, precedence, and suggested grants.

## Matchers

`ApprovalMatcher` should support the full first security epic surface:
- tool name and tool kind
- command executable, command category, and command pattern
- filesystem operation, path, and boundary
- MCP server and MCP tool
- sandbox requirement or sandbox result
- network requirement
- custom metadata key/value

Typed matchers should be used where Roci already has concrete primitives. Metadata matcher exists for future host or provider extensions without another public API break.

## Evaluation Model

`ApprovalContext` represents finalized invocation facts:
- tool call id
- tool name
- tool approval metadata
- redacted argument preview
- command insight, when available
- filesystem access summary, when available
- sandbox requirement/result, when available
- MCP and network metadata, when available

`ApprovalEvaluation` returns:
- final `ApprovalAction`
- matched rules
- safety floor hits
- specificity/explanation metadata
- optional suggested `ApprovalGrant`

Evaluation precedence:

1. Hard safety floors.
2. Matching rules by action strength: `Deny > Ask > Allow`.
3. Same-action matches by specificity.
4. Session grants.
5. Tool metadata safety.
6. Policy default action.

Specificity order is exact invocation > exact tool/path/command > prefix or boundary > category/kind > metadata/default. If two matches have the same action and specificity, earlier rule order wins.

Session grants intentionally come after explicit rules so a temporary allow cannot override configured policy.

Session grants evaluate as `Allow` only. Exact session grants may beat default `Ask` or tool metadata `RequiresApproval`, but never beat hard floors or explicit `Deny`/`Ask` rules.

## Safety Floors

Hard floors are non-bypassable by default. Broad allow rules and `ApprovalPolicy::always()` cannot silently bypass:
- destructive command floors
- denied filesystem paths
- required sandbox unavailable
- future high-risk MCP/network floors

Hosts may choose stricter floors. The v1 API should not expose broad bypass as default behavior.

V1 built-in floor effects:
- destructive command floor returns `Ask`
- denied filesystem path returns `Deny`
- required sandbox unavailable returns `Deny`
- future MCP/network floors must declare `Ask` or `Deny` explicitly when added

## Session Grants

V1 wires exact session grants:
- same tool
- same permission kind
- same normalized arguments or grant key

The data model should also include a rule-shaped grant variant for future UX such as "allow `npm test` for this session." That variant may be reserved/not applied in this task, but should be shaped so future work does not require another API break.

## Runtime Integration

This task defines and wires the contract seam. It does not need to complete all downstream enforcement.

Required in this task:
- Replace enum construction with structured policy constructors.
- Add policy/evaluation types.
- Add evaluator function or trait that accepts `ApprovalContext`.
- Express current behavior through `ApprovalPolicy::ask()`, `always()`, and `never()`.
- Update compile call sites, examples, docs, and roci-cli mapping.

Important runtime invariant:
- Approval evaluation must happen after tool arguments are finalized.
- `pre_tool_use ReplaceArgs` must run before `ApprovalContext` construction.
- If this invariant cannot be fully fixed in `.1.1`, it must be called out in the follow-up for `.1.2` and covered by tests there.

## Approval Events

Approval event payloads should carry:
- redacted preview
- stable evaluation metadata
- reason/explanation
- suggested grant, when available

Raw tool args should not be added to approval/event payloads by default. Raw execution context remains internal unless host already receives it through another explicit path.

## roci-cli

Current CLI UX remains:

```text
--approval ask
--approval always
--approval never
```

CLI maps those values to constructors:

```rust
ApprovalPolicy::ask()
ApprovalPolicy::always()
ApprovalPolicy::never()
```

CLI continues to own terminal prompts and approval decisions. It does not own policy precedence.

## Tests

Required coverage:
- `ApprovalPolicy::ask()`, `always()`, and `never()` preserve old behavior.
- `Deny > Ask > Allow`.
- Specificity tie-break is deterministic.
- Hard safety floor beats broad allow.
- Hand-built broad allow with empty `additional_safety_floors` still cannot bypass built-in floors.
- Exact session grant allows only the matching call.
- Exact session grant beats default ask/tool metadata ask, but not explicit ask/deny or hard floors.
- Rule-shaped grant type is serialized and either reserved or explicitly not applied.
- Approval request preview redacts secrets while metadata explains the decision.
- Approval events do not include raw `payload.arguments` by default.
- CLI approval arg mapping produces expected policy presets.

## Acceptance Criteria

- `ApprovalPolicy` enum variants are no longer public construction path.
- Structured `ApprovalPolicy` is the only canonical policy model.
- Existing runner behavior passes through the new policy seam.
- Host/UI boundary remains unchanged: host renders, core evaluates.
- Docs explain that host owns approval UX/persistence and core owns evaluation/precedence.
- Follow-up integration task `.1.2` has clear responsibility for finalized-args approval ordering if not fully patched here.

## Non-Goals

- Persistent policy storage.
- Full interactive approval command in roci-cli.
- Complete MCP/network/sandbox enforcement.
- Concrete OS sandbox implementation.
- Perfect shell parsing.
- Long-lived compatibility shim for the old enum.
