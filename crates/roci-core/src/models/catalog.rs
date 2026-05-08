//! Provider-neutral model catalog types.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use super::ModelCapabilities;

/// Provider-neutral metadata for one model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub provider_key: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub capabilities: ModelCapabilities,
    pub policy: ModelPolicy,
    pub source: ModelCatalogSource,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// Availability and app-facing policy for a catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPolicy {
    pub requires_credentials: bool,
    pub local: bool,
    pub deprecated: bool,
    pub default_for_provider: bool,
}

/// Where catalog metadata came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelCatalogSource {
    Static,
    Dynamic { endpoint: String },
}

/// Filters controlling provider model listing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelListOptions {
    pub provider_key: Option<String>,
    pub include_dynamic: bool,
    pub include_static: bool,
    pub include_unavailable: bool,
}

impl Default for ModelListOptions {
    fn default() -> Self {
        Self {
            provider_key: None,
            include_dynamic: true,
            include_static: true,
            include_unavailable: false,
        }
    }
}

/// Deterministically ordered, deduplicated model catalog.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ModelCatalog {
    models: Vec<ModelInfo>,
}

impl ModelCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_models(models: impl IntoIterator<Item = ModelInfo>) -> Self {
        let mut catalog = Self::new();
        for model in models {
            catalog.insert(model);
        }
        catalog
    }

    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }

    pub fn update_models(&mut self, mut update: impl FnMut(&mut ModelInfo)) {
        for model in &mut self.models {
            update(model);
        }
        self.normalize();
    }

    pub fn into_models(self) -> Vec<ModelInfo> {
        self.models
    }

    pub fn insert(&mut self, model: ModelInfo) {
        if let Some(existing) = self.models.iter_mut().find(|existing| {
            existing.provider_key == model.provider_key && existing.model_id == model.model_id
        }) {
            if source_rank(&model.source) >= source_rank(&existing.source) {
                *existing = model;
            }
        } else {
            self.models.push(model);
        }

        self.models.sort_by(|a, b| {
            a.provider_key
                .cmp(&b.provider_key)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });
    }

    pub fn extend(&mut self, other: ModelCatalog) {
        for model in other.into_models() {
            self.insert(model);
        }
    }

    fn normalize(&mut self) {
        let models = std::mem::take(&mut self.models);
        for model in models {
            self.insert(model);
        }
    }
}

impl<'de> Deserialize<'de> for ModelCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawCatalog {
            #[serde(default)]
            models: Vec<ModelInfo>,
        }

        let raw = RawCatalog::deserialize(deserializer)?;
        Ok(Self::from_models(raw.models))
    }
}

fn source_rank(source: &ModelCatalogSource) -> u8 {
    match source {
        ModelCatalogSource::Static => 0,
        ModelCatalogSource::Dynamic { .. } => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ModelInputCapabilities;

    fn model(provider: &str, id: &str, source: ModelCatalogSource) -> ModelInfo {
        ModelInfo {
            provider_key: provider.to_string(),
            model_id: id.to_string(),
            display_name: Some(id.to_string()),
            capabilities: ModelCapabilities {
                supports_streaming: true,
                supports_system_messages: true,
                context_length: 128_000,
                input: ModelInputCapabilities::default(),
                ..ModelCapabilities::default()
            },
            policy: ModelPolicy {
                requires_credentials: true,
                local: false,
                deprecated: false,
                default_for_provider: false,
            },
            source,
            metadata: Default::default(),
        }
    }

    #[test]
    fn model_list_options_default_hides_unavailable_and_includes_sources() {
        let options = ModelListOptions::default();

        assert!(options.provider_key.is_none());
        assert!(options.include_dynamic);
        assert!(options.include_static);
        assert!(!options.include_unavailable);
    }

    #[test]
    fn model_catalog_dedupes_dynamic_over_static() {
        let mut catalog = ModelCatalog::default();
        catalog.insert(model("openai", "gpt-4o", ModelCatalogSource::Static));
        catalog.insert(model(
            "openai",
            "gpt-4o",
            ModelCatalogSource::Dynamic {
                endpoint: "/models".to_string(),
            },
        ));

        let models = catalog.into_models();

        assert_eq!(models.len(), 1);
        assert!(matches!(
            models[0].source,
            ModelCatalogSource::Dynamic { .. }
        ));
    }

    #[test]
    fn model_catalog_keeps_deterministic_provider_model_order() {
        let mut catalog = ModelCatalog::default();
        catalog.insert(model("zeta", "b", ModelCatalogSource::Static));
        catalog.insert(model("alpha", "b", ModelCatalogSource::Static));
        catalog.insert(model("alpha", "a", ModelCatalogSource::Static));

        let keys = catalog
            .models()
            .iter()
            .map(|model| (model.provider_key.as_str(), model.model_id.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(keys, vec![("alpha", "a"), ("alpha", "b"), ("zeta", "b")]);
    }

    #[test]
    fn model_catalog_round_trips_json() {
        let mut catalog = ModelCatalog::default();
        catalog.insert(model("openai", "gpt-4o", ModelCatalogSource::Static));

        let json = serde_json::to_string(&catalog).unwrap();
        let decoded: ModelCatalog = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded.models().len(), 1);
        assert_eq!(decoded.models()[0].provider_key, "openai");
    }

    #[test]
    fn model_catalog_deserialize_normalizes_duplicates_and_order() {
        let json = serde_json::json!({
            "models": [
                model("zeta", "b", ModelCatalogSource::Static),
                model("openai", "gpt-4o", ModelCatalogSource::Static),
                model("openai", "gpt-4o", ModelCatalogSource::Dynamic {
                    endpoint: "/models".to_string(),
                })
            ]
        });

        let decoded: ModelCatalog = serde_json::from_value(json).unwrap();
        let keys = decoded
            .models()
            .iter()
            .map(|model| (model.provider_key.as_str(), model.model_id.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(keys, vec![("openai", "gpt-4o"), ("zeta", "b")]);
        assert!(matches!(
            decoded.models()[0].source,
            ModelCatalogSource::Dynamic { .. }
        ));
    }

    #[test]
    fn model_catalog_update_models_renormalizes() {
        let mut catalog = ModelCatalog::from_models([
            model("zeta", "b", ModelCatalogSource::Static),
            model("openai", "a", ModelCatalogSource::Static),
        ]);

        catalog.update_models(|model| {
            if model.provider_key == "zeta" {
                model.provider_key = "openai".to_string();
                model.model_id = "a".to_string();
                model.source = ModelCatalogSource::Dynamic {
                    endpoint: "/models".to_string(),
                };
            }
        });

        assert_eq!(catalog.models().len(), 1);
        assert!(matches!(
            catalog.models()[0].source,
            ModelCatalogSource::Dynamic { .. }
        ));
    }
}
