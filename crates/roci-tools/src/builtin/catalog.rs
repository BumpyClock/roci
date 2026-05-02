//! Catalog for built-in coding tools.

use roci::tools::{ToolCatalog, ToolOrigin};

/// Return all built-in tools as a catalog with fixed builtin names.
///
/// Visibility filtering happens later through request/runtime policy.
#[must_use]
pub fn tool_catalog() -> ToolCatalog {
    let mut catalog = ToolCatalog::new();
    catalog.insert_first_wins(super::shell_tool(), ToolOrigin::Builtin);
    catalog.insert_first_wins(super::read_file_tool(), ToolOrigin::Builtin);
    catalog.insert_first_wins(super::write_file_tool(), ToolOrigin::Builtin);
    catalog.insert_first_wins(super::list_directory_tool(), ToolOrigin::Builtin);
    catalog.insert_first_wins(super::grep_tool(), ToolOrigin::Builtin);
    catalog.insert_first_wins(super::ask_user_tool(), ToolOrigin::Builtin);
    catalog
}
