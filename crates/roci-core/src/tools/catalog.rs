//! Tool catalog and visibility policy.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::error::RociError;

use super::tool::{Tool, ToolApproval};

/// Origin for a tool exposed to an agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOrigin {
    Builtin,
    Dynamic,
    Mcp { server_id: String },
    Custom,
}

/// Host/runtime policy for which tools are visible to a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolVisibilityPolicy {
    no_tools: bool,
    allow: BTreeSet<String>,
    exclude: BTreeSet<String>,
}

impl ToolVisibilityPolicy {
    /// Build policy that hides every tool.
    #[must_use]
    pub fn no_tools() -> Self {
        Self {
            no_tools: true,
            allow: BTreeSet::new(),
            exclude: BTreeSet::new(),
        }
    }

    /// Build policy that allows only listed tool names.
    #[must_use]
    pub fn allow_only(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            no_tools: false,
            allow: names.into_iter().map(Into::into).collect(),
            exclude: BTreeSet::new(),
        }
    }

    /// Build policy that excludes listed tool names.
    #[must_use]
    pub fn exclude(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            no_tools: false,
            allow: BTreeSet::new(),
            exclude: names.into_iter().map(Into::into).collect(),
        }
    }

    /// Add allowed tool names.
    pub fn extend_allow(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        self.allow.extend(names.into_iter().map(Into::into));
    }

    /// Add excluded tool names.
    pub fn extend_exclude(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        self.exclude.extend(names.into_iter().map(Into::into));
    }

    /// Hide every tool.
    pub fn set_no_tools(&mut self, no_tools: bool) {
        self.no_tools = no_tools;
    }

    /// Whether every tool is hidden.
    #[must_use]
    pub const fn is_no_tools(&self) -> bool {
        self.no_tools
    }

    /// Allowed-name set. Empty means all names allowed unless excluded.
    ///
    /// Precedence: [`Self::no_tools`] hides every tool, then exclusions hide
    /// matching names, then non-empty allow set admits only matching names.
    #[must_use]
    pub const fn allow(&self) -> &BTreeSet<String> {
        &self.allow
    }

    /// Excluded-name set. Exclusions win over allow entries with same name.
    #[must_use]
    pub const fn exclude_names(&self) -> &BTreeSet<String> {
        &self.exclude
    }

    /// Return whether `name` should be visible.
    #[must_use]
    pub fn allows(&self, name: &str) -> bool {
        if self.no_tools || self.exclude.contains(name) {
            return false;
        }
        self.allow.is_empty() || self.allow.contains(name)
    }
}

/// Metadata for a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    pub origin: ToolOrigin,
    pub approval: ToolApproval,
}

impl ToolDescriptor {
    /// Build descriptor from a tool and origin.
    #[must_use]
    pub fn from_tool(tool: &dyn Tool, origin: ToolOrigin) -> Self {
        Self {
            name: tool.name().to_string(),
            label: tool.label().to_string(),
            description: tool.description().to_string(),
            origin,
            approval: tool.approval(),
        }
    }
}

#[derive(Clone)]
struct ToolCatalogEntry {
    descriptor: ToolDescriptor,
    tool: Arc<dyn Tool>,
}

/// Deterministic catalog for resolving model-visible tools.
///
/// Duplicate-name behavior is explicit by insertion API:
/// - [`ToolCatalog::insert`] rejects duplicates.
/// - [`ToolCatalog::insert_first_wins`] keeps existing entry.
/// - [`ToolCatalog::insert_or_replace`] replaces metadata and implementation
///   in original position.
#[derive(Clone, Default)]
pub struct ToolCatalog {
    entries: Vec<ToolCatalogEntry>,
}

impl ToolCatalog {
    /// Create empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Build catalog from tools. Duplicate names keep first entry.
    #[must_use]
    pub fn from_tools(tools: Vec<Arc<dyn Tool>>, origin: ToolOrigin) -> Self {
        let mut catalog = Self::new();
        for tool in tools {
            catalog.insert_first_wins(tool, origin.clone());
        }
        catalog
    }

    /// Insert tool and reject duplicate names.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when `tool.name()` already exists.
    pub fn insert(&mut self, tool: Arc<dyn Tool>, origin: ToolOrigin) -> Result<(), RociError> {
        if self.contains_name(tool.name()) {
            return Err(RociError::InvalidState(format!(
                "duplicate tool name in catalog: {}",
                tool.name()
            )));
        }
        self.insert_unchecked(tool, origin);
        Ok(())
    }

    /// Insert tool only if no tool with same name exists.
    pub fn insert_first_wins(&mut self, tool: Arc<dyn Tool>, origin: ToolOrigin) {
        if !self.contains_name(tool.name()) {
            self.insert_unchecked(tool, origin);
        }
    }

    /// Insert or replace tool while preserving original order for that name.
    pub fn insert_or_replace(&mut self, tool: Arc<dyn Tool>, origin: ToolOrigin) {
        let descriptor = ToolDescriptor::from_tool(tool.as_ref(), origin);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.descriptor.name == descriptor.name)
        {
            entry.descriptor = descriptor;
            entry.tool = tool;
            return;
        }
        self.entries.push(ToolCatalogEntry { descriptor, tool });
    }

    /// Add another catalog. Existing names keep first entry.
    pub fn extend_first_wins(&mut self, other: Self) {
        for entry in other.entries {
            if !self.contains_name(&entry.descriptor.name) {
                self.entries.push(entry);
            }
        }
    }

    /// Return descriptors in resolution order.
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.entries
            .iter()
            .map(|entry| entry.descriptor.clone())
            .collect()
    }

    /// Resolve visible tools according to policy.
    #[must_use]
    pub fn resolve(&self, policy: &ToolVisibilityPolicy) -> Vec<Arc<dyn Tool>> {
        self.entries
            .iter()
            .filter(|entry| policy.allows(&entry.descriptor.name))
            .map(|entry| Arc::clone(&entry.tool))
            .collect()
    }

    /// Resolve visible descriptors according to policy.
    #[must_use]
    pub fn resolve_descriptors(&self, policy: &ToolVisibilityPolicy) -> Vec<ToolDescriptor> {
        self.entries
            .iter()
            .filter(|entry| policy.allows(&entry.descriptor.name))
            .map(|entry| entry.descriptor.clone())
            .collect()
    }

    fn contains_name(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.descriptor.name == name)
    }

    fn insert_unchecked(&mut self, tool: Arc<dyn Tool>, origin: ToolOrigin) {
        self.entries.push(ToolCatalogEntry {
            descriptor: ToolDescriptor::from_tool(tool.as_ref(), origin),
            tool,
        });
    }
}

/// Build a catalog from named groups. Later groups do not override earlier groups.
#[must_use]
pub fn catalog_from_groups(
    groups: impl IntoIterator<Item = (ToolOrigin, Vec<Arc<dyn Tool>>)>,
) -> ToolCatalog {
    let mut catalog = ToolCatalog::new();
    for (origin, tools) in groups {
        catalog.extend_first_wins(ToolCatalog::from_tools(tools, origin));
    }
    catalog
}

/// Return counts by origin for diagnostics.
#[must_use]
pub fn count_by_origin(descriptors: &[ToolDescriptor]) -> HashMap<&'static str, usize> {
    let mut counts = HashMap::new();
    for descriptor in descriptors {
        let key = match &descriptor.origin {
            ToolOrigin::Builtin => "builtin",
            ToolOrigin::Dynamic => "dynamic",
            ToolOrigin::Mcp { .. } => "mcp",
            ToolOrigin::Custom => "custom",
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{AgentTool, AgentToolParameters, ToolApproval, ToolApprovalKind};

    fn test_tool(name: &str, approval: ToolApproval) -> Arc<dyn Tool> {
        Arc::new(
            AgentTool::new(
                name,
                format!("{name} description"),
                AgentToolParameters::empty(),
                |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
            )
            .with_approval(approval),
        )
    }

    #[test]
    fn policy_filters_allow_exclude_and_no_tools() {
        let catalog = ToolCatalog::from_tools(
            vec![
                test_tool("read_file", ToolApproval::safe_read_only()),
                test_tool(
                    "write_file",
                    ToolApproval::requires_approval(ToolApprovalKind::FileChange),
                ),
            ],
            ToolOrigin::Builtin,
        );

        let allow = ToolVisibilityPolicy::allow_only(["read_file"]);
        assert_eq!(catalog.resolve_descriptors(&allow)[0].name, "read_file");

        let exclude = ToolVisibilityPolicy::exclude(["write_file"]);
        assert_eq!(catalog.resolve_descriptors(&exclude)[0].name, "read_file");

        assert!(catalog
            .resolve_descriptors(&ToolVisibilityPolicy::no_tools())
            .is_empty());
    }

    #[test]
    fn duplicate_names_keep_first_by_default() {
        let catalog = ToolCatalog::from_tools(
            vec![
                test_tool("read", ToolApproval::safe_read_only()),
                test_tool(
                    "read",
                    ToolApproval::requires_approval(ToolApprovalKind::Other),
                ),
            ],
            ToolOrigin::Custom,
        );

        let descriptors = catalog.descriptors();

        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].approval, ToolApproval::safe_read_only());
    }

    #[test]
    fn insert_or_replace_preserves_position_and_replaces_metadata() {
        let mut catalog = ToolCatalog::new();
        catalog
            .insert(
                test_tool("read", ToolApproval::safe_read_only()),
                ToolOrigin::Builtin,
            )
            .unwrap();
        catalog
            .insert(
                test_tool(
                    "write",
                    ToolApproval::requires_approval(ToolApprovalKind::FileChange),
                ),
                ToolOrigin::Builtin,
            )
            .unwrap();

        catalog.insert_or_replace(
            test_tool(
                "read",
                ToolApproval::requires_approval(ToolApprovalKind::Other),
            ),
            ToolOrigin::Dynamic,
        );

        let descriptors = catalog.descriptors();
        assert_eq!(descriptors[0].name, "read");
        assert_eq!(
            descriptors[0].approval,
            ToolApproval::requires_approval(ToolApprovalKind::Other)
        );
        assert_eq!(descriptors[1].name, "write");
    }
}
