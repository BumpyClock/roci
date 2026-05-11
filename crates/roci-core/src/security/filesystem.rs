use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathResolutionMode {
    Lexical,
    CanonicalizeExisting,
    CanonicalizeBestEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    DenySymlinks,
    FollowIfTargetAllowed,
    AllowLexical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBoundary {
    pub root: PathBuf,
    pub glob: Option<String>,
}

impl PathBoundary {
    pub fn root(root: PathBuf) -> Self {
        Self { root, glob: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOperation {
    Read,
    Write,
    Create,
    Delete,
    List,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAccessRequest {
    pub operation: PathOperation,
    pub path: PathBuf,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemDecision {
    pub allowed: bool,
    pub normalized_path: Option<PathBuf>,
    pub reason: String,
    pub matched_boundary: Option<PathBoundary>,
}

pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBoundary>,
    pub writable_roots: Vec<PathBoundary>,
    pub denied: Vec<PathBoundary>,
    pub resolution_mode: PathResolutionMode,
    pub symlink_policy: SymlinkPolicy,
}

impl FilesystemPolicy {
    pub fn permissive() -> Self {
        Self {
            readable_roots: Vec::new(),
            writable_roots: Vec::new(),
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::Lexical,
            symlink_policy: SymlinkPolicy::AllowLexical,
        }
    }

    pub fn evaluate(&self, request: PathAccessRequest) -> FilesystemDecision {
        let has_restrictions = self.has_restrictions();
        let normalized = self.normalize_request_path(&request);

        if !has_restrictions {
            return FilesystemDecision {
                allowed: true,
                normalized_path: normalized.ok().or(Some(request.path)),
                reason: "allowed: no filesystem restrictions configured".to_string(),
                matched_boundary: None,
            };
        }

        let normalized_path = match normalized {
            Ok(path) => path,
            Err(reason) => {
                return FilesystemDecision {
                    allowed: false,
                    normalized_path: None,
                    reason,
                    matched_boundary: None,
                };
            }
        };
        let original_path = absolute_path(&request.path, request.cwd.as_deref()).ok();

        for boundary in &self.denied {
            match self.normalize_boundary(boundary) {
                Ok(boundary) if boundary.matches(&normalized_path) => {
                    return FilesystemDecision {
                        allowed: false,
                        normalized_path: Some(normalized_path),
                        reason: "denied: path matched denied boundary".to_string(),
                        matched_boundary: Some(boundary),
                    };
                }
                Err(reason) => {
                    return FilesystemDecision {
                        allowed: false,
                        normalized_path: Some(normalized_path),
                        reason: format!("denied: invalid denied boundary: {reason}"),
                        matched_boundary: Some(boundary.clone()),
                    };
                }
                _ => {}
            }
        }

        for boundary in self.allowed_boundaries(request.operation) {
            match self.normalize_boundary(boundary) {
                Ok(normalized_boundary) if normalized_boundary.matches(&normalized_path) => {
                    if matches!(self.symlink_policy, SymlinkPolicy::DenySymlinks) {
                        match original_path.as_deref() {
                            Some(path) => {
                                match contains_symlink_below_boundary(path, &boundary.root) {
                                    Ok(true) => {
                                        return FilesystemDecision {
                                            allowed: false,
                                            normalized_path: Some(normalized_path),
                                            reason: "denied: path contains symlink component"
                                                .to_string(),
                                            matched_boundary: Some(normalized_boundary),
                                        };
                                    }
                                    Ok(false) => {}
                                    Err(error) => {
                                        return FilesystemDecision {
                                            allowed: false,
                                            normalized_path: Some(normalized_path),
                                            reason: format!(
                                            "denied: failed to inspect symlink components: {error}"
                                        ),
                                            matched_boundary: Some(normalized_boundary),
                                        };
                                    }
                                }
                            }
                            None => {
                                return FilesystemDecision {
                                    allowed: false,
                                    normalized_path: Some(normalized_path),
                                    reason: "denied: failed to resolve original path".to_string(),
                                    matched_boundary: Some(normalized_boundary),
                                };
                            }
                        }
                    }

                    return FilesystemDecision {
                        allowed: true,
                        normalized_path: Some(normalized_path),
                        reason: "allowed: path matched operation boundary".to_string(),
                        matched_boundary: Some(normalized_boundary),
                    };
                }
                _ => {}
            }
        }

        FilesystemDecision {
            allowed: false,
            normalized_path: Some(normalized_path),
            reason: "denied: no operation boundary matched path".to_string(),
            matched_boundary: None,
        }
    }

    fn has_restrictions(&self) -> bool {
        !(self.readable_roots.is_empty()
            && self.writable_roots.is_empty()
            && self.denied.is_empty())
    }

    fn allowed_boundaries(&self, operation: PathOperation) -> &[PathBoundary] {
        match operation {
            PathOperation::Read | PathOperation::List | PathOperation::Search => {
                &self.readable_roots
            }
            PathOperation::Write | PathOperation::Create | PathOperation::Delete => {
                &self.writable_roots
            }
        }
    }

    fn normalize_request_path(&self, request: &PathAccessRequest) -> Result<PathBuf, String> {
        let absolute = absolute_path(&request.path, request.cwd.as_deref())?;

        match (self.resolution_mode, self.symlink_policy) {
            (PathResolutionMode::Lexical, SymlinkPolicy::FollowIfTargetAllowed) => {
                canonicalize_best_effort(&absolute)
            }
            (PathResolutionMode::Lexical, _) => Ok(lexical_normalize(&absolute)),
            (PathResolutionMode::CanonicalizeExisting, _) => absolute
                .canonicalize()
                .map_err(|error| format!("denied: failed to canonicalize existing path: {error}")),
            (PathResolutionMode::CanonicalizeBestEffort, _) => canonicalize_best_effort(&absolute),
        }
    }

    fn normalize_boundary(&self, boundary: &PathBoundary) -> Result<PathBoundary, String> {
        if !boundary.root.is_absolute() {
            return Err("boundary root must be absolute".to_string());
        }

        let root = match self.resolution_mode {
            PathResolutionMode::Lexical => {
                if matches!(self.symlink_policy, SymlinkPolicy::FollowIfTargetAllowed) {
                    canonicalize_best_effort(&boundary.root)?
                } else {
                    lexical_normalize(&boundary.root)
                }
            }
            PathResolutionMode::CanonicalizeExisting => boundary
                .root
                .canonicalize()
                .map_err(|error| format!("failed to canonicalize boundary root: {error}"))?,
            PathResolutionMode::CanonicalizeBestEffort => canonicalize_best_effort(&boundary.root)?,
        };

        Ok(PathBoundary {
            root,
            glob: boundary.glob.clone(),
        })
    }
}

impl PathBoundary {
    fn matches(&self, path: &Path) -> bool {
        if !(path == self.root || path.starts_with(&self.root)) {
            return false;
        }

        match &self.glob {
            Some(glob) => {
                let relative = path
                    .strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/");
                glob_matches(glob, &relative)
            }
            None => true,
        }
    }
}

fn absolute_path(path: &Path, cwd: Option<&Path>) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    cwd.map(|cwd| cwd.join(path))
        .ok_or_else(|| "denied: relative path requires cwd when restrictions exist".to_string())
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}

fn canonicalize_best_effort(path: &Path) -> Result<PathBuf, String> {
    let lexical = lexical_normalize(path);
    let mut existing = lexical.clone();
    let mut missing = Vec::new();

    while !existing.exists() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            return Err("denied: no existing path component found".to_string());
        };
        missing.push(name);
        if !existing.pop() {
            return Err("denied: no existing path component found".to_string());
        }
    }

    let mut normalized = existing
        .canonicalize()
        .map_err(|error| format!("denied: failed to canonicalize existing parent: {error}"))?;

    for part in missing.iter().rev() {
        normalized.push(part);
    }

    Ok(normalized)
}

fn contains_symlink_below_boundary(path: &Path, boundary_root: &Path) -> std::io::Result<bool> {
    let normalized_path = lexical_normalize(path);
    let normalized_root = lexical_normalize(boundary_root);
    let relative = normalized_path
        .strip_prefix(&normalized_root)
        .unwrap_or(&normalized_path);
    let mut current = normalized_root;

    for component in relative.components() {
        if matches!(component, Component::CurDir) {
            continue;
        }
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }

    Ok(false)
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut dp = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;

    for pattern_index in 1..=pattern.len() {
        if pattern[pattern_index - 1] == b'*' {
            dp[pattern_index][0] = dp[pattern_index - 1][0];
        }
    }

    for pattern_index in 1..=pattern.len() {
        for value_index in 1..=value.len() {
            dp[pattern_index][value_index] = match pattern[pattern_index - 1] {
                b'*' => dp[pattern_index - 1][value_index] || dp[pattern_index][value_index - 1],
                b'?' => dp[pattern_index - 1][value_index - 1],
                byte => dp[pattern_index - 1][value_index - 1] && byte == value[value_index - 1],
            };
        }
    }

    dp[pattern.len()][value.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn policy(readable: Vec<PathBoundary>, writable: Vec<PathBoundary>) -> FilesystemPolicy {
        FilesystemPolicy {
            readable_roots: readable,
            writable_roots: writable,
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::Lexical,
            symlink_policy: SymlinkPolicy::DenySymlinks,
        }
    }

    fn access(
        policy: &FilesystemPolicy,
        operation: PathOperation,
        path: impl Into<PathBuf>,
        cwd: Option<PathBuf>,
    ) -> FilesystemDecision {
        policy.evaluate(PathAccessRequest {
            operation,
            path: path.into(),
            cwd,
        })
    }

    #[test]
    fn lexical_mode_blocks_parent_escape() {
        let policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );

        assert!(access(&policy, PathOperation::Read, "/workspace/src/lib.rs", None).allowed);
        assert!(
            !access(
                &policy,
                PathOperation::Read,
                "/workspace/../etc/passwd",
                None
            )
            .allowed
        );
    }

    #[test]
    fn best_effort_allows_missing_child_inside_existing_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let policy = FilesystemPolicy {
            readable_roots: vec![PathBoundary::root(temp.path().to_path_buf())],
            writable_roots: Vec::new(),
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::CanonicalizeBestEffort,
            symlink_policy: SymlinkPolicy::DenySymlinks,
        };

        assert!(
            access(
                &policy,
                PathOperation::Read,
                temp.path().join("missing/file.txt"),
                None
            )
            .allowed
        );
    }

    #[test]
    fn cwd_relative_paths_are_normalized_and_traversal_is_blocked() {
        let policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );

        assert!(
            access(
                &policy,
                PathOperation::Read,
                "src/lib.rs",
                Some(PathBuf::from("/workspace"))
            )
            .allowed
        );
        assert!(
            !access(
                &policy,
                PathOperation::Read,
                "../etc/passwd",
                Some(PathBuf::from("/workspace"))
            )
            .allowed
        );
    }

    #[test]
    fn restricted_relative_path_without_cwd_is_denied() {
        let policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );

        let decision = access(&policy, PathOperation::Read, "src/lib.rs", None);

        assert!(!decision.allowed);
        assert_eq!(decision.normalized_path, None);
        assert!(decision.reason.contains("relative path requires cwd"));
    }

    #[test]
    fn denied_boundary_wins_before_allow_boundary() {
        let mut policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );
        policy.denied = vec![PathBoundary::root(PathBuf::from("/workspace/secrets"))];

        let decision = access(
            &policy,
            PathOperation::Read,
            "/workspace/secrets/key.txt",
            None,
        );

        assert!(!decision.allowed);
        assert_eq!(decision.matched_boundary, policy.denied.first().cloned());
    }

    #[test]
    fn denied_glob_wins_before_allow_boundary() {
        let mut policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );
        policy.denied = vec![PathBoundary {
            root: PathBuf::from("/workspace"),
            glob: Some("secrets/*".to_string()),
        }];

        let decision = access(
            &policy,
            PathOperation::Read,
            "/workspace/secrets/key.txt",
            None,
        );

        assert!(!decision.allowed);
        assert_eq!(decision.matched_boundary, policy.denied.first().cloned());
    }

    #[test]
    fn invalid_denied_boundary_fails_closed_before_allow_boundary() {
        let mut policy = policy(
            vec![PathBoundary::root(PathBuf::from("/workspace"))],
            Vec::new(),
        );
        policy.denied = vec![PathBoundary::root(PathBuf::from("relative-deny"))];

        let decision = access(&policy, PathOperation::Read, "/workspace/file.txt", None);

        assert!(!decision.allowed);
        assert_eq!(decision.matched_boundary, policy.denied.first().cloned());
        assert!(decision.reason.contains("invalid denied boundary"));
    }

    #[test]
    fn read_and_write_roots_are_operation_specific() {
        let policy = policy(
            vec![PathBoundary::root(PathBuf::from("/read"))],
            vec![PathBoundary::root(PathBuf::from("/write"))],
        );

        assert!(access(&policy, PathOperation::Read, "/read/file", None).allowed);
        assert!(!access(&policy, PathOperation::Write, "/read/file", None).allowed);
        assert!(access(&policy, PathOperation::Write, "/write/file", None).allowed);
    }

    #[test]
    fn no_restrictions_is_permissive() {
        assert!(
            access(
                &FilesystemPolicy::permissive(),
                PathOperation::Delete,
                "../anything",
                None
            )
            .allowed
        );
    }

    #[cfg(unix)]
    #[test]
    fn deny_symlinks_rejects_existing_symlink_component() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::create_dir(&target).expect("target dir");
        symlink(&target, &link).expect("symlink");

        let policy = FilesystemPolicy {
            readable_roots: vec![PathBoundary::root(temp.path().to_path_buf())],
            writable_roots: Vec::new(),
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::CanonicalizeBestEffort,
            symlink_policy: SymlinkPolicy::DenySymlinks,
        };

        let decision = access(&policy, PathOperation::Read, link.join("file.txt"), None);

        assert!(!decision.allowed);
        assert!(decision.reason.contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn follow_symlink_allows_target_inside_allowed_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::create_dir(&target).expect("target dir");
        symlink(&target, &link).expect("symlink");

        let policy = FilesystemPolicy {
            readable_roots: vec![PathBoundary::root(temp.path().to_path_buf())],
            writable_roots: Vec::new(),
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::CanonicalizeBestEffort,
            symlink_policy: SymlinkPolicy::FollowIfTargetAllowed,
        };

        assert!(access(&policy, PathOperation::Read, link.join("file.txt"), None).allowed);
    }

    #[cfg(unix)]
    #[test]
    fn allow_lexical_allows_symlink_without_target_check() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let link = temp.path().join("link");
        symlink(outside.path(), &link).expect("symlink");

        let policy = FilesystemPolicy {
            readable_roots: vec![PathBoundary::root(temp.path().to_path_buf())],
            writable_roots: Vec::new(),
            denied: Vec::new(),
            resolution_mode: PathResolutionMode::Lexical,
            symlink_policy: SymlinkPolicy::AllowLexical,
        };

        assert!(access(&policy, PathOperation::Read, link.join("file.txt"), None).allowed);
    }
}
