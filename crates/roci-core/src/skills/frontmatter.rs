//! Frontmatter parsing for markdown-like skill files.

use std::{fs, path::{Path, PathBuf}, sync::LazyLock};

use crate::skills::diagnostics::{SkillDiagnostic, SkillDiagnosticLevel};
use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
}

/// Parsed skill metadata extracted from frontmatter.
pub(crate) struct ParsedSkill {
    /// Skill name used in prompts and lookups.
    pub name: String,
    /// Skill description injected into prompt context.
    pub description: String,
    /// Whether model invocation should be skipped for this skill.
    pub disable_model_invocation: bool,
}

static SKILL_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9-]+$").expect("skill name validation regex must compile")
});

fn warning(path: &Path, message: impl Into<String>) -> SkillDiagnostic {
    SkillDiagnostic {
        level: SkillDiagnosticLevel::Warning,
        message: message.into(),
        path: path.to_path_buf(),
        collision: None,
    }
}

fn extract_frontmatter_contents(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    let mut frontmatter = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            return Some(frontmatter.join("\n"));
        }
        frontmatter.push(line);
    }

    None
}

fn valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }

    if !SKILL_NAME_RE.is_match(name) {
        return false;
    }

    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }

    if name.contains("--") {
        return false;
    }

    true
}

fn parent_directory_name(path: &Path) -> String {
    path.parent()
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_string()
}

fn parse_skill_name(path: &Path, raw_name: Option<String>, diagnostics: &mut Vec<SkillDiagnostic>) -> Option<String> {
    let parsed_name = raw_name.unwrap_or_else(|| parent_directory_name(path));

    if !valid_skill_name(&parsed_name) {
        diagnostics.push(warning(
            path,
            format!("Skill name '{parsed_name}' is invalid. It must match /^[a-z0-9-]+$/, be at most 64 chars, not start or end with '-', and not contain '--'."),
        ));
        return None;
    }

    let parent_name = parent_directory_name(path);
    if !parent_name.is_empty() && parsed_name != parent_name {
        diagnostics.push(warning(
            path,
            format!("Skill name '{parsed_name}' does not match parent directory name '{parent_name}'."),
        ));
    }

    Some(parsed_name)
}

/// Parse a markdown-like skill definition file and return parsed metadata plus diagnostics.
pub(crate) fn parse_skill_file(path: &Path) -> (Option<ParsedSkill>, Vec<SkillDiagnostic>) {
    let mut diagnostics = Vec::new();

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            diagnostics.push(warning(path, format!("Unable to read skill file: {error}")));
            return (None, diagnostics);
        }
    };

    let frontmatter_text = match extract_frontmatter_contents(&content) {
        Some(text) => text,
        None => {
            diagnostics.push(warning(path, "Skill file is missing valid YAML frontmatter."));
            return (None, diagnostics);
        }
    };

    let frontmatter = match serde_yaml::from_str::<SkillFrontmatter>(&frontmatter_text) {
        Ok(frontmatter) => frontmatter,
        Err(error) => {
            diagnostics.push(warning(path, format!("Invalid skill frontmatter YAML: {error}")));
            return (None, diagnostics);
        }
    };

    let name = match parse_skill_name(path, frontmatter.name, &mut diagnostics) {
        Some(name) => name,
        None => return (None, diagnostics),
    };

    let description = match frontmatter.description {
        Some(description) => description,
        None => {
            diagnostics.push(warning(
                path,
                "Skill frontmatter is missing required field: description.",
            ));
            return (None, diagnostics);
        }
    };

    if description.is_empty() || description.len() > 1024 {
        diagnostics.push(warning(
            path,
            "Skill frontmatter description must be non-empty and at most 1024 chars.",
        ));
        return (None, diagnostics);
    }

    let disable_model_invocation = frontmatter.disable_model_invocation.unwrap_or(false);

    (
        Some(ParsedSkill {
            name,
            description,
            disable_model_invocation,
        }),
        diagnostics,
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_skill_file, ParsedSkill};
    use crate::skills::diagnostics::SkillDiagnosticLevel;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use tempfile::{tempdir, TempDir};

    fn write_skill_file(path: &Path, body: &str) {
        let mut file = File::create(path).expect("skill file should be created");
        file.write_all(body.as_bytes())
            .expect("skill file contents should be written");
    }

    fn make_skill_dir(temp_dir: &TempDir, name: &str) -> PathBuf {
        let dir = temp_dir.path().join(name);
        fs::create_dir(&dir).expect("skill directory should be created");
        dir
    }

    #[test]
    fn parsing_a_skill_file_with_a_valid_frontmatter_loads_the_skill_definition() {
        let dir = tempdir().expect("temporary directory should be created");
        let skill_dir = make_skill_dir(&dir, "valid-skill");
        let path = skill_dir.join("one.md");

        let content = "---\nname: valid-skill\ndescription: This skill is valid.\ndisable-model-invocation: true\n---\n";
        write_skill_file(&path, content);

        let (parsed, diagnostics) = parse_skill_file(&path);
        assert!(diagnostics.is_empty());

        let skill = parsed.expect("skill should parse successfully");
        assert_eq!(skill.name, "valid-skill");
        assert_eq!(skill.description, "This skill is valid.");
        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn parsing_a_skill_file_without_a_description_returns_none_and_reports_a_warning() {
        let dir = tempdir().expect("temporary directory should be created");
        let skill_dir = make_skill_dir(&dir, "no-description");
        let path = skill_dir.join("two.md");

        let content = "---\nname: no-description\n---\n";
        write_skill_file(&path, content);

        let (parsed, diagnostics) = parse_skill_file(&path);

        assert!(parsed.is_none());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, SkillDiagnosticLevel::Warning);
        assert!(diagnostics[0].message.contains("description"));
    }

    #[test]
    fn parsing_a_skill_file_with_invalid_yaml_returns_none_and_reports_a_warning() {
        let dir = tempdir().expect("temporary directory should be created");
        let skill_dir = make_skill_dir(&dir, "bad-yaml");
        let path = skill_dir.join("three.md");

        let content = "---\nname: [bad\ndescription: malformed\n---\n";
        write_skill_file(&path, content);

        let (parsed, diagnostics) = parse_skill_file(&path);

        assert!(parsed.is_none());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, SkillDiagnosticLevel::Warning);
    }

    #[test]
    fn parsing_a_skill_file_with_a_name_that_differs_from_parent_reports_a_warning_but_still_loads() {
        let dir = tempdir().expect("temporary directory should be created");
        let skill_dir = make_skill_dir(&dir, "parent-name");
        let path = skill_dir.join("four.md");

        let content = "---\nname: child-name\ndescription: This description is okay.\n---\n";
        write_skill_file(&path, content);

        let (parsed, diagnostics) = parse_skill_file(&path);

        let skill = parsed.expect("skill should parse successfully");
        assert_eq!(skill.name, "child-name");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, SkillDiagnosticLevel::Warning);
        assert!(diagnostics[0].message.contains("does not match parent"));
    }

    #[test]
    fn parsing_a_skill_file_with_an_invalid_name_reports_a_warning_and_returns_none() {
        let dir = tempdir().expect("temporary directory should be created");
        let skill_dir = make_skill_dir(&dir, "bad-name");
        let path = skill_dir.join("five.md");

        let content = "---\nname: Bad_Name\ndescription: This is okay.\n---\n";
        write_skill_file(&path, content);

        let (parsed, diagnostics) = parse_skill_file(&path);

        assert!(parsed.is_none());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, SkillDiagnosticLevel::Warning);
        assert!(diagnostics[0].message.contains("invalid"));
    }
}
