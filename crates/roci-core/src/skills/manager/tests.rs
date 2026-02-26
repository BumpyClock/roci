use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::tempdir;

use super::{ManagedSkillScope, ManagedSkillSourceKind, SkillManager, MANAGED_MANIFEST_FILE_NAME};
use crate::resource::ResourceDirectories;

fn write_skill(root: &Path, directory: &str, name: &str, description: &str, body: &str) -> PathBuf {
    let skill_dir = root.join(directory);
    fs::create_dir_all(&skill_dir).expect("skill directory should be created");
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .expect("SKILL.md should be written");
    fs::write(skill_dir.join("payload.txt"), body).expect("payload file should be written");
    skill_dir
}

fn test_directories(root: &Path) -> ResourceDirectories {
    ResourceDirectories {
        project_dir: root.join(".roci"),
        agent_dir: root.join("home/.roci/agent"),
    }
}

fn skill_root(root: &Path, scope: ManagedSkillScope) -> PathBuf {
    match scope {
        ManagedSkillScope::Project => root.join(".roci/skills"),
        ManagedSkillScope::Global => root.join("home/.roci/agent/skills"),
    }
}

#[test]
fn install_local_source_installs_single_skill_and_persists_manifest() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source directory should be created");
    write_skill(
        &source,
        "alpha",
        "alpha",
        "alpha description",
        "alpha-body-v1",
    );

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    let result = manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    assert_eq!(result.installed.len(), 1);
    assert_eq!(result.installed[0].name, "alpha");
    assert_eq!(
        result.installed[0].source.kind,
        ManagedSkillSourceKind::LocalPath
    );

    let installed_payload = skill_root(temp.path(), ManagedSkillScope::Project)
        .join("alpha")
        .join("payload.txt");
    assert_eq!(
        fs::read_to_string(installed_payload).expect("installed payload should be readable"),
        "alpha-body-v1"
    );

    let manifest_path =
        skill_root(temp.path(), ManagedSkillScope::Project).join(MANAGED_MANIFEST_FILE_NAME);
    let manifest = fs::read_to_string(manifest_path).expect("manifest should be readable");
    assert!(manifest.contains("\"name\": \"alpha\""));
}

#[test]
fn install_local_source_installs_multiple_skills_from_one_source() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source directory should be created");
    write_skill(
        &source,
        "first",
        "first-skill",
        "first description",
        "first-body",
    );
    write_skill(
        &source,
        "second",
        "second-skill",
        "second description",
        "second-body",
    );

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    let result = manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    assert_eq!(result.installed.len(), 2);
    assert!(result
        .installed
        .iter()
        .any(|entry| entry.name == "first-skill"));
    assert!(result
        .installed
        .iter()
        .any(|entry| entry.name == "second-skill"));

    let first_payload = skill_root(temp.path(), ManagedSkillScope::Project)
        .join("first-skill")
        .join("payload.txt");
    let second_payload = skill_root(temp.path(), ManagedSkillScope::Project)
        .join("second-skill")
        .join("payload.txt");
    assert_eq!(
        fs::read_to_string(first_payload).expect("first payload should be readable"),
        "first-body"
    );
    assert_eq!(
        fs::read_to_string(second_payload).expect("second payload should be readable"),
        "second-body"
    );
}

#[test]
fn install_git_source_uses_local_git_fixture() {
    let temp = tempdir().expect("temp dir should be created");
    let repo_root = temp.path().join("git-source");
    fs::create_dir_all(&repo_root).expect("repo root should be created");
    write_skill(
        &repo_root,
        "fixture",
        "git-fixture",
        "fixture description",
        "git-body-v1",
    );

    run_git(&repo_root, ["init"]);
    run_git(&repo_root, ["config", "user.email", "test@example.com"]);
    run_git(&repo_root, ["config", "user.name", "Test User"]);
    run_git(&repo_root, ["add", "."]);
    run_git(&repo_root, ["commit", "-m", "init"]);

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    let source = format!("file://{}", repo_root.to_string_lossy());
    let result = manager
        .install(temp.path(), ManagedSkillScope::Global, &source)
        .expect("git install should succeed");

    assert_eq!(result.installed.len(), 1);
    assert_eq!(result.installed[0].name, "git-fixture");
    assert_eq!(
        result.installed[0].source.kind,
        ManagedSkillSourceKind::GitUrl
    );
    assert_eq!(result.installed[0].source.value, source);

    let installed_payload = skill_root(temp.path(), ManagedSkillScope::Global)
        .join("git-fixture")
        .join("payload.txt");
    assert_eq!(
        fs::read_to_string(installed_payload).expect("installed payload should be readable"),
        "git-body-v1"
    );
}

#[test]
fn remove_deletes_skill_directory_and_manifest_entry() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source should be created");
    write_skill(
        &source,
        "skill",
        "remove-me",
        "remove description",
        "remove-body",
    );

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    let removed = manager
        .remove(temp.path(), ManagedSkillScope::Project, "remove-me")
        .expect("remove should succeed");
    assert!(removed.removed.is_some());
    assert!(!skill_root(temp.path(), ManagedSkillScope::Project)
        .join("remove-me")
        .exists());

    let manifest_path =
        skill_root(temp.path(), ManagedSkillScope::Project).join(MANAGED_MANIFEST_FILE_NAME);
    let manifest = fs::read_to_string(manifest_path).expect("manifest should be readable");
    assert!(!manifest.contains("remove-me"));
}

#[test]
fn update_one_only_resyncs_requested_skill() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source should be created");
    let alpha_dir = write_skill(&source, "alpha", "alpha", "alpha description", "alpha-v1");
    let beta_dir = write_skill(&source, "beta", "beta", "beta description", "beta-v1");

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    fs::write(alpha_dir.join("payload.txt"), "alpha-v2").expect("alpha payload should be updated");
    fs::write(beta_dir.join("payload.txt"), "beta-v2").expect("beta payload should be updated");

    let update = manager
        .update(temp.path(), ManagedSkillScope::Project, Some("alpha"))
        .expect("update should succeed");

    assert_eq!(update.updated.len(), 1);
    assert_eq!(update.updated[0].name, "alpha");

    let install_root = skill_root(temp.path(), ManagedSkillScope::Project);
    assert_eq!(
        fs::read_to_string(install_root.join("alpha/payload.txt"))
            .expect("alpha payload should be readable"),
        "alpha-v2"
    );
    assert_eq!(
        fs::read_to_string(install_root.join("beta/payload.txt"))
            .expect("beta payload should be readable"),
        "beta-v1"
    );
}

#[test]
fn update_all_resyncs_all_managed_skills() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source should be created");
    let alpha_dir = write_skill(&source, "alpha", "alpha", "alpha description", "alpha-v1");
    let beta_dir = write_skill(&source, "beta", "beta", "beta description", "beta-v1");

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    fs::write(alpha_dir.join("payload.txt"), "alpha-v2").expect("alpha payload should be updated");
    fs::write(beta_dir.join("payload.txt"), "beta-v2").expect("beta payload should be updated");

    let update = manager
        .update(temp.path(), ManagedSkillScope::Project, None)
        .expect("update should succeed");

    assert_eq!(update.updated.len(), 2);

    let install_root = skill_root(temp.path(), ManagedSkillScope::Project);
    assert_eq!(
        fs::read_to_string(install_root.join("alpha/payload.txt"))
            .expect("alpha payload should be readable"),
        "alpha-v2"
    );
    assert_eq!(
        fs::read_to_string(install_root.join("beta/payload.txt"))
            .expect("beta payload should be readable"),
        "beta-v2"
    );
}

#[test]
fn list_returns_managed_and_unmanaged_skills() {
    let temp = tempdir().expect("temp dir should be created");
    let source = temp.path().join("source");
    fs::create_dir_all(&source).expect("source should be created");
    write_skill(
        &source,
        "managed",
        "managed-skill",
        "managed description",
        "managed-body",
    );

    let unmanaged_root = temp.path().join(".agents/skills/unmanaged");
    fs::create_dir_all(&unmanaged_root).expect("unmanaged root should be created");
    fs::write(
        unmanaged_root.join("SKILL.md"),
        "---\nname: unmanaged-skill\ndescription: unmanaged description\n---\n",
    )
    .expect("unmanaged SKILL.md should be written");

    let manager = SkillManager::new().with_directories(test_directories(temp.path()));
    manager
        .install(
            temp.path(),
            ManagedSkillScope::Project,
            &source.to_string_lossy(),
        )
        .expect("install should succeed");

    let list = manager.list(temp.path()).expect("list should succeed");

    let managed_discovered = list
        .discovered
        .iter()
        .find(|item| item.skill.name == "managed-skill")
        .expect("managed skill should be discovered");
    assert!(managed_discovered.managed.is_some());
    assert_eq!(
        managed_discovered
            .managed
            .as_ref()
            .expect("managed metadata should exist")
            .source
            .kind,
        ManagedSkillSourceKind::LocalPath
    );

    let unmanaged_discovered = list
        .discovered
        .iter()
        .find(|item| item.skill.name == "unmanaged-skill")
        .expect("unmanaged skill should be discovered");
    assert!(unmanaged_discovered.managed.is_none());

    assert!(list
        .managed
        .iter()
        .any(|entry| entry.record.name == "managed-skill"));
}

fn run_git<I, S>(cwd: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_string())
        .collect();
    let output = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .output()
        .expect("git command should execute");

    assert!(
        output.status.success(),
        "git command failed: git {} stderr={}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}
