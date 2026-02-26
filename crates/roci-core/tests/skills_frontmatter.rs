use roci_core::skills::{
    load_skills, LoadSkillsOptions, SkillDiagnosticLevel, SkillRoot, SkillSource,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_skill(root: &Path, folder: &str, frontmatter: &str) -> PathBuf {
    let skill_dir = root.join(folder);
    fs::create_dir_all(&skill_dir).expect("skill directory should be created");
    let file_path = skill_dir.join("SKILL.md");
    fs::write(&file_path, frontmatter).expect("skill file should be written");
    file_path
}

fn load_from_root(root: &Path) -> roci_core::skills::LoadSkillsResult {
    let options = LoadSkillsOptions {
        roots: vec![SkillRoot {
            path: root.to_path_buf(),
            source: SkillSource::ProjectRoci,
        }],
        explicit_paths: Vec::new(),
        follow_symlinks: false,
    };
    load_skills(&options)
}

#[test]
fn a_skill_with_valid_frontmatter_loads() {
    let dir = tempdir().expect("temp dir should be created");
    let frontmatter = "---\nname: valid-skill\ndescription: This skill is valid\ndisable-model-invocation: true\n---\n";
    write_skill(dir.path(), "valid-skill", frontmatter);

    let result = load_from_root(dir.path());

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "valid-skill");
    assert_eq!(result.skills[0].description, "This skill is valid");
    assert!(result.skills[0].disable_model_invocation);
}

#[test]
fn a_skill_without_a_description_is_skipped_and_reports_a_warning() {
    let dir = tempdir().expect("temp dir should be created");
    let frontmatter = "---\nname: no-description\n---\n";
    write_skill(dir.path(), "no-description", frontmatter);

    let result = load_from_root(dir.path());

    assert!(result.skills.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, SkillDiagnosticLevel::Warning);
    assert!(result.diagnostics[0].message.contains("description"));
}

#[test]
fn a_skill_with_invalid_yaml_is_skipped_and_reports_a_warning() {
    let dir = tempdir().expect("temp dir should be created");
    let frontmatter = "---\nname: [bad\ndescription: malformed\n---\n";
    write_skill(dir.path(), "bad-yaml", frontmatter);

    let result = load_from_root(dir.path());

    assert!(result.skills.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, SkillDiagnosticLevel::Warning);
}

#[test]
fn a_skill_with_a_name_mismatch_reports_a_warning_but_still_loads() {
    let dir = tempdir().expect("temp dir should be created");
    let frontmatter = "---\nname: child-name\ndescription: This description is okay\n---\n";
    write_skill(dir.path(), "parent-name", frontmatter);

    let result = load_from_root(dir.path());

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "child-name");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, SkillDiagnosticLevel::Warning);
    assert!(result.diagnostics[0].message.contains("does not match"));
}

#[test]
fn a_skill_with_an_invalid_name_is_skipped_and_reports_a_warning() {
    let dir = tempdir().expect("temp dir should be created");
    let frontmatter = "---\nname: Bad_Name\ndescription: This is okay\n---\n";
    write_skill(dir.path(), "bad-name", frontmatter);

    let result = load_from_root(dir.path());

    assert!(result.skills.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].level, SkillDiagnosticLevel::Warning);
    assert!(result.diagnostics[0].message.contains("invalid"));
}
