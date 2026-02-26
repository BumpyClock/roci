use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::RociError;
use crate::skills::manager::ManagedSkillRecord;

pub(crate) const MANAGED_MANIFEST_FILE_NAME: &str = ".managed-skills.json";
const MANAGED_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ManagedSkillManifest {
    pub(crate) version: u32,
    pub(crate) skills: Vec<ManagedSkillRecord>,
}

impl Default for ManagedSkillManifest {
    fn default() -> Self {
        Self {
            version: MANAGED_MANIFEST_VERSION,
            skills: Vec::new(),
        }
    }
}

pub(crate) fn load_manifest(skill_root: &Path) -> Result<ManagedSkillManifest, RociError> {
    let path = skill_root.join(MANAGED_MANIFEST_FILE_NAME);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedSkillManifest::default());
        }
        Err(error) => return Err(RociError::Io(error)),
    };

    let mut manifest: ManagedSkillManifest = serde_json::from_str(&raw)?;
    if manifest.version != MANAGED_MANIFEST_VERSION {
        return Err(RociError::Configuration(format!(
            "Unsupported managed skill manifest version '{}' in '{}'",
            manifest.version,
            path.display()
        )));
    }

    let mut seen = HashSet::new();
    for record in &manifest.skills {
        if !seen.insert(record.name.clone()) {
            return Err(RociError::Configuration(format!(
                "Managed skill manifest contains duplicate skill '{}'",
                record.name
            )));
        }
    }

    sort_manifest(&mut manifest);
    Ok(manifest)
}

pub(crate) fn save_manifest(
    skill_root: &Path,
    manifest: &ManagedSkillManifest,
) -> Result<(), RociError> {
    fs::create_dir_all(skill_root)?;
    let mut manifest = manifest.clone();
    sort_manifest(&mut manifest);
    let serialized = serde_json::to_string_pretty(&manifest)?;
    fs::write(
        skill_root.join(MANAGED_MANIFEST_FILE_NAME),
        format!("{serialized}\n"),
    )?;
    Ok(())
}

pub(crate) fn upsert_manifest_record(
    manifest: &mut ManagedSkillManifest,
    record: &ManagedSkillRecord,
) {
    if let Some(existing) = manifest
        .skills
        .iter_mut()
        .find(|existing| existing.name == record.name)
    {
        *existing = record.clone();
        return;
    }
    manifest.skills.push(record.clone());
}

fn sort_manifest(manifest: &mut ManagedSkillManifest) {
    manifest
        .skills
        .sort_by(|left, right| left.name.cmp(&right.name));
}
