## Overview
Define the reusable filesystem access model for path normalization and allow/deny decisions before built-in tools touch disk.

## Scope
- Add `FilesystemPolicy` pure evaluation type in `roci-core`.
- Define readable roots, writable roots, denied paths/globs, path resolution mode, and symlink policy.
- Define `PathAccessRequest` with operation (`Read`, `Write`, `Create`, `Delete`, `List`, `Search`), path, and cwd.
- Define `FilesystemDecision` with allow/deny, matched rule/boundary, normalized path facts, and reason.

## Decisions
- Follow Codex's path-policy posture: normalize before evaluating, handle symlinks explicitly, and make denied paths win before allow roots.
- Default policy remains permissive until a host configures restrictions.
- This type is a pure policy evaluator only; concrete OS sandboxing remains out of scope.

## Path and symlink semantics
Path resolution modes: `Lexical`, `CanonicalizeExisting`, `CanonicalizeBestEffort`.

Symlink policy variants: `DenySymlinks`, `FollowIfTargetAllowed`, `AllowLexical`.

Decision precedence: invalid/unsupported normalization -> deny when restrictions exist; denied paths/globs; operation-specific allow root; permissive default when no restrictions are configured. Denied rules always win.

## Constraints / Non-goals
- No file I/O in the evaluator except path metadata needed for normalization when available.
- No platform sandbox implementation.
- No approval engine or built-in tool wiring in this task.

## Interfaces (CLI/API)
```rust
pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBoundary>,
    pub writable_roots: Vec<PathBoundary>,
    pub denied: Vec<PathBoundary>,
    pub resolution_mode: PathResolutionMode,
    pub symlink_policy: SymlinkPolicy,
}

pub struct PathAccessRequest {
    pub operation: PathOperation,
    pub path: std::path::PathBuf,
    pub cwd: Option<std::path::PathBuf>,
}

pub struct FilesystemDecision {
    pub allowed: bool,
    pub normalized_path: Option<std::path::PathBuf>,
    pub reason: String,
    pub matched_boundary: Option<PathBoundary>,
}
```

## Data model / schema changes
- Add `FilesystemPolicy`, `PathAccessRequest`, `FilesystemDecision`, path operation, resolution mode, symlink policy, and path boundary types to `roci-core`.
- Add normalization result facts that approval policy and built-in tools can consume later.
- Keep concrete sandbox execution out of the policy type.

## Acceptance criteria
1. Filesystem policy API lives in `roci-core` and can be used by shell/file tools later.
2. Decisions are deterministic for absolute paths, cwd-relative paths, denied paths, read roots, write roots, and unsupported/unknown normalization cases.
3. Deny rules take precedence over allow rules.
4. Symlink behavior is explicit and test-covered.
5. Tests cover path traversal, cwd-relative normalization, denied metadata paths, and permissive-default behavior.

## Test plan
- Unit tests for path normalization and decision precedence.
- Fixture tests for symlink policy variants where platform support is available.
- Regression tests showing policy evaluation does not mutate or touch files beyond normalization metadata.
