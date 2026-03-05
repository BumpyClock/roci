//! Built-in coding tools for the CLI agent.
//!
//! Provides standard tools (`shell`, `read_file`, `write_file`, `list_directory`,
//! `grep`, `ask_user`) that a coding agent can use to interact with the local filesystem and
//! execute commands. Each tool is constructed via [`AgentTool::new`] and returned
//! as `Arc<dyn Tool>`.
//!
//! # Usage
//!
//! ```rust,no_run
//! use roci_tools::builtin::all_tools;
//!
//! let tools = all_tools();
//! assert_eq!(tools.len(), 6);
//! ```

mod ask_user;
mod common;
mod grep;
mod list_directory;
mod read_file;
mod shell;
mod write_file;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use roci::tools::tool::Tool;

pub use self::ask_user::ask_user_tool;
pub use self::grep::grep_tool;
pub use self::list_directory::list_directory_tool;
pub use self::read_file::read_file_tool;
pub use self::shell::shell_tool;
pub use self::write_file::write_file_tool;

/// Return all built-in coding tools.
pub fn all_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        shell_tool(),
        read_file_tool(),
        write_file_tool(),
        list_directory_tool(),
        grep_tool(),
        ask_user_tool(),
    ]
}
