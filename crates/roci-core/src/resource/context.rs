use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::RociError;

use super::settings::ResourceDirectories;

const AGENTS_FILE_NAME: &str = "AGENTS.md";
const CLAUDE_FILE_NAME: &str = "CLAUDE.md";
const SYSTEM_FILE_NAME: &str = "SYSTEM.md";
const APPEND_SYSTEM_FILE_NAME: &str = "APPEND_SYSTEM.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFileResource {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDiagnostic {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextPromptResources {
    pub context_files: Vec<ContextFileResource>,
    pub system_prompt: Option<String>,
    pub append_system_prompts: Vec<String>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

#[derive(Debug, Clone)]
pub struct ContextPromptLoader {
    directories: ResourceDirectories,
}

impl Default for ContextPromptLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextPromptLoader {
    pub fn new() -> Self {
        Self {
            directories: ResourceDirectories::default(),
        }
    }

    pub fn with_directories(mut self, directories: ResourceDirectories) -> Self {
        self.directories = directories;
        self
    }

    pub fn directories(&self) -> &ResourceDirectories {
        &self.directories
    }

    pub fn load(&self, cwd: &Path) -> Result<ContextPromptResources, RociError> {
        self.load_with_home(cwd, std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }

    pub fn load_with_home(
        &self,
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> Result<ContextPromptResources, RociError> {
        let resolved_dirs = self.directories.resolve_with_home(cwd, home_dir)?;
        let mut diagnostics = Vec::new();

        let context_files = discover_context_files(&resolved_dirs.agent_dir, cwd, &mut diagnostics);

        let system_prompt = read_preferred_prompt(
            &resolved_dirs.project_dir.join(SYSTEM_FILE_NAME),
            &resolved_dirs.agent_dir.join(SYSTEM_FILE_NAME),
            &mut diagnostics,
        );
        let append_system_prompt = read_preferred_prompt(
            &resolved_dirs.project_dir.join(APPEND_SYSTEM_FILE_NAME),
            &resolved_dirs.agent_dir.join(APPEND_SYSTEM_FILE_NAME),
            &mut diagnostics,
        );

        Ok(ContextPromptResources {
            context_files,
            system_prompt,
            append_system_prompts: append_system_prompt.into_iter().collect(),
            diagnostics,
        })
    }
}

fn discover_context_files(
    global_agent_dir: &Path,
    cwd: &Path,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) -> Vec<ContextFileResource> {
    let mut ordered_directories = vec![global_agent_dir.to_path_buf()];
    ordered_directories.extend(ancestor_directories_from_root(cwd));

    let mut seen_paths = HashSet::<PathBuf>::new();
    let mut files = Vec::new();

    for directory in ordered_directories {
        let Some(candidate_path) = preferred_context_file_in_directory(&directory) else {
            continue;
        };

        let dedupe_key = canonical_or_original(&candidate_path);
        if !seen_paths.insert(dedupe_key) {
            continue;
        }

        match fs::read_to_string(&candidate_path) {
            Ok(content) => files.push(ContextFileResource {
                path: candidate_path,
                content,
            }),
            Err(error) => diagnostics.push(ResourceDiagnostic {
                path: candidate_path,
                message: format!("Unable to read context file: {error}"),
            }),
        }
    }

    files
}

fn preferred_context_file_in_directory(directory: &Path) -> Option<PathBuf> {
    let agents_path = directory.join(AGENTS_FILE_NAME);
    if agents_path.is_file() {
        return Some(agents_path);
    }

    let claude_path = directory.join(CLAUDE_FILE_NAME);
    if claude_path.is_file() {
        return Some(claude_path);
    }

    None
}

fn ancestor_directories_from_root(cwd: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = cwd.ancestors().map(Path::to_path_buf).collect();
    directories.reverse();
    directories
}

fn read_preferred_prompt(
    preferred_path: &Path,
    fallback_path: &Path,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) -> Option<String> {
    if let Some(content) = read_optional_file(preferred_path, diagnostics) {
        return Some(content);
    }
    read_optional_file(fallback_path, diagnostics)
}

fn read_optional_file(path: &Path, diagnostics: &mut Vec<ResourceDiagnostic>) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            diagnostics.push(ResourceDiagnostic {
                path: path.to_path_buf(),
                message: format!("Unable to read resource file: {error}"),
            });
            None
        }
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::ContextPromptLoader;
    use crate::resource::ResourceDirectories;

    #[test]
    fn context_files_are_loaded_with_global_first_then_ancestors_from_root_to_cwd() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let root = temp.path().join("workspace");
        let project = root.join("project");
        let nested = project.join("src/feature");

        fs::create_dir_all(home.join(".roci/agent")).expect("global directory should be created");
        fs::create_dir_all(&nested).expect("nested directory should be created");

        fs::write(home.join(".roci/agent/AGENTS.md"), "global agents context")
            .expect("global context should be written");
        fs::write(root.join("AGENTS.md"), "root context").expect("root context should be written");
        fs::write(project.join("AGENTS.md"), "project context")
            .expect("project context should be written");
        fs::write(nested.join("AGENTS.md"), "cwd context").expect("cwd context should be written");

        let resources = ContextPromptLoader::new()
            .load_with_home(&nested, Some(&home))
            .expect("resources should load");

        let actual: Vec<PathBuf> = resources
            .context_files
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(
            actual,
            vec![
                home.join(".roci/agent/AGENTS.md"),
                root.join("AGENTS.md"),
                project.join("AGENTS.md"),
                nested.join("AGENTS.md"),
            ],
        );
    }

    #[test]
    fn agents_file_has_precedence_over_claude_in_same_directory() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");

        fs::create_dir_all(home.join(".roci/agent")).expect("global directory should be created");
        fs::create_dir_all(&cwd).expect("cwd should be created");

        fs::write(home.join(".roci/agent/AGENTS.md"), "global agents")
            .expect("global agents should be written");
        fs::write(home.join(".roci/agent/CLAUDE.md"), "global claude")
            .expect("global claude should be written");
        fs::write(cwd.join("AGENTS.md"), "project agents")
            .expect("project agents should be written");
        fs::write(cwd.join("CLAUDE.md"), "project claude")
            .expect("project claude should be written");

        let resources = ContextPromptLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("resources should load");

        let content: Vec<&str> = resources
            .context_files
            .iter()
            .map(|entry| entry.content.as_str())
            .collect();
        assert_eq!(content, vec!["global agents", "project agents"]);
    }

    #[test]
    fn duplicate_context_paths_are_deduplicated() {
        let temp = tempdir().expect("temp dir should be created");
        let cwd = temp.path().join("workspace");
        let shared = temp.path().join("shared-agent");

        fs::create_dir_all(&cwd).expect("cwd should be created");
        fs::create_dir_all(&shared).expect("shared directory should be created");
        fs::write(shared.join("AGENTS.md"), "shared context")
            .expect("shared context should be written");

        let loader = ContextPromptLoader::new().with_directories(ResourceDirectories {
            agent_dir: PathBuf::from("../shared-agent"),
            project_dir: PathBuf::from(".roci"),
        });

        #[cfg(unix)]
        std::os::unix::fs::symlink(&shared, cwd.join("linked-shared"))
            .expect("symlink should be created");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&shared, cwd.join("linked-shared"))
            .expect("symlink should be created");

        let resources = loader
            .load_with_home(&cwd.join("linked-shared"), None)
            .expect("resources should load");

        assert_eq!(resources.context_files.len(), 1);
        assert_eq!(resources.context_files[0].content, "shared context");
    }

    #[test]
    fn system_and_append_prompts_prefer_project_directory_over_global() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let project_resource_dir = cwd.join(".roci");
        let global_resource_dir = home.join(".roci/agent");

        fs::create_dir_all(&project_resource_dir).expect("project resource directory should exist");
        fs::create_dir_all(&global_resource_dir).expect("global resource directory should exist");

        fs::write(global_resource_dir.join("SYSTEM.md"), "global system")
            .expect("global system should be written");
        fs::write(project_resource_dir.join("SYSTEM.md"), "project system")
            .expect("project system should be written");
        fs::write(
            global_resource_dir.join("APPEND_SYSTEM.md"),
            "global append",
        )
        .expect("global append should be written");
        fs::write(
            project_resource_dir.join("APPEND_SYSTEM.md"),
            "project append",
        )
        .expect("project append should be written");

        let resources = ContextPromptLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("resources should load");

        assert_eq!(resources.system_prompt, Some("project system".to_string()));
        assert_eq!(
            resources.append_system_prompts,
            vec!["project append".to_string()],
        );
    }

    #[test]
    fn loader_honors_directory_overrides_for_system_and_append_prompts() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let custom_agent = home.join("custom-agent");
        let custom_project = cwd.join("resource");

        fs::create_dir_all(&cwd).expect("cwd should be created");
        fs::create_dir_all(&custom_agent).expect("custom agent should be created");
        fs::create_dir_all(&custom_project).expect("custom project should be created");

        fs::write(custom_agent.join("SYSTEM.md"), "agent system")
            .expect("agent system should be written");
        fs::write(custom_project.join("APPEND_SYSTEM.md"), "project append")
            .expect("project append should be written");

        let loader = ContextPromptLoader::new().with_directories(ResourceDirectories {
            agent_dir: PathBuf::from("~/custom-agent"),
            project_dir: PathBuf::from("resource"),
        });

        let resources = loader
            .load_with_home(&cwd, Some(&home))
            .expect("resources should load");

        assert_eq!(resources.system_prompt, Some("agent system".to_string()));
        assert_eq!(
            resources.append_system_prompts,
            vec!["project append".to_string()],
        );
    }

    #[test]
    fn read_failures_are_reported_through_diagnostics() {
        let temp = tempdir().expect("temp dir should be created");
        let home = temp.path().join("home");
        let cwd = temp.path().join("workspace");
        let project_resource_dir = cwd.join(".roci");
        let global_resource_dir = home.join(".roci/agent");

        fs::create_dir_all(&project_resource_dir).expect("project resource directory should exist");
        fs::create_dir_all(&global_resource_dir).expect("global resource directory should exist");
        fs::create_dir_all(project_resource_dir.join("SYSTEM.md"))
            .expect("project system path should be a directory to force read failure");
        fs::write(global_resource_dir.join("SYSTEM.md"), "global system")
            .expect("global system should be written");

        let resources = ContextPromptLoader::new()
            .load_with_home(&cwd, Some(&home))
            .expect("resources should load");

        assert_eq!(resources.system_prompt, Some("global system".to_string()));
        assert!(
            resources
                .diagnostics
                .iter()
                .any(|entry| entry.path == project_resource_dir.join("SYSTEM.md")),
            "expected read failure diagnostics for invalid project system prompt path",
        );
    }
}
