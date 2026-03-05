use std::time::Duration;

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
