//! Frontmatter parsing for markdown-like skill files.

use std::{fs, path::Path, sync::OnceLock};

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

static SKILL_NAME_RE: OnceLock<Regex> = OnceLock::new();

fn skill_name_re() -> &'static Regex {
    SKILL_NAME_RE.get_or_init(|| {
        Regex::new(r"^[a-z0-9-]+$").expect("skill name validation regex must compile")
    })
}

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

    if !skill_name_re().is_match(name) {
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

fn parse_skill_name(
    path: &Path,
    raw_name: Option<String>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Option<String> {
    let parsed_name = raw_name.unwrap_or_else(|| parent_directory_name(path));

    if !valid_skill_name(&parsed_name) {
        diagnostics.push(warning(
            path,
            format!(
                "Skill name '{parsed_name}' is invalid; it must match /^[a-z0-9-]+$/, be at most 64 chars, not start or end with '-', and not contain '--'"
            ),
        ));
        return None;
    }

    let parent_name = parent_directory_name(path);
    if !parent_name.is_empty() && parsed_name != parent_name {
        diagnostics.push(warning(
            path,
            format!(
                "Skill name '{parsed_name}' does not match parent directory name '{parent_name}'"
            ),
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
            diagnostics.push(warning(
                path,
                "Skill file is missing valid YAML frontmatter",
            ));
            return (None, diagnostics);
        }
    };

    let frontmatter = match serde_yaml::from_str::<SkillFrontmatter>(&frontmatter_text) {
        Ok(frontmatter) => frontmatter,
        Err(error) => {
            diagnostics.push(warning(
                path,
                format!("Invalid skill frontmatter YAML: {error}"),
            ));
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
                "Skill frontmatter is missing required field: description",
            ));
            return (None, diagnostics);
        }
    };

    if description.is_empty() || description.len() > 1024 {
        diagnostics.push(warning(
            path,
            "Skill frontmatter description must be non-empty and at most 1024 chars",
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
