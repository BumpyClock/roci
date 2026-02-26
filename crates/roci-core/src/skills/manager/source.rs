use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use uuid::Uuid;

use crate::error::RociError;
use crate::resource::settings::resolve_path;
use crate::skills::loader::{load_skills, LoadSkillsOptions};
use crate::skills::manager::{ManagedSkillSource, ManagedSkillSourceKind};
use crate::skills::model::Skill;

pub(crate) fn parse_source(input: &str, cwd: &Path) -> Result<ManagedSkillSource, RociError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(RociError::InvalidArgument(
            "Skill source must not be empty".to_string(),
        ));
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let local_candidate = resolve_path(trimmed, cwd, home.as_deref())?;
    if local_candidate.exists() {
        let canonical = fs::canonicalize(&local_candidate).unwrap_or(local_candidate);
        return Ok(ManagedSkillSource {
            kind: ManagedSkillSourceKind::LocalPath,
            value: canonical.to_string_lossy().into_owned(),
        });
    }

    if looks_like_git_url(trimmed) {
        return Ok(ManagedSkillSource {
            kind: ManagedSkillSourceKind::GitUrl,
            value: trimmed.to_string(),
        });
    }

    Err(RociError::InvalidArgument(format!(
        "Skill source '{}' is neither an existing local path nor a supported git URL",
        trimmed
    )))
}

fn looks_like_git_url(source: &str) -> bool {
    source.contains("://")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
}

pub(crate) fn materialize_source(
    source: &ManagedSkillSource,
) -> Result<MaterializedSource, RociError> {
    match source.kind {
        ManagedSkillSourceKind::LocalPath => {
            let path = PathBuf::from(&source.value);
            if !path.exists() {
                return Err(RociError::InvalidState(format!(
                    "Managed local source path '{}' does not exist",
                    path.display()
                )));
            }
            Ok(MaterializedSource {
                root: path,
                _temp: None,
            })
        }
        ManagedSkillSourceKind::GitUrl => {
            let temp = EphemeralDirectory::new("roci-skill-source")?;
            let clone_root = temp.path().join("source");
            run_git_clone(&source.value, &clone_root)?;
            Ok(MaterializedSource {
                root: clone_root,
                _temp: Some(temp),
            })
        }
    }
}

#[derive(Debug)]
pub(crate) struct MaterializedSource {
    root: PathBuf,
    _temp: Option<EphemeralDirectory>,
}

impl MaterializedSource {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

fn run_git_clone(url: &str, destination: &Path) -> Result<(), RociError> {
    let output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(url)
        .arg(destination)
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    Err(RociError::InvalidState(format!(
        "Git clone failed for '{}' with status {}: {}",
        url,
        output.status,
        if message.is_empty() {
            "no stderr output"
        } else {
            message
        }
    )))
}

pub(crate) fn discover_source_skills(source_root: &Path) -> Result<Vec<Skill>, RociError> {
    let result = load_skills(&LoadSkillsOptions {
        roots: Vec::new(),
        explicit_paths: vec![source_root.to_path_buf()],
        follow_symlinks: true,
    });

    if result.skills.is_empty() {
        return Err(RociError::InvalidState(format!(
            "No skills were found in source '{}'",
            source_root.display()
        )));
    }

    Ok(result.skills)
}

#[derive(Debug)]
struct EphemeralDirectory {
    path: PathBuf,
}

impl EphemeralDirectory {
    fn new(prefix: &str) -> Result<Self, RociError> {
        let base = std::env::temp_dir();
        for _ in 0..8 {
            let candidate = base.join(format!("{prefix}-{}", Uuid::new_v4()));
            match fs::create_dir(&candidate) {
                Ok(_) => return Ok(Self { path: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(RociError::Io(error)),
            }
        }

        Err(RociError::InvalidState(
            "Failed to create temporary directory for skill source cloning".to_string(),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for EphemeralDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
