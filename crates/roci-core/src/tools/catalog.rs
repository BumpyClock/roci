//! Tool catalog and visibility policy.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::error::RociError;

use super::tool::{Tool, ToolPromptMetadata, ToolSafetySummary};

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

    /// Return whether a descriptor should be visible.
    #[must_use]
    pub fn allows_descriptor(&self, descriptor: &ToolDescriptor) -> bool {
        let matches = |names: &BTreeSet<String>| {
            names.contains(&descriptor.name)
                || descriptor.aliases.iter().any(|alias| names.contains(alias))
        };
        if self.no_tools || matches(&self.exclude) {
            return false;
        }
        self.allow.is_empty() || matches(&self.allow)
    }
}

/// Metadata for a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub prompt: String,
    pub prompt_metadata: ToolPromptMetadata,
    pub origin: ToolOrigin,
    pub safety: ToolSafetySummary,
}

impl ToolDescriptor {
    /// Build descriptor from a tool and origin.
    #[must_use]
    pub fn from_tool(tool: &dyn Tool, origin: ToolOrigin) -> Self {
        Self {
            name: tool.name().to_string(),
            label: tool.label().to_string(),
            description: tool.description().to_string(),
            aliases: normalized_aliases(tool),
            prompt: tool.prompt().to_string(),
            prompt_metadata: tool.prompt_metadata(),
            origin,
            safety: tool.safety_summary(),
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

fn normalized_aliases(tool: &dyn Tool) -> Vec<String> {
    let mut aliases = BTreeSet::new();
    for alias in tool.aliases() {
        if alias != tool.name() {
            aliases.insert(alias.clone());
        }
    }
    aliases.into_iter().collect()
}

fn alias_collision_error(alias: &str, owner: &str, existing_owner: &str) -> RociError {
    RociError::InvalidState(format!(
        "tool alias '{alias}' for '{owner}' collides with '{existing_owner}'"
    ))
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
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when canonical names or aliases collide.
    pub fn from_tools(tools: Vec<Arc<dyn Tool>>, origin: ToolOrigin) -> Result<Self, RociError> {
        let mut catalog = Self::new();
        for tool in tools {
            catalog.insert_first_wins(tool, origin.clone())?;
        }
        Ok(catalog)
    }

    /// Insert tool and reject duplicate names.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when canonical names or aliases collide.
    pub fn insert(&mut self, tool: Arc<dyn Tool>, origin: ToolOrigin) -> Result<(), RociError> {
        if self.contains_name(tool.name()) {
            return Err(RociError::InvalidState(format!(
                "duplicate tool name in catalog: {}",
                tool.name()
            )));
        }
        self.validate_tool_aliases(tool.as_ref(), None)?;
        self.insert_unchecked(tool, origin);
        Ok(())
    }

    /// Insert tool only if no tool with same name exists.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when canonical names or aliases collide.
    pub fn insert_first_wins(
        &mut self,
        tool: Arc<dyn Tool>,
        origin: ToolOrigin,
    ) -> Result<bool, RociError> {
        if !self.contains_name(tool.name()) {
            self.validate_tool_aliases(tool.as_ref(), None)?;
            self.insert_unchecked(tool, origin);
            return Ok(true);
        }
        Ok(false)
    }

    /// Insert or replace tool while preserving original order for that name.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when aliases collide with another tool.
    pub fn insert_or_replace(
        &mut self,
        tool: Arc<dyn Tool>,
        origin: ToolOrigin,
    ) -> Result<(), RociError> {
        self.validate_tool_aliases(tool.as_ref(), Some(tool.name()))?;
        let descriptor = ToolDescriptor::from_tool(tool.as_ref(), origin);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.descriptor.name == descriptor.name)
        {
            entry.descriptor = descriptor;
            entry.tool = tool;
            return Ok(());
        }
        self.entries.push(ToolCatalogEntry { descriptor, tool });
        Ok(())
    }

    /// Add another catalog. Existing names keep first entry.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] when inserted entries would collide.
    pub fn extend_first_wins(&mut self, other: Self) -> Result<(), RociError> {
        let entries_to_insert = other
            .entries
            .into_iter()
            .filter(|entry| !self.contains_name(&entry.descriptor.name))
            .collect::<Vec<_>>();
        self.validate_entry_batch(&entries_to_insert)?;
        self.entries.extend(entries_to_insert);
        Ok(())
    }

    fn validate_tool_aliases(
        &self,
        tool: &dyn Tool,
        replacing_name: Option<&str>,
    ) -> Result<(), RociError> {
        let descriptor = ToolDescriptor::from_tool(tool, ToolOrigin::Custom);
        self.validate_descriptor(&descriptor, replacing_name)
    }

    fn validate_descriptor(
        &self,
        descriptor: &ToolDescriptor,
        replacing_name: Option<&str>,
    ) -> Result<(), RociError> {
        for entry in &self.entries {
            if replacing_name == Some(entry.descriptor.name.as_str()) {
                continue;
            }
            if entry.descriptor.name == descriptor.name {
                return Err(RociError::InvalidState(format!(
                    "duplicate tool name in catalog: {}",
                    descriptor.name
                )));
            }
            if entry
                .descriptor
                .aliases
                .iter()
                .any(|alias| alias == &descriptor.name)
            {
                return Err(alias_collision_error(
                    &descriptor.name,
                    &descriptor.name,
                    &entry.descriptor.name,
                ));
            }
            for alias in &descriptor.aliases {
                if alias == &entry.descriptor.name
                    || entry
                        .descriptor
                        .aliases
                        .iter()
                        .any(|existing_alias| existing_alias == alias)
                {
                    return Err(alias_collision_error(
                        alias,
                        &descriptor.name,
                        &entry.descriptor.name,
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_entry_batch(&self, entries: &[ToolCatalogEntry]) -> Result<(), RociError> {
        for entry in entries {
            self.validate_descriptor(&entry.descriptor, None)?;
        }

        let mut names = BTreeMap::<String, String>::new();
        for entry in entries {
            let name = &entry.descriptor.name;
            if names.insert(name.clone(), name.clone()).is_some() {
                return Err(RociError::InvalidState(format!(
                    "duplicate tool name in catalog: {name}"
                )));
            }
        }

        for entry in entries {
            for alias in &entry.descriptor.aliases {
                if let Some(existing_owner) = names.get(alias) {
                    return Err(alias_collision_error(
                        alias,
                        &entry.descriptor.name,
                        existing_owner,
                    ));
                }
            }
        }

        let mut aliases = BTreeMap::<String, String>::new();
        for entry in entries {
            for alias in &entry.descriptor.aliases {
                if let Some(existing_owner) =
                    aliases.insert(alias.clone(), entry.descriptor.name.clone())
                {
                    return Err(alias_collision_error(
                        alias,
                        &entry.descriptor.name,
                        &existing_owner,
                    ));
                }
            }
        }
        Ok(())
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
            .filter(|entry| policy.allows_descriptor(&entry.descriptor))
            .map(|entry| Arc::clone(&entry.tool))
            .collect()
    }

    /// Resolve visible descriptors according to policy.
    #[must_use]
    pub fn resolve_descriptors(&self, policy: &ToolVisibilityPolicy) -> Vec<ToolDescriptor> {
        self.entries
            .iter()
            .filter(|entry| policy.allows_descriptor(&entry.descriptor))
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
pub fn catalog_from_groups(
    groups: impl IntoIterator<Item = (ToolOrigin, Vec<Arc<dyn Tool>>)>,
) -> Result<ToolCatalog, RociError> {
    let mut catalog = ToolCatalog::new();
    for (origin, tools) in groups {
        catalog.extend_first_wins(ToolCatalog::from_tools(tools, origin)?)?;
    }
    Ok(catalog)
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
    use crate::tools::{
        AgentTool, AgentToolParameters, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary,
    };

    fn safety_summary(
        read_only_by_default: bool,
        destructive_by_default: bool,
        concurrency_safe_by_default: bool,
        approval_kind: ToolSafetyKind,
    ) -> ToolSafetySummary {
        ToolSafetySummary {
            read_only_by_default,
            destructive_by_default,
            concurrency_safe_by_default,
            approval_kind,
        }
    }

    fn read_only_summary() -> ToolSafetySummary {
        safety_summary(true, false, true, ToolSafetyKind::Read)
    }

    fn approval_required_summary(kind: ToolSafetyKind) -> ToolSafetySummary {
        safety_summary(false, false, false, kind)
    }

    fn test_tool(name: &str, plan: ToolSafetyPlan, summary: ToolSafetySummary) -> Arc<dyn Tool> {
        Arc::new(
            AgentTool::new(
                name,
                format!("{name} description"),
                AgentToolParameters::empty(),
                |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
            )
            .with_static_safety(plan, summary),
        )
    }

    fn test_tool_with_aliases(name: &str, aliases: &[&str]) -> Arc<dyn Tool> {
        Arc::new(
            AgentTool::new(
                name,
                format!("{name} description"),
                AgentToolParameters::empty(),
                |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
            )
            .with_aliases(aliases.iter().copied()),
        )
    }

    #[test]
    fn policy_filters_allow_exclude_and_no_tools() {
        let catalog = ToolCatalog::from_tools(
            vec![
                test_tool(
                    "read_file",
                    ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
                    read_only_summary(),
                ),
                test_tool(
                    "write_file",
                    ToolSafetyPlan::approval_required(ToolSafetyKind::FileChange),
                    approval_required_summary(ToolSafetyKind::FileChange),
                ),
            ],
            ToolOrigin::Builtin,
        )
        .unwrap();

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
                test_tool(
                    "read",
                    ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
                    read_only_summary(),
                ),
                test_tool(
                    "read",
                    ToolSafetyPlan::approval_required(ToolSafetyKind::Other),
                    approval_required_summary(ToolSafetyKind::Other),
                ),
            ],
            ToolOrigin::Custom,
        )
        .unwrap();

        let descriptors = catalog.descriptors();

        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].safety.approval_kind, ToolSafetyKind::Read);
        assert!(descriptors[0].safety.read_only_by_default);
        assert!(!descriptors[0].safety.destructive_by_default);
        assert!(descriptors[0].safety.concurrency_safe_by_default);
    }

    #[test]
    fn prompt_metadata_descriptor_keeps_prompt_separate_from_description() {
        let metadata = ToolPromptMetadata {
            guidelines: vec!["Prefer exact reads.".to_string()],
            search_hint: Some("future-search-only".to_string()),
        };
        let tool = AgentTool::new(
            "read",
            "UI description",
            AgentToolParameters::empty(),
            |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
        )
        .with_prompt("Model prompt")
        .with_prompt_metadata(metadata.clone());

        let descriptor = ToolDescriptor::from_tool(&tool, ToolOrigin::Custom);

        assert_eq!(descriptor.description, "UI description");
        assert_eq!(descriptor.prompt, "Model prompt");
        assert_eq!(descriptor.prompt_metadata, metadata);
    }

    #[test]
    fn insert_or_replace_preserves_position_and_replaces_metadata() {
        let mut catalog = ToolCatalog::new();
        catalog
            .insert(
                test_tool(
                    "read",
                    ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
                    read_only_summary(),
                ),
                ToolOrigin::Builtin,
            )
            .unwrap();
        catalog
            .insert(
                test_tool(
                    "write",
                    ToolSafetyPlan::approval_required(ToolSafetyKind::FileChange),
                    approval_required_summary(ToolSafetyKind::FileChange),
                ),
                ToolOrigin::Builtin,
            )
            .unwrap();

        catalog
            .insert_or_replace(
                test_tool(
                    "read",
                    ToolSafetyPlan::approval_required(ToolSafetyKind::Other),
                    approval_required_summary(ToolSafetyKind::Other),
                ),
                ToolOrigin::Dynamic,
            )
            .unwrap();

        let descriptors = catalog.descriptors();
        assert_eq!(descriptors[0].name, "read");
        assert_eq!(descriptors[0].safety.approval_kind, ToolSafetyKind::Other);
        assert!(!descriptors[0].safety.read_only_by_default);
        assert!(!descriptors[0].safety.destructive_by_default);
        assert!(!descriptors[0].safety.concurrency_safe_by_default);
        assert_eq!(descriptors[1].name, "write");
    }

    #[test]
    fn catalog_alias_self_alias_is_ignored_and_same_tool_aliases_are_deduped() {
        let catalog = ToolCatalog::from_tools(
            vec![test_tool_with_aliases(
                "read",
                &["old_read", "read", "old_read"],
            )],
            ToolOrigin::Custom,
        )
        .unwrap();

        let descriptors = catalog.descriptors();

        assert_eq!(descriptors[0].aliases, vec!["old_read"]);
    }

    #[test]
    fn catalog_alias_rejects_canonical_alias_and_alias_alias_collisions() {
        let canonical_collision = ToolCatalog::from_tools(
            vec![
                test_tool_with_aliases("read", &["legacy_read"]),
                test_tool_with_aliases("legacy_read", &[]),
            ],
            ToolOrigin::Custom,
        );
        assert!(matches!(
            canonical_collision,
            Err(RociError::InvalidState(message)) if message.contains("legacy_read")
        ));

        let alias_collision = ToolCatalog::from_tools(
            vec![
                test_tool_with_aliases("read", &["legacy"]),
                test_tool_with_aliases("scan", &["legacy"]),
            ],
            ToolOrigin::Custom,
        );
        assert!(matches!(
            alias_collision,
            Err(RociError::InvalidState(message)) if message.contains("legacy")
        ));
    }

    #[test]
    fn catalog_alias_matching_is_case_sensitive() {
        let catalog = ToolCatalog::from_tools(
            vec![
                test_tool_with_aliases("read", &["legacy"]),
                test_tool_with_aliases("scan", &["Legacy"]),
            ],
            ToolOrigin::Custom,
        )
        .unwrap();

        assert_eq!(catalog.descriptors().len(), 2);
    }

    #[test]
    fn catalog_alias_insert_first_wins_keeps_existing_canonical_without_validating_dropped_tool() {
        let mut catalog = ToolCatalog::new();
        catalog
            .insert_first_wins(test_tool_with_aliases("read", &[]), ToolOrigin::Custom)
            .unwrap();
        catalog
            .insert_first_wins(
                test_tool_with_aliases("write", &["legacy"]),
                ToolOrigin::Custom,
            )
            .unwrap();
        let inserted = catalog
            .insert_first_wins(
                test_tool_with_aliases("read", &["legacy"]),
                ToolOrigin::Custom,
            )
            .unwrap();

        assert!(!inserted);
        assert_eq!(catalog.descriptors().len(), 2);
    }

    #[test]
    fn catalog_alias_insert_or_replace_validates_against_other_tools() {
        let mut catalog = ToolCatalog::from_tools(
            vec![
                test_tool_with_aliases("read", &["legacy_read"]),
                test_tool_with_aliases("write", &["legacy_write"]),
            ],
            ToolOrigin::Custom,
        )
        .unwrap();

        let err = catalog
            .insert_or_replace(
                test_tool_with_aliases("read", &["legacy_write"]),
                ToolOrigin::Dynamic,
            )
            .unwrap_err();

        assert!(
            matches!(err, RociError::InvalidState(message) if message.contains("legacy_write"))
        );
        assert_eq!(catalog.descriptors()[0].aliases, vec!["legacy_read"]);
    }

    #[test]
    fn catalog_alias_grouped_catalog_rejects_cross_group_collision() {
        let result = catalog_from_groups([
            (
                ToolOrigin::Builtin,
                vec![test_tool_with_aliases("read", &["legacy_read"])],
            ),
            (
                ToolOrigin::Dynamic,
                vec![test_tool_with_aliases("write", &["legacy_read"])],
            ),
        ]);

        assert!(matches!(result, Err(RociError::InvalidState(_))));
    }

    #[test]
    fn catalog_alias_visibility_matches_aliases_and_exclude_wins() {
        let catalog = ToolCatalog::from_tools(
            vec![test_tool_with_aliases("read", &["legacy_read"])],
            ToolOrigin::Custom,
        )
        .unwrap();

        let allow = ToolVisibilityPolicy::allow_only(["legacy_read"]);
        assert_eq!(catalog.resolve_descriptors(&allow)[0].name, "read");

        let mut allow_and_exclude = ToolVisibilityPolicy::allow_only(["read"]);
        allow_and_exclude.extend_exclude(["legacy_read"]);
        assert!(catalog.resolve_descriptors(&allow_and_exclude).is_empty());
    }

    #[test]
    fn catalog_alias_canonical_output_stays_canonical_when_visible_by_alias() {
        let catalog = ToolCatalog::from_tools(
            vec![test_tool_with_aliases("read", &["legacy_read"])],
            ToolOrigin::Custom,
        )
        .unwrap();

        let tools = catalog.resolve(&ToolVisibilityPolicy::allow_only(["legacy_read"]));

        assert_eq!(tools[0].name(), "read");
    }
}
