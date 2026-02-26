//! Utilities for converting loaded skills into prompt snippets.

use crate::skills::model::Skill;

/// Render a prompt block describing the skills that can be loaded at runtime.
///
/// Only skills with `disable_model_invocation = false` are included. If no visible
/// skills exist, returns an empty string.
///
/// The output is formatted as XML-like blocks:
/// - Introductory guidance text
/// - `<available_skills>` container with child `<skill>` entries
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible_skills: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();

    if visible_skills.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    output.push_str("\n\n");
    output.push_str("The following skills provide specialized instructions for specific tasks.\n");
    output.push_str(
        "Use the read_file tool to load a skill's file when the task matches its description.\n",
    );
    output.push_str(
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md) and use that absolute path in tool commands.\n",
    );
    output.push_str("<available_skills>\n");

    for skill in visible_skills {
        output.push_str("  <skill>\n");
        output.push_str("    <name>");
        output.push_str(&escape_xml(&skill.name));
        output.push_str("</name>\n");

        output.push_str("    <description>");
        output.push_str(&escape_xml(&skill.description));
        output.push_str("</description>\n");

        output.push_str("    <location>");
        output.push_str(&escape_xml(&skill.file_path.to_string_lossy()));
        output.push_str("</location>\n");

        output.push_str("  </skill>\n");
    }

    output.push_str("</available_skills>");
    output
}

/// Merge an optional base system prompt with rendered skills text.
///
/// If no visible skills are provided, this returns `base` unchanged.
/// If `base` is `Some`, the rendered skills block is appended.
/// If `base` is `None`, returns only the rendered skills block with leading newlines removed.
pub fn merge_system_prompt_with_skills(base: Option<String>, skills: &[Skill]) -> Option<String> {
    let skills_text = format_skills_for_prompt(skills);

    if skills_text.is_empty() {
        return base;
    }

    match base {
        Some(mut existing_prompt) => {
            existing_prompt.push_str(&skills_text);
            Some(existing_prompt)
        }
        None => Some(skills_text.trim_start_matches('\n').to_string()),
    }
}

fn escape_xml(input: &str) -> String {
    let mut output = String::new();

    for ch in input.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(ch),
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{format_skills_for_prompt, merge_system_prompt_with_skills, Skill};
    use std::path::PathBuf;

    fn skill(name: &str, description: &str, path: &str, disable_model_invocation: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            file_path: PathBuf::from(path),
            base_dir: PathBuf::from("/tmp"),
            disable_model_invocation,
            source: crate::skills::model::SkillSource::ProjectAgents,
        }
    }

    #[test]
    fn format_skills_for_prompt_returns_empty_string_when_no_skills_are_visible() {
        let skills = vec![skill("my-skill", "desc", "/tmp/one.md", true)];
        assert!(format_skills_for_prompt(&skills).is_empty());
    }

    #[test]
    fn format_skills_for_prompt_skips_disabled_skills() {
        let skills = vec![
            skill("enabled", "Enabled skill", "/tmp/enabled.md", false),
            skill("disabled", "Disabled skill", "/tmp/disabled.md", true),
        ];

        let output = format_skills_for_prompt(&skills);
        assert!(output.contains("<name>enabled</name>"));
        assert!(!output.contains("<name>disabled</name>"));
    }

    #[test]
    fn merge_system_prompt_with_skills_appends_to_existing_base() {
        let skills = vec![skill("one", "desc", "/tmp/one.md", false)];
        let merged = merge_system_prompt_with_skills(Some("base".into()), &skills).unwrap();
        assert!(merged.starts_with("base"));
        assert!(merged.contains("<available_skills>"));
    }

    #[test]
    fn merge_system_prompt_with_skills_removes_leading_newlines_when_base_is_none() {
        let skills = vec![skill("one", "desc", "/tmp/one.md", false)];
        let merged = merge_system_prompt_with_skills(None, &skills).unwrap();
        let formatted = format_skills_for_prompt(&skills);
        assert_eq!(merged, formatted.trim_start_matches('\n'));
    }
}
