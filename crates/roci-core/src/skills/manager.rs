use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::RociError;
use crate::resource::settings::ResolvedResourceDirectories;
use crate::resource::ResourceDirectories;
use crate::skills::loader::{default_skill_roots, load_skills, LoadSkillsOptions};
use crate::skills::model::{Skill, SkillSource};
use crate::skills::SkillDiagnostic;

mod filesystem;
mod manifest;
mod source;

use filesystem::{copy_directory_recursive, remove_path};
use manifest::{load_manifest, save_manifest, upsert_manifest_record};
use source::{discover_source_skills, materialize_source, parse_source};

#[cfg(test)]
pub(crate) use manifest::MANAGED_MANIFEST_FILE_NAME;

/// Defines which install root should be used for managed skill operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSkillScope {
    Project,
    Global,
}

impl ManagedSkillScope {
    fn skill_root(self, directories: &ResolvedResourceDirectories) -> PathBuf {
        match self {
            Self::Project => directories.project_dir.join("skills"),
            Self::Global => directories.agent_dir.join("skills"),
        }
    }
}

/// Describes the origin of a managed skill installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedSkillSourceKind {
    LocalPath,
    GitUrl,
}

/// Source metadata persisted to the managed skill manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSkillSource {
    pub kind: ManagedSkillSourceKind,
    pub value: String,
}

/// Persistent manifest entry for one managed skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedSkillRecord {
    pub name: String,
    pub directory: String,
    pub source: ManagedSkillSource,
}

/// Result for a managed skill install operation.
#[derive(Debug, Clone, Default)]
pub struct InstallManagedSkillsResult {
    pub installed: Vec<ManagedSkillRecord>,
}

/// Result for a managed skill remove operation.
#[derive(Debug, Clone, Default)]
pub struct RemoveManagedSkillResult {
    pub removed: Option<ManagedSkillRecord>,
}

/// Result for a managed skill update operation.
#[derive(Debug, Clone, Default)]
pub struct UpdateManagedSkillsResult {
    pub updated: Vec<ManagedSkillRecord>,
}

/// Discovered skill metadata with optional managed-skill link.
#[derive(Debug, Clone)]
pub struct DiscoveredSkillListItem {
    pub skill: Skill,
    pub managed: Option<ManagedSkillRecord>,
}

/// Managed manifest metadata with current filesystem status.
#[derive(Debug, Clone)]
pub struct ManagedSkillListItem {
    pub scope: ManagedSkillScope,
    pub record: ManagedSkillRecord,
    pub install_path: PathBuf,
    pub exists_on_disk: bool,
}

/// Combined output for listing discovered and managed skill states.
#[derive(Debug, Clone, Default)]
pub struct ListManagedSkillsResult {
    pub discovered: Vec<DiscoveredSkillListItem>,
    pub managed: Vec<ManagedSkillListItem>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Filesystem manager for installing and synchronizing managed skill packages.
#[derive(Debug, Clone)]
pub struct SkillManager {
    directories: ResourceDirectories,
}

impl Default for SkillManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillManager {
    pub fn new() -> Self {
        Self {
            directories: ResourceDirectories::default(),
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.directories = directories;
        self
    }

    pub fn install(
        &self,
        cwd: &Path,
        scope: ManagedSkillScope,
        source: &str,
    ) -> Result<InstallManagedSkillsResult, RociError> {
        let resolved = self.directories.resolve(cwd)?;
        let skill_root = scope.skill_root(&resolved);
        let source = parse_source(source, cwd)?;
        let materialized = materialize_source(&source)?;
        let source_skills = discover_source_skills(materialized.root())?;

        let mut manifest = load_manifest(&skill_root)?;
        fs::create_dir_all(&skill_root)?;

        let mut installed = Vec::with_capacity(source_skills.len());
        for skill in source_skills {
            let directory = skill.name.clone();
            let target_dir = skill_root.join(&directory);
            let already_managed = manifest
                .skills
                .iter()
                .any(|record| record.name == skill.name);
            if target_dir.exists() && !already_managed {
                return Err(RociError::InvalidState(format!(
                    "Refusing to overwrite unmanaged skill directory '{}'",
                    target_dir.display()
                )));
            }

            if target_dir.exists() {
                remove_path(&target_dir)?;
            }
            copy_directory_recursive(&skill.base_dir, &target_dir)?;

            let record = ManagedSkillRecord {
                name: skill.name,
                directory,
                source: source.clone(),
            };
            upsert_manifest_record(&mut manifest, &record);
            installed.push(record);
        }

        save_manifest(&skill_root, &manifest)?;
        Ok(InstallManagedSkillsResult { installed })
    }

    pub fn remove(
        &self,
        cwd: &Path,
        scope: ManagedSkillScope,
        name: &str,
    ) -> Result<RemoveManagedSkillResult, RociError> {
        let resolved = self.directories.resolve(cwd)?;
        let skill_root = scope.skill_root(&resolved);
        let mut manifest = load_manifest(&skill_root)?;

        let position = manifest
            .skills
            .iter()
            .position(|record| record.name == name);
        let Some(position) = position else {
            return Ok(RemoveManagedSkillResult { removed: None });
        };

        let removed = manifest.skills.remove(position);
        remove_path(&skill_root.join(&removed.directory))?;
        save_manifest(&skill_root, &manifest)?;
        Ok(RemoveManagedSkillResult {
            removed: Some(removed),
        })
    }

    pub fn update(
        &self,
        cwd: &Path,
        scope: ManagedSkillScope,
        name: Option<&str>,
    ) -> Result<UpdateManagedSkillsResult, RociError> {
        let resolved = self.directories.resolve(cwd)?;
        let skill_root = scope.skill_root(&resolved);
        let manifest = load_manifest(&skill_root)?;

        let records = match name {
            Some(name) => {
                let Some(record) = manifest.skills.iter().find(|record| record.name == name) else {
                    return Err(RociError::InvalidArgument(format!(
                        "Managed skill '{name}' was not found in {scope:?} scope"
                    )));
                };
                vec![record.clone()]
            }
            None => manifest.skills.clone(),
        };

        let mut updated = Vec::with_capacity(records.len());
        for record in records {
            let materialized = materialize_source(&record.source)?;
            let source_skills = discover_source_skills(materialized.root())?;
            let Some(source_skill) = source_skills
                .into_iter()
                .find(|skill| skill.name == record.name)
            else {
                return Err(RociError::InvalidState(format!(
                    "Skill '{}' was not found in its update source '{}'",
                    record.name, record.source.value
                )));
            };

            let target_dir = skill_root.join(&record.directory);
            if target_dir.exists() {
                remove_path(&target_dir)?;
            }
            copy_directory_recursive(&source_skill.base_dir, &target_dir)?;
            updated.push(record);
        }

        Ok(UpdateManagedSkillsResult { updated })
    }

    pub fn list(&self, cwd: &Path) -> Result<ListManagedSkillsResult, RociError> {
        let resolved = self.directories.resolve(cwd)?;
        let project_root = ManagedSkillScope::Project.skill_root(&resolved);
        let global_root = ManagedSkillScope::Global.skill_root(&resolved);

        let project_manifest = load_manifest(&project_root)?;
        let global_manifest = load_manifest(&global_root)?;

        let mut managed_lookup = HashMap::<(ManagedSkillScope, String), ManagedSkillRecord>::new();
        let mut managed =
            Vec::with_capacity(project_manifest.skills.len() + global_manifest.skills.len());

        for (scope, root, manifest) in [
            (ManagedSkillScope::Project, &project_root, project_manifest),
            (ManagedSkillScope::Global, &global_root, global_manifest),
        ] {
            for record in manifest.skills {
                let install_path = root.join(&record.directory);
                managed_lookup.insert((scope, record.name.clone()), record.clone());
                managed.push(ManagedSkillListItem {
                    scope,
                    record,
                    exists_on_disk: install_path.exists(),
                    install_path,
                });
            }
        }

        managed.sort_by(|left, right| {
            left.scope
                .cmp(&right.scope)
                .then_with(|| left.record.name.cmp(&right.record.name))
        });

        let load_result = load_skills(&LoadSkillsOptions {
            roots: default_skill_roots(&resolved),
            explicit_paths: Vec::new(),
            follow_symlinks: true,
        });

        let mut discovered = Vec::with_capacity(load_result.skills.len());
        for skill in load_result.skills {
            let managed_record = managed_key_for_skill(skill.source, &skill.name)
                .and_then(|key| managed_lookup.get(&key).cloned());
            discovered.push(DiscoveredSkillListItem {
                skill,
                managed: managed_record,
            });
        }

        Ok(ListManagedSkillsResult {
            discovered,
            managed,
            diagnostics: load_result.diagnostics,
        })
    }
}

fn managed_key_for_skill(source: SkillSource, name: &str) -> Option<(ManagedSkillScope, String)> {
    match source {
        SkillSource::ProjectRoci => Some((ManagedSkillScope::Project, name.to_string())),
        SkillSource::GlobalRoci => Some((ManagedSkillScope::Global, name.to_string())),
        SkillSource::Explicit | SkillSource::ProjectAgents | SkillSource::GlobalAgents => None,
    }
}

#[cfg(test)]
mod tests;
