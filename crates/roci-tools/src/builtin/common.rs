use std::time::Duration;

use roci::error::RociError;
use roci::prelude::LogicalPath;
use roci::tools::tool::ToolExecutionContext;

pub(super) const SHELL_OUTPUT_MAX_BYTES: usize = 32_768;
pub(super) const READ_FILE_MAX_BYTES: usize = 65_536;
pub(super) const GREP_OUTPUT_MAX_BYTES: usize = 32_768;
pub(super) const SHELL_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let mut cutoff = max_bytes;
    while cutoff > 0 && !s.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    s[..cutoff].to_string()
}

pub(super) fn resolve_session_path(
    ctx: &ToolExecutionContext,
    raw_path: &str,
) -> Result<Option<LogicalPath>, RociError> {
    let Some(cwd) = ctx.session_cwd.as_ref() else {
        return Ok(None);
    };
    let path = cwd.join(raw_path).map_err(|err| RociError::ToolExecution {
        tool_name: ctx.tool_name.clone().unwrap_or_else(|| "tool".to_string()),
        message: err.to_string(),
    })?;
    Ok(Some(path))
}

pub(super) fn validate_session_shell_command(command: &str) -> Result<(), String> {
    let trimmed = command.trim_start();
    if trimmed.starts_with('/') {
        return Err("command starts with absolute path".to_string());
    }
    let denied_prefixes = ["--output=/"];
    if let Some(pattern) = denied_prefixes
        .iter()
        .find(|pattern| has_shell_token_with_prefix(command, pattern))
    {
        return Err(format!("matched denied pattern `{pattern}`"));
    }
    let denied_commands = ["sudo", "chmod", "chown"];
    if let Some(pattern) = denied_commands
        .iter()
        .find(|pattern| has_shell_token(command, pattern))
    {
        return Err(format!("matched denied command `{pattern}`"));
    }
    let denied_substrings = [
        " /", "\t/", "../", " cd /", "cd /", "> /", ">/", ">> /", ">>/", "2> /", "2>/", "< /",
        "</", "rm -rf",
    ];
    if let Some(pattern) = denied_substrings
        .iter()
        .find(|pattern| command.contains(*pattern))
    {
        return Err(format!("matched denied pattern `{pattern}`"));
    }
    Ok(())
}

fn has_shell_token(command: &str, denied: &str) -> bool {
    shell_tokens(command).any(|token| token == denied)
}

fn has_shell_token_with_prefix(command: &str, denied_prefix: &str) -> bool {
    shell_tokens(command).any(|token| token.starts_with(denied_prefix))
}

fn shell_tokens(command: &str) -> impl Iterator<Item = &str> {
    command.split(|ch: char| ch.is_ascii_whitespace() || matches!(ch, ';' | '&' | '|' | '(' | ')'))
}
