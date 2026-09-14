//! Account-catalog-derived Cursor model families and deterministic variant routing.
//!
//! Semantics adapted from CLIProxyAPIPlus PR 235 (MIT); see LICENSE.protocol.
//! No variant is invented and credentials are never rotated to satisfy options.

use roci_core::error::RociError;
use roci_core::models::{
    ModelCapabilities, ModelCatalog, ModelCatalogSource, ModelInfo, ModelPolicy,
};
use roci_core::types::{
    GenerationSettings, GenerationSpeed, OpenAiServiceTier, ReasoningEffort, ThinkingMode,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
struct Variant<'a> {
    id: &'a str,
    effort: &'a str,
    thinking: bool,
    fast: bool,
}

fn variant(id: &str) -> (&str, Variant<'_>) {
    let mut result = Variant {
        id,
        effort: "",
        thinking: false,
        fast: false,
    };
    let mut base = id;
    while let Some((prefix, suffix)) = base.rsplit_once('-') {
        let mut next = prefix;
        match suffix {
            "fast" => result.fast = true,
            "thinking" => result.thinking = true,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
                if !result.effort.is_empty() {
                    break;
                }
                result.effort = suffix;
                if suffix == "high" {
                    if let Some(prefix) = prefix.strip_suffix("-extra") {
                        result.effort = "xhigh";
                        next = prefix;
                    }
                }
            }
            _ => break,
        }
        base = next;
    }
    (base, result)
}

fn split_suffix(model: &str) -> (&str, Option<&str>) {
    model
        .strip_suffix(')')
        .and_then(|name| name.rsplit_once('('))
        .filter(|(name, _)| !name.is_empty())
        .map_or((model, None), |(name, suffix)| (name, Some(suffix)))
}

/// Explicit upstream variant names keep their selected effort/thinking/speed.
pub fn is_explicit_variant(model: &str) -> bool {
    let (model, _) = split_suffix(model);
    variant(model).0 != model
}

fn groups(ids: &[String]) -> BTreeMap<&str, Vec<Variant<'_>>> {
    let mut groups: BTreeMap<&str, Vec<Variant<'_>>> = BTreeMap::new();
    for id in ids
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect::<BTreeSet<_>>()
    {
        let (family, variant) = variant(id);
        groups.entry(family).or_default().push(variant);
    }
    groups
}

fn budget_effort(budget: u64) -> &'static str {
    match budget {
        0 => "none",
        1..=512 => "minimal",
        513..=1024 => "low",
        1025..=8192 => "medium",
        8193..=24576 => "high",
        _ => "xhigh",
    }
}

fn suffix_effort(suffix: &str) -> Option<String> {
    let value = suffix.trim().to_ascii_lowercase();
    match value.as_str() {
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "auto" => Some(value),
        "-1" => Some("auto".into()),
        _ => value.parse::<u64>().ok().map(|n| budget_effort(n).into()),
    }
}

fn unsupported(message: impl Into<String>) -> RociError {
    RociError::UnsupportedOperation(message.into())
}

/// Reject unsupported controls before catalog or generation network requests.
pub(super) fn validate_settings(settings: &GenerationSettings) -> Result<(), RociError> {
    let mut value =
        serde_json::to_value(settings).map_err(|_| unsupported("invalid Cursor settings"))?;
    let object = value
        .as_object_mut()
        .expect("generation settings serialize as an object");
    for key in ["reasoning_effort", "speed", "stream_idle_timeout_ms"] {
        object.remove(key);
    }
    for (group, allowed) in [
        ("anthropic", "thinking"),
        ("openai_responses", "service_tier"),
    ] {
        if let Some(group) = object
            .get_mut(group)
            .and_then(serde_json::Value::as_object_mut)
        {
            group.remove(allowed);
        }
    }
    for (name, value) in object {
        if !value.is_null()
            && !value
                .as_object()
                .is_some_and(|group| group.values().all(serde_json::Value::is_null))
        {
            return Err(unsupported(format!(
                "Cursor does not support generation setting '{name}'"
            )));
        }
    }
    Ok(())
}

fn fast(settings: &GenerationSettings) -> Result<bool, RociError> {
    let tier = match settings
        .openai_responses
        .as_ref()
        .and_then(|o| o.service_tier)
    {
        None | Some(OpenAiServiceTier::Auto) => None,
        Some(OpenAiServiceTier::Default) => Some(false),
        Some(OpenAiServiceTier::Priority) => Some(true),
        Some(OpenAiServiceTier::Flex) => {
            return Err(unsupported("Cursor does not support service_tier=flex"))
        }
    };
    let speed = settings.speed.map(|speed| speed == GenerationSpeed::Fast);
    if speed.zip(tier).is_some_and(|(speed, tier)| speed != tier) {
        return Err(unsupported("Cursor speed conflicts with service_tier"));
    }
    Ok(speed.or(tier).unwrap_or(false))
}

/// Resolve a family using only this account's actual advertised upstream IDs.
/// Explicit variants ignore family selection controls, matching the reference.
pub fn resolve(
    model: &str,
    settings: &GenerationSettings,
    ids: &[String],
) -> Result<String, RociError> {
    let (name, suffix) = split_suffix(model);
    if is_explicit_variant(model) {
        return Ok(name.into());
    }
    let families = groups(ids);
    let Some(variants) = families.get(name) else {
        return Ok(model.into());
    };
    let has_effort = variants.iter().any(|v| !v.effort.is_empty());
    let has_thinking = variants.iter().any(|v| v.thinking);
    let fast = fast(settings)?;
    let suffix = suffix.and_then(suffix_effort);
    let mut effort = suffix
        .clone()
        .or_else(|| settings.reasoning_effort.map(|e| e.to_string()))
        .unwrap_or_default();
    let mut thinking = None;
    if suffix.is_none() {
        match settings
            .anthropic
            .as_ref()
            .and_then(|a| a.thinking.as_ref())
        {
            Some(ThinkingMode::Disabled) => thinking = Some(false),
            Some(ThinkingMode::Enabled { budget_tokens }) => {
                thinking = Some(true);
                // The SDK requires a budget for Enabled. Thinking-only Cursor
                // families have no budget/effort dimension, only an on/off ID.
                if effort.is_empty() && has_effort {
                    effort = budget_effort(u64::from(*budget_tokens)).into();
                }
            }
            None => {}
        }
    }
    if !has_effort && !has_thinking {
        effort.clear();
        thinking = None;
    }
    if effort == "auto" {
        effort.clear();
    }
    if !matches!(
        effort.as_str(),
        "" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        return Err(unsupported(format!(
            "Cursor model {model} does not support reasoning effort {effort}"
        )));
    }
    if effort.is_empty() && thinking.is_none() && !fast && variants.iter().any(|v| v.id == name) {
        return Ok(name.into());
    }
    let mut want_thinking = has_thinking;
    if thinking == Some(false) || effort == "none" {
        want_thinking = false;
        if thinking == Some(false) && !has_thinking {
            effort = "none".into();
        }
    }
    if thinking == Some(true) {
        if effort == "none" {
            return Err(unsupported(
                "Cursor enabled thinking conflicts with effort none",
            ));
        }
        want_thinking = has_thinking;
    }
    let score = |effort: &str| match effort {
        "medium" => 7,
        "high" => 6,
        "" => 5,
        "low" => 4,
        "minimal" => 3,
        "xhigh" => 2,
        "max" => 1,
        _ => 0,
    };
    let selected = variants
        .iter()
        .filter(|v| {
            v.fast == fast
                && v.thinking == want_thinking
                && (effort.is_empty() || effort == "none" || v.effort == effort)
                && (effort != "none" || has_thinking || matches!(v.effort, "none" | ""))
        })
        .min_by(|a, b| {
            score(b.effort)
                .cmp(&score(a.effort))
                .then_with(|| a.id.cmp(b.id))
        });
    selected.map(|v| v.id.into()).ok_or_else(|| unsupported(format!(
        "Cursor model {model} has no advertised variant in this account for effort={effort:?}, thinking={want_thinking}, speed={}",
        if fast { "fast" } else { "standard" }
    )))
}

/// Preserve upstream IDs and add family entries/options derived from the account catalog.
pub fn catalog(ids: &[String], capabilities: &ModelCapabilities) -> ModelCatalog {
    let info = |id: &str| ModelInfo {
        provider_key: "cursor".into(),
        model_id: id.into(),
        display_name: Some(id.into()),
        capabilities: capabilities.clone(),
        policy: ModelPolicy {
            requires_credentials: true,
            local: false,
            deprecated: false,
            default_for_provider: false,
        },
        source: ModelCatalogSource::Dynamic {
            endpoint: "cursor:GetUsableModels".into(),
        },
        metadata: Default::default(),
    };
    let mut catalog = ModelCatalog::from_models(ids.iter().map(|id| {
        let mut model = info(id);
        let (family, variant) = variant(id);
        model.capabilities.supports_reasoning =
            variant.thinking || (!variant.effort.is_empty() && variant.effort != "none");
        model.capabilities.supported_speeds = vec![if variant.fast {
            GenerationSpeed::Fast
        } else {
            GenerationSpeed::Standard
        }];
        model
            .metadata
            .insert("cursor_family_id".into(), json!(family));
        model
            .metadata
            .insert("cursor_explicit_variant".into(), json!(family != id));
        model
    }));
    for (name, variants) in groups(ids) {
        if variants.len() == 1 && variants[0].id == name {
            continue;
        }
        let mut family = info(name);
        let mut efforts: BTreeSet<_> = variants
            .iter()
            .map(|v| v.effort)
            .filter(|e| !e.is_empty())
            .collect();
        // `none` can select a non-thinking ID without a literal `-none`
        // suffix. Expose this valid SDK control alongside the named levels.
        let has_thinking = variants.iter().any(|v| v.thinking);
        if (has_thinking || !efforts.is_empty())
            && variants
                .iter()
                .any(|v| !v.thinking && (has_thinking || matches!(v.effort, "" | "none")))
        {
            efforts.insert("none");
        }
        family.capabilities.reasoning_effort.supported = [
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Max,
        ]
        .into_iter()
        .filter(|e| efforts.contains(e.to_string().as_str()))
        .collect();
        family.capabilities.reasoning_effort.default = None;
        family.capabilities.supports_reasoning = variants
            .iter()
            .any(|v| v.thinking || (!v.effort.is_empty() && v.effort != "none"));
        family.metadata.insert("cursor_family".into(), json!(true));
        family
            .metadata
            .insert("cursor_family_id".into(), json!(name));
        family
            .metadata
            .insert("cursor_explicit_variant".into(), json!(false));
        family.capabilities.supported_speeds = [GenerationSpeed::Standard, GenerationSpeed::Fast]
            .into_iter()
            .filter(|speed| {
                variants
                    .iter()
                    .any(|v| v.fast == (*speed == GenerationSpeed::Fast))
            })
            .collect();
        family.metadata.insert(
            "cursor_variants".into(),
            json!(variants.iter().map(|v| v.id).collect::<Vec<_>>()),
        );
        family.metadata.insert(
            "cursor_speeds".into(),
            json!(["standard", "fast"]
                .into_iter()
                .filter(|s| variants.iter().any(|v| v.fast == (*s == "fast")))
                .collect::<Vec<_>>()),
        );
        family.metadata.insert(
            "cursor_thinking".into(),
            json!(variants.iter().any(|v| v.thinking)),
        );
        catalog.insert(family);
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).into()).collect()
    }
    fn settings(value: Value) -> GenerationSettings {
        serde_json::from_value(value).unwrap()
    }
    fn reference_models() -> Vec<String> {
        ids(&[
            "claude-fable-5-1-medium",
            "claude-fable-5-1-thinking-medium",
            "claude-fable-5-1-thinking-high",
            "claude-fable-5-1-thinking-high-fast",
            "composer-2.5",
            "composer-2.5-fast",
            "cursor-grok-4.6-high",
            "cursor-grok-4.6-xhigh",
        ])
    }

    #[test]
    fn pr235_family_selection_and_suffix_precedence() {
        let models = reference_models();
        for (model, body, expected) in [
            (
                "claude-fable-5-1",
                json!({"reasoning_effort":"high"}),
                "claude-fable-5-1-thinking-high",
            ),
            (
                "claude-fable-5-1",
                json!({}),
                "claude-fable-5-1-thinking-medium",
            ),
            (
                "cursor-grok-4.6",
                json!({"reasoning_effort":"xhigh"}),
                "cursor-grok-4.6-xhigh",
            ),
            (
                "claude-fable-5-1",
                json!({"reasoning_effort":"high", "speed":"fast"}),
                "claude-fable-5-1-thinking-high-fast",
            ),
            (
                "claude-fable-5-1",
                json!({"reasoning_effort":"high", "openai_responses":{"service_tier":"priority"}}),
                "claude-fable-5-1-thinking-high-fast",
            ),
            (
                "claude-fable-5-1",
                json!({"reasoning_effort":"none"}),
                "claude-fable-5-1-medium",
            ),
            (
                "composer-2.5",
                json!({"reasoning_effort":"high"}),
                "composer-2.5",
            ),
            (
                "composer-2.5",
                json!({"reasoning_effort":"high", "speed":"fast"}),
                "composer-2.5-fast",
            ),
            (
                "claude-fable-5-1-thinking-high",
                json!({"reasoning_effort":"low", "speed":"fast"}),
                "claude-fable-5-1-thinking-high",
            ),
            (
                "claude-fable-5-1(high)",
                json!({"reasoning_effort":"medium"}),
                "claude-fable-5-1-thinking-high",
            ),
            (
                "claude-fable-5-1(high)",
                json!({"anthropic":{"thinking":{"type":"disabled"}}}),
                "claude-fable-5-1-thinking-high",
            ),
            (
                "claude-fable-5-1(none)",
                json!({"anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}}),
                "claude-fable-5-1-medium",
            ),
            (
                "claude-fable-5-1(16384)",
                json!({"anthropic":{"thinking":{"type":"disabled"}}}),
                "claude-fable-5-1-thinking-high",
            ),
            (
                "claude-fable-5-1(auto)",
                json!({"reasoning_effort":"high", "anthropic":{"thinking":{"type":"disabled"}}}),
                "claude-fable-5-1-thinking-medium",
            ),
            (
                "claude-fable-5-1-thinking-high(low)",
                json!({}),
                "claude-fable-5-1-thinking-high",
            ),
            ("composer-2.5(high)", json!({}), "composer-2.5"),
            ("unknown-model(high)", json!({}), "unknown-model(high)"),
            (
                "claude-fable-5-1(invalid)",
                json!({"reasoning_effort":"high"}),
                "claude-fable-5-1-thinking-high",
            ),
        ] {
            assert_eq!(
                resolve(model, &settings(body), &models).unwrap(),
                expected,
                "{model}"
            );
        }
    }

    #[test]
    fn thinking_budget_disabled_and_effort_only_families() {
        let models = ids(&["gpt-5.6-sol-high", "gpt-5.6-sol-medium", "gpt-5.6-sol-none"]);
        for (body, expected) in [
            (
                json!({"anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}}),
                "gpt-5.6-sol-high",
            ),
            (
                json!({"reasoning_effort":"medium", "anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}}),
                "gpt-5.6-sol-medium",
            ),
            (
                json!({"reasoning_effort":"high", "anthropic":{"thinking":{"type":"disabled"}}}),
                "gpt-5.6-sol-none",
            ),
        ] {
            assert_eq!(
                resolve("gpt-5.6-sol", &settings(body), &models).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn unavailable_options_and_conflicts_are_errors_without_fallback() {
        let models = reference_models();
        for body in [
            json!({"reasoning_effort":"low"}),
            json!({"reasoning_effort":"ultra"}),
            json!({"reasoning_effort":"medium", "speed":"fast"}),
            json!({"speed":"standard", "openai_responses":{"service_tier":"priority"}}),
            json!({"speed":"fast", "openai_responses":{"service_tier":"default"}}),
            json!({"openai_responses":{"service_tier":"flex"}}),
            json!({"reasoning_effort":"none", "anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}}),
        ] {
            assert!(
                resolve("claude-fable-5-1", &settings(body.clone()), &models).is_err(),
                "{body}"
            );
        }
        let settings = settings(json!({"reasoning_effort":"high", "speed":"fast"}));
        assert!(resolve("claude-fable-5-1", &settings, &models).is_ok());
        let other_account = ids(&["claude-fable-5-1-thinking-medium"]);
        assert!(resolve("claude-fable-5-1", &settings, &other_account).is_err());
    }

    #[test]
    fn extra_high_tie_breaker_is_independent_of_catalog_order() {
        for names in [
            ["gpt-5.5-xhigh", "gpt-5.5-extra-high"],
            ["gpt-5.5-extra-high", "gpt-5.5-xhigh"],
        ] {
            assert_eq!(
                resolve(
                    "gpt-5.5",
                    &settings(json!({"reasoning_effort":"xhigh"})),
                    &ids(&names)
                )
                .unwrap(),
                "gpt-5.5-extra-high"
            );
        }
    }

    #[test]
    fn catalog_preserves_ids_and_describes_only_advertised_family_options() {
        let ids = reference_models();
        let catalog = catalog(&ids, &ModelCapabilities::default());
        assert_eq!(catalog.models().len(), ids.len() + 2);
        for id in &ids {
            assert!(catalog.models().iter().any(|m| &m.model_id == id));
        }
        let family = catalog
            .models()
            .iter()
            .find(|m| m.model_id == "claude-fable-5-1")
            .unwrap();
        assert_eq!(
            family.capabilities.reasoning_effort.supported,
            [
                ReasoningEffort::None,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert_eq!(
            family.capabilities.supported_speeds,
            [GenerationSpeed::Standard, GenerationSpeed::Fast]
        );
        assert_eq!(family.metadata["cursor_family"], true);
        assert_eq!(family.metadata["cursor_explicit_variant"], false);
        let exact = catalog
            .models()
            .iter()
            .find(|m| m.model_id == "claude-fable-5-1-thinking-high-fast")
            .unwrap();
        assert_eq!(exact.capabilities.supported_speeds, [GenerationSpeed::Fast]);
        assert_eq!(exact.metadata["cursor_family_id"], "claude-fable-5-1");
        assert_eq!(exact.metadata["cursor_explicit_variant"], true);
        let composer = catalog
            .models()
            .iter()
            .find(|m| m.model_id == "composer-2.5")
            .unwrap();
        assert!(!composer.capabilities.supports_reasoning);
        assert!(composer.capabilities.reasoning_effort.supported.is_empty());
    }

    #[test]
    fn thinking_only_catalog_has_no_invented_effort_levels() {
        let ids = ids(&["claude-fable-5-1", "claude-fable-5-1-thinking"]);
        let catalog = catalog(&ids, &ModelCapabilities::default());
        let family = catalog
            .models()
            .iter()
            .find(|m| m.model_id == "claude-fable-5-1")
            .unwrap();
        assert!(family.capabilities.supports_reasoning);
        assert_eq!(
            family.capabilities.reasoning_effort.supported,
            [ReasoningEffort::None]
        );
        assert_eq!(
            resolve(
                "claude-fable-5-1",
                &settings(
                    json!({"anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}})
                ),
                &ids
            )
            .unwrap(),
            "claude-fable-5-1-thinking"
        );
        assert!(resolve("claude-fable-5-1", &settings(json!({"reasoning_effort":"high", "anthropic":{"thinking":{"type":"enabled", "budget_tokens":16384}}})), &ids).is_err());
        assert_eq!(
            resolve("claude-fable-5-1", &GenerationSettings::default(), &ids).unwrap(),
            "claude-fable-5-1"
        );
        assert_eq!(
            resolve(
                "claude-fable-5-1",
                &settings(json!({"anthropic":{"thinking":{"type":"disabled"}}})),
                &ids
            )
            .unwrap(),
            "claude-fable-5-1"
        );
    }
}
