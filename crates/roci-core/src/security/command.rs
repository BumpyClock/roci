use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub trait CommandClassifier: Send + Sync {
    fn classify(&self, input: CommandInput) -> CommandInsight;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInput {
    pub raw_command: String,
    pub cwd: Option<PathBuf>,
    pub tool_name: Option<String>,
    pub shell_kind: Option<ShellKind>,
    pub platform: Option<CommandPlatform>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Sh,
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPlatform {
    Unix,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    ReadOnly,
    WritesFilesystem,
    DestructiveDelete,
    PrivilegeEscalation,
    PermissionChange,
    ProcessControl,
    NetworkLikely,
    CodeExecution,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInsight {
    pub normalized_command: String,
    pub primary_executable: Option<String>,
    pub categories: Vec<CommandCategory>,
    pub reasons: Vec<String>,
    pub confidence: CommandConfidence,
}

pub struct HeuristicCommandClassifier;

pub fn classify_shell_command(raw_command: &str) -> CommandInsight {
    HeuristicCommandClassifier.classify(CommandInput {
        raw_command: raw_command.to_string(),
        cwd: None,
        tool_name: None,
        shell_kind: None,
        platform: None,
    })
}

impl CommandClassifier for HeuristicCommandClassifier {
    fn classify(&self, input: CommandInput) -> CommandInsight {
        let (segments, connectors) = split_segments(&input.raw_command);
        let mut categories = Vec::new();
        let mut reasons = Vec::new();
        let mut primary_executable = None;
        let mut normalized_segments = Vec::new();

        detect_shell_syntax(&input.raw_command, &mut categories, &mut reasons);

        for connector in connectors {
            add_reason(&mut reasons, format!("connector detected: {connector}"));
            add_category(&mut categories, CommandCategory::Unknown);
        }

        for segment in segments {
            let tokens = tokenize_segment(&segment);
            if tokens.is_empty() {
                continue;
            }

            let segment_insight = classify_segment(&tokens, &mut categories, &mut reasons);
            match segment_insight {
                Some((executable, normalized_segment)) => {
                    if primary_executable.is_none() {
                        primary_executable = Some(executable.clone());
                    }
                    normalized_segments.push(normalized_segment);
                }
                None => {
                    add_category(&mut categories, CommandCategory::Unknown);
                    add_reason(&mut reasons, "unknown or empty command segment".to_string());
                }
            }
        }

        if categories.is_empty() {
            add_category(&mut categories, CommandCategory::Unknown);
            add_reason(&mut reasons, "unknown or empty command segment".to_string());
        }

        let confidence = if categories.contains(&CommandCategory::Unknown) {
            CommandConfidence::Low
        } else if input.raw_command.contains(';')
            || input.raw_command.contains('&')
            || input.raw_command.contains("&&")
            || input.raw_command.contains("||")
            || input.raw_command.contains('|')
            || input.raw_command.contains('\n')
        {
            CommandConfidence::Medium
        } else {
            CommandConfidence::High
        };

        CommandInsight {
            normalized_command: if normalized_segments.is_empty() {
                input.raw_command.trim().to_string()
            } else {
                normalized_segments.join(" && ")
            },
            primary_executable,
            categories,
            reasons,
            confidence,
        }
    }
}

const WRAPPERS: &[&str] = &["sudo", "doas", "command", "builtin", "time", "env", "xargs"];
const DESTRUCTIVE: &[&str] = &["rm", "rmdir", "unlink", "shred", "dd", "mkfs"];
const WRITE_FS: &[&str] = &["mv", "cp", "touch", "mkdir", "tee"];
const PERMISSION: &[&str] = &["chmod", "chown", "chgrp", "setfacl"];
const PROCESS: &[&str] = &["kill", "pkill", "killall", "launchctl", "systemctl"];
const NETWORK: &[&str] = &["curl", "wget", "ssh", "scp", "rsync", "nc"];
const CODE_EXEC: &[&str] = &[
    "sh", "bash", "zsh", "fish", "python", "ruby", "node", "perl", "eval",
];
const READ_ONLY: &[&str] = &[
    "cat", "ls", "pwd", "head", "tail", "less", "more", "grep", "rg", "find", "wc", "echo",
];
const GIT_READ_ONLY: &[&str] = &["status", "log", "diff", "show", "branch"];
const GIT_WRITE: &[&str] = &[
    "commit", "push", "rebase", "reset", "checkout", "clean", "apply", "pull", "merge", "add",
];

fn split_segments(input: &str) -> (Vec<String>, Vec<String>) {
    let mut segments = Vec::new();
    let mut connectors = Vec::new();
    let mut segment = String::new();
    let mut chars = input.chars().peekable();
    let mut quote = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            segment.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            segment.push(ch);
            escaped = true;
            continue;
        }

        if let Some(active_quote) = quote {
            segment.push(ch);
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            segment.push(ch);
            continue;
        }

        let connector = match ch {
            '\n' => Some("newline".to_string()),
            ';' => Some(";".to_string()),
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                Some("&&".to_string())
            }
            '&' => Some("&".to_string()),
            '|' if chars.peek() == Some(&'|') => {
                chars.next();
                Some("||".to_string())
            }
            '|' => Some("|".to_string()),
            _ => None,
        };

        if let Some(connector) = connector {
            let trimmed = segment.trim();
            if !trimmed.is_empty() {
                segments.push(trimmed.to_string());
            }
            segment.clear();
            connectors.push(connector);
        } else {
            segment.push(ch);
        }
    }

    let trimmed = segment.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }

    (segments, connectors)
}

fn tokenize_segment(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in segment.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }

        if ch == '\\' {
            escaped = true;
            continue;
        }

        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                token.push(ch);
            }
            continue;
        }

        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }

        if ch.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(ch);
        }
    }

    if !token.is_empty() {
        tokens.push(token);
    }

    tokens
}

fn classify_segment(
    tokens: &[String],
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) -> Option<(String, String)> {
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index].as_str();
        if is_env_assignment(token) {
            index += 1;
            continue;
        }

        if WRAPPERS.contains(&token) {
            add_reason(reasons, format!("wrapper detected: {token}"));
            if matches!(token, "sudo" | "doas") {
                add_category(categories, CommandCategory::PrivilegeEscalation);
            }
            index = skip_wrapper_arguments(token, tokens, index + 1, categories, reasons);
            continue;
        }

        let executable = token.to_string();
        classify_executable(token, &tokens[index + 1..], categories, reasons);
        return Some((executable, tokens[index..].join(" ")));
    }

    None
}

fn classify_executable(
    executable: &str,
    args: &[String],
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    if executable == "git" {
        classify_git(args.first().map(String::as_str), categories, reasons);
        return;
    }

    if executable == "find" {
        classify_find(args, categories, reasons);
        return;
    }

    if executable == "eval" {
        classify_eval(args, categories, reasons);
        return;
    }

    if DESTRUCTIVE.contains(&executable) {
        add_category(categories, CommandCategory::DestructiveDelete);
        add_reason(
            reasons,
            format!("matched destructive delete executable: {executable}"),
        );
    } else if WRITE_FS.contains(&executable) {
        add_category(categories, CommandCategory::WritesFilesystem);
        add_reason(
            reasons,
            format!("matched filesystem write executable: {executable}"),
        );
    } else if PERMISSION.contains(&executable) {
        add_category(categories, CommandCategory::PermissionChange);
        add_reason(
            reasons,
            format!("matched permission change executable: {executable}"),
        );
    } else if PROCESS.contains(&executable) {
        add_category(categories, CommandCategory::ProcessControl);
        add_reason(
            reasons,
            format!("matched process control executable: {executable}"),
        );
    } else if NETWORK.contains(&executable) {
        add_category(categories, CommandCategory::NetworkLikely);
        add_reason(reasons, format!("matched network executable: {executable}"));
    } else if CODE_EXEC.contains(&executable) {
        add_category(categories, CommandCategory::CodeExecution);
        add_reason(
            reasons,
            format!("matched code execution executable: {executable}"),
        );
    } else if READ_ONLY.contains(&executable) {
        add_category(categories, CommandCategory::ReadOnly);
        add_reason(
            reasons,
            format!("matched read-only executable: {executable}"),
        );
    } else {
        add_category(categories, CommandCategory::Unknown);
        add_reason(reasons, format!("unknown executable: {executable}"));
    }
}

fn classify_eval(
    args: &[String],
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    add_category(categories, CommandCategory::CodeExecution);
    add_category(categories, CommandCategory::Unknown);
    add_reason(reasons, "matched eval command execution".to_string());

    let joined = args.join(" ");
    let (segments, connectors) = split_segments(&joined);
    for connector in connectors {
        add_reason(reasons, format!("connector detected: {connector}"));
    }
    for segment in segments {
        let tokens = tokenize_segment(&segment);
        if !tokens.is_empty() {
            let _ = classify_segment(&tokens, categories, reasons);
        }
    }
}

fn detect_shell_syntax(
    input: &str,
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    if input.contains("$(") {
        add_category(categories, CommandCategory::Unknown);
        add_reason(reasons, "shell syntax detected: $()".to_string());
    }
    if input.contains('`') {
        add_category(categories, CommandCategory::Unknown);
        add_reason(reasons, "shell syntax detected: backticks".to_string());
    }
    if input.contains('>') || input.contains('<') {
        add_category(categories, CommandCategory::Unknown);
        add_reason(reasons, "shell syntax detected: redirect".to_string());
    }
    if input.contains('\n') {
        add_category(categories, CommandCategory::Unknown);
        add_reason(reasons, "shell syntax detected: newline".to_string());
    }

    if categories.contains(&CommandCategory::Unknown) {
        for token in shellish_tokens(input) {
            if is_known_command_token(&token) {
                classify_executable(&token, &[], categories, reasons);
            }
        }
    }
}

fn shellish_tokens(input: &str) -> Vec<String> {
    input
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn skip_wrapper_arguments(
    wrapper: &str,
    tokens: &[String],
    mut index: usize,
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) -> usize {
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            return index + 1;
        }

        if is_env_assignment(token) {
            index += 1;
            continue;
        }

        if is_known_command_token(token) || WRAPPERS.contains(&token) {
            return index;
        }

        if token.starts_with('-') {
            index += 1;
            if wrapper_option_takes_value(wrapper, token)
                && !token.contains('=')
                && index < tokens.len()
            {
                index += 1;
            }
            continue;
        }

        if next_known_command_index(tokens, index + 1).is_some() {
            add_category(categories, CommandCategory::Unknown);
            add_reason(
                reasons,
                format!("unknown wrapper argument before executable: {token}"),
            );
            index += 1;
            continue;
        }

        return index;
    }

    index
}

fn wrapper_option_takes_value(wrapper: &str, option: &str) -> bool {
    match wrapper {
        "sudo" | "doas" => matches!(
            option,
            "-u" | "--user"
                | "-g"
                | "--group"
                | "-h"
                | "--host"
                | "-p"
                | "--prompt"
                | "-C"
                | "--close-from"
                | "-r"
                | "--role"
                | "-t"
                | "--type"
        ),
        "env" => matches!(
            option,
            "-C" | "--chdir" | "-S" | "--split-string" | "-u" | "--unset"
        ),
        "xargs" => matches!(
            option,
            "-I" | "--replace"
                | "-i"
                | "--replace-str"
                | "-E"
                | "--eof"
                | "-a"
                | "--arg-file"
                | "-d"
                | "--delimiter"
                | "-n"
                | "--max-args"
                | "-P"
                | "--max-procs"
                | "-s"
                | "--max-chars"
        ),
        _ => false,
    }
}

fn is_known_command_token(token: &str) -> bool {
    token == "git"
        || DESTRUCTIVE.contains(&token)
        || WRITE_FS.contains(&token)
        || PERMISSION.contains(&token)
        || PROCESS.contains(&token)
        || NETWORK.contains(&token)
        || CODE_EXEC.contains(&token)
        || READ_ONLY.contains(&token)
}

fn next_known_command_index(tokens: &[String], start: usize) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| is_known_command_token(token).then_some(index))
}

fn classify_find(
    args: &[String],
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    let mut risky = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "-delete" => {
                risky = true;
                add_category(categories, CommandCategory::DestructiveDelete);
                add_reason(
                    reasons,
                    "matched find destructive predicate: -delete".to_string(),
                );
            }
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                risky = true;
                let predicate = args[index].clone();
                add_category(categories, CommandCategory::CodeExecution);
                add_category(categories, CommandCategory::Unknown);
                add_reason(
                    reasons,
                    format!("matched find execution predicate: {predicate}"),
                );
                classify_find_exec_args(&args[index + 1..], categories, reasons);
            }
            "-fprint" | "-fprint0" | "-fprintf" | "-fls" => {
                risky = true;
                let predicate = args[index].clone();
                add_category(categories, CommandCategory::WritesFilesystem);
                add_category(categories, CommandCategory::Unknown);
                add_reason(
                    reasons,
                    format!("matched find filesystem write predicate: {predicate}"),
                );
            }
            _ => {}
        }
        index += 1;
    }

    if !risky {
        add_category(categories, CommandCategory::ReadOnly);
        add_reason(reasons, "matched read-only executable: find".to_string());
    }
}

fn classify_find_exec_args(
    args: &[String],
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    for arg in args {
        if matches!(arg.as_str(), ";" | "+") {
            return;
        }

        if is_known_command_token(arg) {
            classify_executable(arg, &[], categories, reasons);
            return;
        }
    }
}

fn classify_git(
    subcommand: Option<&str>,
    categories: &mut Vec<CommandCategory>,
    reasons: &mut Vec<String>,
) {
    match subcommand {
        Some(subcommand) if GIT_READ_ONLY.contains(&subcommand) => {
            add_category(categories, CommandCategory::ReadOnly);
            add_reason(
                reasons,
                format!("matched git read-only subcommand: {subcommand}"),
            );
        }
        Some(subcommand) if GIT_WRITE.contains(&subcommand) => {
            add_category(categories, CommandCategory::WritesFilesystem);
            add_reason(
                reasons,
                format!("matched git write subcommand: {subcommand}"),
            );
        }
        Some(subcommand) => {
            add_category(categories, CommandCategory::Unknown);
            add_reason(reasons, format!("unknown git subcommand: {subcommand}"));
        }
        None => {
            add_category(categories, CommandCategory::Unknown);
            add_reason(reasons, "unknown git subcommand".to_string());
        }
    }
}

fn is_env_assignment(token: &str) -> bool {
    let Some((key, _)) = token.split_once('=') else {
        return false;
    };

    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && key
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
}

fn add_category(categories: &mut Vec<CommandCategory>, category: CommandCategory) {
    if !categories.contains(&category) {
        categories.push(category);
    }
}

fn add_reason(reasons: &mut Vec<String>, reason: String) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(report: &CommandInsight, category: CommandCategory) -> bool {
        report.categories.contains(&category)
    }

    struct CommandFixture {
        command: &'static str,
        expected_categories: &'static [CommandCategory],
        expected_confidence: CommandConfidence,
        expected_reasons: &'static [&'static str],
    }

    impl CommandFixture {
        fn new(
            command: &'static str,
            expected_categories: &'static [CommandCategory],
            expected_confidence: CommandConfidence,
        ) -> Self {
            Self {
                command,
                expected_categories,
                expected_confidence,
                expected_reasons: &[],
            }
        }

        fn with_reasons(mut self, expected_reasons: &'static [&'static str]) -> Self {
            self.expected_reasons = expected_reasons;
            self
        }

        fn assert_matches(&self) {
            let report = classify_shell_command(self.command);

            assert_eq!(
                report.categories.as_slice(),
                self.expected_categories,
                "categories mismatch for command `{}`; reasons: {:?}",
                self.command,
                report.reasons
            );
            assert_eq!(
                &report.confidence, &self.expected_confidence,
                "confidence mismatch for command `{}`; reasons: {:?}",
                self.command, report.reasons
            );
            for expected_reason in self.expected_reasons {
                assert!(
                    report
                        .reasons
                        .iter()
                        .any(|reason| reason == expected_reason),
                    "missing reason `{}` for command `{}`; got {:?}",
                    expected_reason,
                    self.command,
                    report.reasons
                );
            }
        }
    }

    #[test]
    fn command_classifier_fixture_corpus() {
        use CommandCategory::{
            CodeExecution, DestructiveDelete, NetworkLikely, PermissionChange, PrivilegeEscalation,
            ProcessControl, ReadOnly, Unknown, WritesFilesystem,
        };
        use CommandConfidence::{High, Low};

        let fixtures = [
            CommandFixture::new("cat Cargo.toml", &[ReadOnly], High),
            CommandFixture::new("ls crates", &[ReadOnly], High),
            CommandFixture::new("grep roci Cargo.toml", &[ReadOnly], High),
            CommandFixture::new("rg roci crates", &[ReadOnly], High),
            CommandFixture::new("git status", &[ReadOnly], High)
                .with_reasons(&["matched git read-only subcommand: status"]),
            CommandFixture::new("git show HEAD", &[ReadOnly], High)
                .with_reasons(&["matched git read-only subcommand: show"]),
            CommandFixture::new("touch /tmp/roci-file", &[WritesFilesystem], High),
            CommandFixture::new("mkdir /tmp/roci-dir", &[WritesFilesystem], High),
            CommandFixture::new("cp Cargo.toml /tmp/roci-copy", &[WritesFilesystem], High),
            CommandFixture::new("mv /tmp/roci-old /tmp/roci-new", &[WritesFilesystem], High),
            CommandFixture::new("tee /tmp/roci-out", &[WritesFilesystem], High),
            CommandFixture::new("rm -rf /tmp/roci-dir", &[DestructiveDelete], High)
                .with_reasons(&["matched destructive delete executable: rm"]),
            CommandFixture::new("rmdir /tmp/roci-dir", &[DestructiveDelete], High),
            CommandFixture::new("shred /tmp/roci-secret", &[DestructiveDelete], High),
            CommandFixture::new("find /tmp/roci-dir -delete", &[DestructiveDelete], High)
                .with_reasons(&["matched find destructive predicate: -delete"]),
            CommandFixture::new(
                r"find /tmp/roci-dir -exec rm {} \;",
                &[CodeExecution, Unknown, DestructiveDelete],
                Low,
            )
            .with_reasons(&["matched find execution predicate: -exec"]),
            CommandFixture::new("sudo ls /root", &[PrivilegeEscalation, ReadOnly], High)
                .with_reasons(&["wrapper detected: sudo"]),
            CommandFixture::new("doas ls /root", &[PrivilegeEscalation, ReadOnly], High)
                .with_reasons(&["wrapper detected: doas"]),
            CommandFixture::new(
                "sudo -u root ls /root",
                &[PrivilegeEscalation, ReadOnly],
                High,
            )
            .with_reasons(&["wrapper detected: sudo"]),
            CommandFixture::new("chmod 600 /tmp/roci-secret", &[PermissionChange], High),
            CommandFixture::new(
                "chown root:wheel /tmp/roci-secret",
                &[PermissionChange],
                High,
            ),
            CommandFixture::new("chgrp wheel /tmp/roci-secret", &[PermissionChange], High),
            CommandFixture::new("kill 1234", &[ProcessControl], High),
            CommandFixture::new("pkill roci", &[ProcessControl], High),
            CommandFixture::new("systemctl restart roci.service", &[ProcessControl], High),
            CommandFixture::new("launchctl kickstart system/roci", &[ProcessControl], High),
            CommandFixture::new("curl https://example.com", &[NetworkLikely], High),
            CommandFixture::new("wget https://example.com/file", &[NetworkLikely], High),
            CommandFixture::new("ssh example.com", &[NetworkLikely], High),
            CommandFixture::new("scp file example.com:/tmp/file", &[NetworkLikely], High),
            CommandFixture::new(
                "rsync -av src/ example.com:/tmp/src",
                &[NetworkLikely],
                High,
            ),
            CommandFixture::new("nc example.com 443", &[NetworkLikely], High),
            CommandFixture::new("sh -c 'echo ok'", &[CodeExecution], High),
            CommandFixture::new("bash -c 'echo ok'", &[CodeExecution], High),
            CommandFixture::new("python -c 'print(1)'", &[CodeExecution], High),
            CommandFixture::new("node -e 'console.log(1)'", &[CodeExecution], High),
            CommandFixture::new(
                "eval rm -rf /tmp/roci-dir",
                &[CodeExecution, Unknown, DestructiveDelete],
                Low,
            )
            .with_reasons(&["matched eval command execution"]),
            CommandFixture::new(
                "eval find /tmp/roci-dir -delete",
                &[CodeExecution, Unknown, DestructiveDelete],
                Low,
            )
            .with_reasons(&["matched find destructive predicate: -delete"]),
            CommandFixture::new(
                "eval git reset --hard",
                &[CodeExecution, Unknown, WritesFilesystem],
                Low,
            )
            .with_reasons(&["matched git write subcommand: reset"]),
            CommandFixture::new(
                "eval sudo ls /root",
                &[CodeExecution, Unknown, PrivilegeEscalation, ReadOnly],
                Low,
            )
            .with_reasons(&["matched eval command execution", "wrapper detected: sudo"]),
            CommandFixture::new(
                "eval 'rm -rf /tmp/roci-dir'",
                &[CodeExecution, Unknown, DestructiveDelete],
                Low,
            )
            .with_reasons(&["matched eval command execution"]),
            CommandFixture::new(
                "eval 'sudo rm -rf /tmp/roci-dir'",
                &[
                    CodeExecution,
                    Unknown,
                    PrivilegeEscalation,
                    DestructiveDelete,
                ],
                Low,
            )
            .with_reasons(&["matched eval command execution", "wrapper detected: sudo"]),
            CommandFixture::new(
                "echo $(eval rm -rf /tmp/roci-dir)",
                &[Unknown, ReadOnly, CodeExecution, DestructiveDelete],
                Low,
            )
            .with_reasons(&["shell syntax detected: $()"]),
            CommandFixture::new(
                "echo `chmod 600 /tmp/roci-secret`",
                &[Unknown, ReadOnly, PermissionChange],
                Low,
            )
            .with_reasons(&["shell syntax detected: backticks"]),
            CommandFixture::new("cat Cargo.toml | grep roci", &[Unknown, ReadOnly], Low)
                .with_reasons(&["connector detected: |"]),
            CommandFixture::new(
                "cat Cargo.toml; rm -rf /tmp/roci-dir",
                &[Unknown, ReadOnly, DestructiveDelete],
                Low,
            )
            .with_reasons(&["connector detected: ;"]),
            CommandFixture::new(
                "cat Cargo.toml\nrm -rf /tmp/roci-dir",
                &[Unknown, ReadOnly, DestructiveDelete],
                Low,
            )
            .with_reasons(&["shell syntax detected: newline"]),
            CommandFixture::new(
                "cat Cargo.toml & rm -rf /tmp/roci-dir",
                &[Unknown, ReadOnly, DestructiveDelete],
                Low,
            )
            .with_reasons(&["connector detected: &"]),
            CommandFixture::new("cat Cargo.toml > /tmp/roci-out", &[Unknown, ReadOnly], Low)
                .with_reasons(&["shell syntax detected: redirect"]),
            CommandFixture::new(
                "tee /tmp/roci-out < Cargo.toml",
                &[Unknown, WritesFilesystem],
                Low,
            )
            .with_reasons(&["shell syntax detected: redirect"]),
        ];

        for fixture in fixtures {
            fixture.assert_matches();
        }
    }

    #[test]
    fn classifies_wrapper_and_env_command() {
        let report = classify_shell_command("FOO=bar env sudo rm -rf /tmp/demo");

        assert_eq!(report.primary_executable.as_deref(), Some("rm"));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert!(has(&report, CommandCategory::PrivilegeEscalation));
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "wrapper detected: sudo"));
        assert_eq!(report.normalized_command, "rm -rf /tmp/demo");
    }

    #[test]
    fn preserves_unknown_for_unparsed_shell_features() {
        let report = classify_shell_command("echo ok | sh");

        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::CodeExecution));
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "connector detected: |"));
    }

    #[test]
    fn unions_categories_across_connectors() {
        let report = classify_shell_command("cat Cargo.toml && curl https://example.com");

        assert!(has(&report, CommandCategory::ReadOnly));
        assert!(has(&report, CommandCategory::NetworkLikely));
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "connector detected: &&"));
    }

    #[test]
    fn classifies_git_read_only_and_write_subcommands() {
        let readonly = classify_shell_command("git status && git show HEAD");
        let writing = classify_shell_command("git commit -m test || git reset --hard");

        assert!(has(&readonly, CommandCategory::ReadOnly));
        assert!(!has(&readonly, CommandCategory::WritesFilesystem));
        assert!(has(&writing, CommandCategory::WritesFilesystem));
        assert!(writing
            .reasons
            .iter()
            .any(|reason| reason == "matched git write subcommand: commit"));
    }

    #[test]
    fn unknown_executable_lowers_confidence() {
        let report = classify_shell_command("custom-tool --flag");

        assert!(has(&report, CommandCategory::Unknown));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert_eq!(report.primary_executable.as_deref(), Some("custom-tool"));
    }

    #[test]
    fn classifies_direct_filesystem_write_command() {
        let report = classify_shell_command("touch src/new-file.rs");

        assert_eq!(report.primary_executable.as_deref(), Some("touch"));
        assert!(has(&report, CommandCategory::WritesFilesystem));
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched filesystem write executable: touch"));
    }

    #[test]
    fn classifies_permission_change_commands() {
        let chmod = classify_shell_command("chmod 600 secret.txt");
        let chown = classify_shell_command("chown root:wheel secret.txt");

        assert!(has(&chmod, CommandCategory::PermissionChange));
        assert!(has(&chown, CommandCategory::PermissionChange));
        assert!(chmod
            .reasons
            .iter()
            .any(|reason| reason == "matched permission change executable: chmod"));
    }

    #[test]
    fn classifies_process_control_commands() {
        let kill = classify_shell_command("kill 1234");
        let systemctl = classify_shell_command("systemctl restart demo.service");

        assert!(has(&kill, CommandCategory::ProcessControl));
        assert!(has(&systemctl, CommandCategory::ProcessControl));
        assert!(kill
            .reasons
            .iter()
            .any(|reason| reason == "matched process control executable: kill"));
    }

    #[test]
    fn sudo_option_value_does_not_hide_destructive_command() {
        let report = classify_shell_command("sudo -u root rm -rf /tmp/demo");

        assert_eq!(report.primary_executable.as_deref(), Some("rm"));
        assert!(has(&report, CommandCategory::PrivilegeEscalation));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.normalized_command, "rm -rf /tmp/demo");
    }

    #[test]
    fn env_option_value_does_not_hide_permission_change_command() {
        let report = classify_shell_command("env -C /tmp chmod 600 file");

        assert_eq!(report.primary_executable.as_deref(), Some("chmod"));
        assert!(has(&report, CommandCategory::PermissionChange));
        assert_eq!(report.normalized_command, "chmod 600 file");
    }

    #[test]
    fn xargs_replacement_option_does_not_hide_process_control_command() {
        let report = classify_shell_command("xargs -I {} kill {}");

        assert_eq!(report.primary_executable.as_deref(), Some("kill"));
        assert!(has(&report, CommandCategory::ProcessControl));
        assert_eq!(report.normalized_command, "kill {}");
    }

    #[test]
    fn command_substitution_marks_unknown_and_surfaces_nested_risk() {
        let report = classify_shell_command("echo $(rm -rf /tmp/demo)");

        assert_eq!(report.primary_executable.as_deref(), Some("echo"));
        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "shell syntax detected: $()"));
    }

    #[test]
    fn backticks_mark_unknown_and_surface_nested_risk() {
        let report = classify_shell_command("echo `chmod 600 file`");

        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::PermissionChange));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "shell syntax detected: backticks"));
    }

    #[test]
    fn redirect_marks_unknown() {
        let report = classify_shell_command("cat Cargo.toml > /tmp/out");

        assert!(has(&report, CommandCategory::ReadOnly));
        assert!(has(&report, CommandCategory::Unknown));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "shell syntax detected: redirect"));
    }

    #[test]
    fn newline_separator_marks_unknown_and_unions_categories() {
        let report = classify_shell_command("cat Cargo.toml\nrm -rf /tmp/demo");

        assert!(has(&report, CommandCategory::ReadOnly));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert!(has(&report, CommandCategory::Unknown));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "connector detected: newline"));
    }

    #[test]
    fn single_ampersand_separator_marks_unknown_and_unions_categories() {
        let report = classify_shell_command("cat Cargo.toml & rm -rf /tmp/demo");

        assert_eq!(report.primary_executable.as_deref(), Some("cat"));
        assert!(has(&report, CommandCategory::ReadOnly));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert!(has(&report, CommandCategory::Unknown));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "connector detected: &"));
    }

    #[test]
    fn find_delete_is_destructive() {
        let report = classify_shell_command("find /tmp/demo -delete");

        assert_eq!(report.primary_executable.as_deref(), Some("find"));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find destructive predicate: -delete"));
    }

    #[test]
    fn find_exec_marks_execution_unknown_and_surfaces_nested_delete() {
        let report = classify_shell_command(r"find . -exec rm {} \;");

        assert_eq!(report.primary_executable.as_deref(), Some("find"));
        assert!(has(&report, CommandCategory::CodeExecution));
        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find execution predicate: -exec"));
    }

    #[test]
    fn find_execdir_marks_execution_unknown_and_surfaces_nested_delete() {
        let report = classify_shell_command(r"find . -execdir rm {} \;");

        assert!(has(&report, CommandCategory::CodeExecution));
        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find execution predicate: -execdir"));
    }

    #[test]
    fn find_ok_marks_execution_unknown_and_surfaces_nested_delete() {
        let report = classify_shell_command(r"find . -ok rm {} \;");

        assert!(has(&report, CommandCategory::CodeExecution));
        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find execution predicate: -ok"));
    }

    #[test]
    fn find_okdir_marks_execution_unknown_and_surfaces_nested_delete() {
        let report = classify_shell_command(r"find . -okdir rm {} \;");

        assert!(has(&report, CommandCategory::CodeExecution));
        assert!(has(&report, CommandCategory::Unknown));
        assert!(has(&report, CommandCategory::DestructiveDelete));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find execution predicate: -okdir"));
    }

    #[test]
    fn find_fprint_writes_filesystem_and_is_unknown() {
        let report = classify_shell_command("find . -fprint /tmp/out");

        assert!(has(&report, CommandCategory::WritesFilesystem));
        assert!(has(&report, CommandCategory::Unknown));
        assert_eq!(report.confidence, CommandConfidence::Low);
        assert!(report
            .reasons
            .iter()
            .any(|reason| reason == "matched find filesystem write predicate: -fprint"));
    }

    #[test]
    fn find_fprintf_and_fls_write_filesystem_and_are_unknown() {
        let fprintf = classify_shell_command("find . -fprintf /tmp/out %p");
        let fls = classify_shell_command("find . -fls /tmp/out");
        let fprint0 = classify_shell_command("find . -fprint0 /tmp/out");

        assert!(has(&fprintf, CommandCategory::WritesFilesystem));
        assert!(has(&fprintf, CommandCategory::Unknown));
        assert!(has(&fls, CommandCategory::WritesFilesystem));
        assert!(has(&fls, CommandCategory::Unknown));
        assert!(has(&fprint0, CommandCategory::WritesFilesystem));
        assert!(has(&fprint0, CommandCategory::Unknown));
    }
}
