use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::resource::settings::ResolvedResourceDirectories;
use crate::skills::diagnostics::{SkillCollision, SkillDiagnostic, SkillDiagnosticLevel};
use crate::skills::frontmatter::parse_skill_file;
use crate::skills::model::{Skill, SkillSource};

const SKILL_FILE_NAME: &str = "SKILL.md";

/// Build default skill roots using resolved resource directories.
///
/// Order (highest to lower precedence during loading):
/// 1) project `project_dir/skills`
/// 2) project `.agents/skills` (sibling of `project_dir`)
/// 3) global `agent_dir/skills`
/// 4) global `.agents/skills` (derived from `agent_dir`)
pub fn default_skill_roots(directories: &ResolvedResourceDirectories) -> Vec<SkillRoot> {
    let roots = vec![
        SkillRoot {
            path: directories.project_dir.join("skills"),
            source: SkillSource::ProjectRoci,
        },
        SkillRoot {
            path: project_agents_root(&directories.project_dir).join("skills"),
            source: SkillSource::ProjectAgents,
        },
        SkillRoot {
            path: directories.agent_dir.join("skills"),
            source: SkillSource::GlobalRoci,
        },
        SkillRoot {
            path: global_agents_root(&directories.agent_dir).join("skills"),
            source: SkillSource::GlobalAgents,
        },
    ];

    roots
}

fn project_agents_root(project_dir: &Path) -> PathBuf {
    project_dir
        .parent()
        .unwrap_or(project_dir)
        .join(".agents")
}

fn global_agents_root(agent_dir: &Path) -> PathBuf {
    let mut base = agent_dir.parent();
    let agent_name = agent_dir.file_name().and_then(|name| name.to_str());
    if agent_name == Some("agent") {
        base = base.and_then(|dir| dir.parent());
    }
    base.unwrap_or(agent_dir).join(".agents")
}

/// A configured skill search root and its source classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    /// Filesystem path where recursive skill discovery starts.
    pub path: PathBuf,
    /// Source tag assigned to all skills discovered under this root.
    pub source: SkillSource,
}

/// Loader configuration for explicit and discovered skill files.
#[derive(Debug, Clone)]
pub struct LoadSkillsOptions {
    /// Ordered search roots scanned after explicit paths.
    pub roots: Vec<SkillRoot>,
    /// Explicit file or directory paths scanned before `roots`.
    pub explicit_paths: Vec<PathBuf>,
    /// Whether directory walks should follow symlinked directories.
    pub follow_symlinks: bool,
}

impl Default for LoadSkillsOptions {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            explicit_paths: Vec::new(),
            follow_symlinks: true,
        }
    }
}

/// Output of loading and parsing skill definitions.
#[derive(Debug, Default, Clone)]
pub struct LoadSkillsResult {
    /// Loaded skills after precedence and deduplication are applied.
    pub skills: Vec<Skill>,
    /// Non-fatal warnings and collision diagnostics encountered while loading.
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    source: SkillSource,
}

/// Load skill definitions from explicit paths and ordered roots.
///
/// Precedence order is:
/// 1. `explicit_paths` in the given order
/// 2. `roots` in the given order
///
/// When multiple loaded skills share the same `name`, the first one wins and a
/// collision diagnostic is emitted for each losing skill.
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();

    for explicit_path in &options.explicit_paths {
        collect_explicit_candidates(
            explicit_path,
            options.follow_symlinks,
            &mut candidates,
            &mut diagnostics,
        );
    }

    for root in &options.roots {
        collect_root_candidates(
            root,
            options.follow_symlinks,
            &mut candidates,
            &mut diagnostics,
        );
    }

    let mut seen_paths = HashSet::<PathBuf>::new();
    let mut seen_names = HashMap::<String, PathBuf>::new();
    let mut skills = Vec::new();

    for candidate in candidates {
        let dedupe_key = canonical_or_original(&candidate.path);
        if !seen_paths.insert(dedupe_key) {
            continue;
        }

        let (parsed, mut parse_diagnostics) = parse_skill_file(&candidate.path);
        diagnostics.append(&mut parse_diagnostics);

        let Some(parsed) = parsed else {
            continue;
        };

        if let Some(winner_path) = seen_names.get(&parsed.name) {
            diagnostics.push(collision_diagnostic(
                &parsed.name,
                winner_path,
                &candidate.path,
            ));
            continue;
        }

        seen_names.insert(parsed.name.clone(), candidate.path.clone());

        let base_dir = candidate
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();

        skills.push(Skill {
            name: parsed.name,
            description: parsed.description,
            file_path: candidate.path,
            base_dir,
            disable_model_invocation: parsed.disable_model_invocation,
            source: candidate.source,
        });
    }

    LoadSkillsResult {
        skills,
        diagnostics,
    }
}

fn collect_explicit_candidates(
    path: &Path,
    follow_symlinks: bool,
    out: &mut Vec<Candidate>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            diagnostics.push(warning(
                path,
                format!("Unable to access explicit skill path: {error}"),
            ));
            return;
        }
    };

    if metadata.is_dir() {
        let files = scan_for_skill_files(path, follow_symlinks, diagnostics);
        for file in files {
            out.push(Candidate {
                path: file,
                source: SkillSource::Explicit,
            });
        }
        return;
    }

    if metadata.is_file() {
        if path.file_name() == Some(OsStr::new(SKILL_FILE_NAME)) {
            out.push(Candidate {
                path: path.to_path_buf(),
                source: SkillSource::Explicit,
            });
        } else {
            diagnostics.push(warning(path, "Explicit skill file must be named SKILL.md"));
        }
        return;
    }

    diagnostics.push(warning(
        path,
        "Explicit skill path is neither a file nor directory",
    ));
}

fn collect_root_candidates(
    root: &SkillRoot,
    follow_symlinks: bool,
    out: &mut Vec<Candidate>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let files = scan_for_skill_files(&root.path, follow_symlinks, diagnostics);
    for file in files {
        out.push(Candidate {
            path: file,
            source: root.source,
        });
    }
}

fn scan_for_skill_files(
    root: &Path,
    follow_symlinks: bool,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }

    let mut builder = WalkBuilder::new(root);
    builder.follow_links(follow_symlinks);
    builder.hidden(true);
    builder.ignore(true);
    builder.git_ignore(true);
    builder.git_exclude(true);
    builder.git_global(true);
    builder.parents(true);
    builder.add_custom_ignore_filename(".fdignore");

    let mut files = Vec::new();

    for entry in builder.build() {
        match entry {
            Ok(entry) => {
                if !entry
                    .file_type()
                    .map(|file_type| file_type.is_file())
                    .unwrap_or(false)
                {
                    continue;
                }

                if entry.file_name() != OsStr::new(SKILL_FILE_NAME) {
                    continue;
                }

                if has_component(entry.path(), "node_modules") {
                    continue;
                }

                files.push(entry.into_path());
            }
            Err(error) => {
                diagnostics.push(warning(
                    root,
                    format!("Failed while scanning for skills: {error}"),
                ));
            }
        }
    }

    files.sort();
    files
}

fn has_component(path: &Path, needle: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str() == OsStr::new(needle))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn warning(path: &Path, message: impl Into<String>) -> SkillDiagnostic {
    SkillDiagnostic {
        level: SkillDiagnosticLevel::Warning,
        message: message.into(),
        path: path.to_path_buf(),
        collision: None,
    }
}

fn collision_diagnostic(name: &str, winner_path: &Path, loser_path: &Path) -> SkillDiagnostic {
    SkillDiagnostic {
        level: SkillDiagnosticLevel::Collision,
        message: format!(
            "Skill name collision for '{name}'; keeping '{}' and skipping '{}'",
            winner_path.display(),
            loser_path.display(),
        ),
        path: loser_path.to_path_buf(),
        collision: Some(SkillCollision {
            name: name.to_string(),
            winner_path: winner_path.to_path_buf(),
            loser_path: loser_path.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{default_skill_roots, load_skills, LoadSkillsOptions, SkillRoot};
    use crate::resource::settings::ResolvedResourceDirectories;
    use crate::skills::diagnostics::SkillDiagnosticLevel;
    use crate::skills::model::SkillSource;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn default_skill_roots_use_resolved_resource_directories() {
        let directories = ResolvedResourceDirectories {
            project_dir: PathBuf::from("/workspace/project/.roci"),
            agent_dir: PathBuf::from("/home/tester/.roci/agent"),
        };

        let roots = default_skill_roots(&directories);

        assert_eq!(roots[0].path, directories.project_dir.join("skills"));
        assert_eq!(
            roots[1].path,
            PathBuf::from("/workspace/project/.agents/skills")
        );
        assert_eq!(roots[2].path, directories.agent_dir.join("skills"));
        assert_eq!(roots[3].path, PathBuf::from("/home/tester/.agents/skills"));
    }

    #[test]
    fn default_skill_roots_use_parent_of_agent_dir_when_agent_dir_is_not_named_agent() {
        let directories = ResolvedResourceDirectories {
            project_dir: PathBuf::from("/workspace/project/.roci"),
            agent_dir: PathBuf::from("/config/global"),
        };

        let roots = default_skill_roots(&directories);

        assert_eq!(roots[2].path, PathBuf::from("/config/global/skills"));
        assert_eq!(roots[3].path, PathBuf::from("/config/.agents/skills"));
    }

    fn write_skill_file(
        root: &Path,
        folder_name: &str,
        description: &str,
        source_hint: &str,
    ) -> PathBuf {
        let skill_dir = root.join(folder_name);
        fs::create_dir_all(&skill_dir).expect("skill dir should be created");
        let file_path = skill_dir.join("SKILL.md");
        let content =
            format!("---\nname: {folder_name}\ndescription: {description}\n---\n\n{source_hint}\n");
        fs::write(&file_path, content).expect("skill file should be written");
        file_path
    }

    #[test]
    fn an_explicit_skill_path_wins_when_a_root_skill_uses_the_same_name() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let explicit_file = write_skill_file(
            temp_dir.path(),
            "collision-skill",
            "Explicit description",
            "explicit",
        );

        let root_dir = temp_dir.path().join("root");
        fs::create_dir_all(&root_dir).expect("root dir should be created");
        write_skill_file(&root_dir, "collision-skill", "Root description", "root");

        let options = LoadSkillsOptions {
            roots: vec![SkillRoot {
                path: root_dir,
                source: SkillSource::ProjectAgents,
            }],
            explicit_paths: vec![explicit_file.clone()],
            follow_symlinks: false,
        };

        let result = load_skills(&options);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].source, SkillSource::Explicit);
        assert_eq!(result.skills[0].description, "Explicit description");

        let collision = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.level == SkillDiagnosticLevel::Collision)
            .expect("collision diagnostic should exist");

        let collision_details = collision
            .collision
            .as_ref()
            .expect("collision details should be present");
        assert_eq!(collision_details.winner_path, explicit_file);
        assert!(collision_details
            .loser_path
            .ends_with("root/collision-skill/SKILL.md"));
    }

    #[test]
    fn the_first_root_in_order_wins_when_project_and_global_define_the_same_skill_name() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let project_root = temp_dir.path().join("project");
        let global_root = temp_dir.path().join("global");
        fs::create_dir_all(&project_root).expect("project root should be created");
        fs::create_dir_all(&global_root).expect("global root should be created");

        let project_file = write_skill_file(
            &project_root,
            "shared-skill",
            "Project description",
            "project",
        );
        write_skill_file(&global_root, "shared-skill", "Global description", "global");

        let options = LoadSkillsOptions {
            roots: vec![
                SkillRoot {
                    path: project_root,
                    source: SkillSource::ProjectRoci,
                },
                SkillRoot {
                    path: global_root,
                    source: SkillSource::GlobalRoci,
                },
            ],
            explicit_paths: Vec::new(),
            follow_symlinks: false,
        };

        let result = load_skills(&options);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].source, SkillSource::ProjectRoci);
        assert_eq!(result.skills[0].file_path, project_file);

        let collision = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.level == SkillDiagnosticLevel::Collision)
            .expect("collision diagnostic should exist");

        let collision_details = collision
            .collision
            .as_ref()
            .expect("collision details should be present");
        assert_eq!(collision_details.winner_path, project_file);
    }

    #[test]
    fn a_skill_markdown_file_matched_by_gitignore_is_not_loaded() {
        let temp_dir = tempdir().expect("temp dir should be created");
        let root = temp_dir.path().join("root");
        fs::create_dir_all(&root).expect("root should be created");
        fs::create_dir(root.join(".git")).expect("git dir should be created");

        fs::write(root.join(".gitignore"), "ignored-skill/\n")
            .expect("gitignore should be written");

        write_skill_file(&root, "ignored-skill", "Should not be loaded", "ignored");
        write_skill_file(&root, "visible-skill", "Should be loaded", "visible");

        let options = LoadSkillsOptions {
            roots: vec![SkillRoot {
                path: root,
                source: SkillSource::ProjectAgents,
            }],
            explicit_paths: Vec::new(),
            follow_symlinks: false,
        };

        let result = load_skills(&options);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "visible-skill");
    }

    #[cfg(any(unix, windows))]
    fn create_directory_symlink(source: &Path, destination: &Path) {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(source, destination).expect("symlink should be created");
        }

        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(source, destination)
                .expect("symlink should be created");
        }
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn a_symlinked_directory_is_scanned_when_follow_symlinks_is_enabled() {
        let temp_dir = tempdir().expect("temp dir should be created");

        let source_root = temp_dir.path().join("source");
        fs::create_dir_all(&source_root).expect("source root should be created");
        write_skill_file(
            &source_root,
            "symlink-skill",
            "From symlink target",
            "symlink",
        );

        let scan_root = temp_dir.path().join("scan-root");
        fs::create_dir_all(&scan_root).expect("scan root should be created");
        create_directory_symlink(&source_root, &scan_root.join("linked-skills"));

        let options = LoadSkillsOptions {
            roots: vec![SkillRoot {
                path: scan_root,
                source: SkillSource::ProjectAgents,
            }],
            explicit_paths: Vec::new(),
            follow_symlinks: true,
        };

        let result = load_skills(&options);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "symlink-skill");
    }
}
