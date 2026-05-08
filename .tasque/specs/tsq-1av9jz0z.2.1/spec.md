## Overview
Define the reusable command classification contract and normalized command inputs for SDK security primitives.

## Scope
- Add a `CommandClassifier` trait and default heuristic classifier contract in `roci-core`.
- Define normalized `CommandInput` facts: raw command, optional cwd, tool name, shell kind/platform, and environment hints where needed.
- Define `CommandInsight` output: normalized command text, primary executable, matched categories, reasons, and confidence.
- Ship default command categories: `ReadOnly`, `WritesFilesystem`, `DestructiveDelete`, `PrivilegeEscalation`, `PermissionChange`, `ProcessControl`, `NetworkLikely`, `CodeExecution`, `Unknown`.
- Treat classifier output as a safety floor/input to approval policy, not as a perfect security boundary.

## Decisions
- Follow Codex's safer posture: known-safe/whitelist style for silent allow; unknown or risky commands require policy/approval handling later.
- Keep Pi-style event extensibility for later host integration, but do not make extension hooks the only security boundary.
- No full shell parser in v1; normalize enough for deterministic categories and testable reasons.

## Normalization details
Minimum v1 normalization inspects leading env assignments, common wrappers (`sudo`, `doas`, `command`, `builtin`, `time`, `env`, `xargs`), and shell connectors (`;`, `&&`, `||`, `|`). Multi-command input returns union categories. If any segment is unknown, include `Unknown`; unknown/risky categories cannot be silently allowed by default policy.

## Constraints / Non-goals
- No approval engine implementation in this task.
- No OS sandboxing or command execution changes.
- No AI/LLM classifier.

## Interfaces (CLI/API)
```rust
pub trait CommandClassifier: Send + Sync {
    fn classify(&self, input: CommandInput) -> CommandInsight;
}

pub struct CommandInput {
    pub raw_command: String,
    pub cwd: Option<std::path::PathBuf>,
    pub tool_name: Option<String>,
    pub shell_kind: Option<ShellKind>,
    pub platform: Option<CommandPlatform>,
}

pub struct CommandInsight {
    pub normalized_command: String,
    pub primary_executable: Option<String>,
    pub categories: Vec<CommandCategory>,
    pub reasons: Vec<String>,
    pub confidence: CommandConfidence,
}
```

## Data model / schema changes
- Add `CommandInput`, `CommandInsight`, `CommandCategory`, `CommandConfidence`, and default classifier types to `roci-core`.
- Add wrapper/connector normalization helpers used by the default classifier.
- Keep raw command text on `CommandInput` for audit and preview surfaces.

## Acceptance criteria
1. The command classifier API lives in `roci-core` and is host/app independent.
2. Default classifier returns deterministic categories and reason strings for common read-only, write, destructive, privilege, process, network, code-exec, and unknown commands.
3. Normalization preserves raw command text while exposing structured facts for policy evaluation.
4. Tests prove unknown commands are not silently treated as safe.
5. Tests cover command sequences/wrappers enough to prevent obvious bypasses of the safety floor.

## Test plan
- Unit tests for category assignment and normalization.
- Regression fixtures for safe read-only commands, destructive deletes, privilege escalation, permission changes, network-like commands, code execution, and unknowns.
