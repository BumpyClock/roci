//! Ordered model candidate sets.

use std::collections::HashSet;

use crate::error::RociError;

use super::LanguageModel;

/// Ordered, deduplicated language-model candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCandidates {
    candidates: Vec<LanguageModel>,
}

impl ModelCandidates {
    /// Build candidates, preserving first occurrence for each `(provider, model_id)`.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::Configuration`] when no candidates are provided.
    pub fn new(candidates: Vec<LanguageModel>) -> Result<Self, RociError> {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let key = (
                candidate.provider_name().to_string(),
                candidate.model_id().to_string(),
            );
            if seen.insert(key) {
                deduped.push(candidate);
            }
        }
        if deduped.is_empty() {
            return Err(RociError::Configuration(
                "model candidates cannot be empty".to_string(),
            ));
        }
        Ok(Self {
            candidates: deduped,
        })
    }

    /// Build candidates from one model.
    pub fn from_model(model: LanguageModel) -> Self {
        Self {
            candidates: vec![model],
        }
    }

    /// First candidate tried for a run.
    pub fn primary(&self) -> &LanguageModel {
        &self.candidates[0]
    }

    /// Borrow ordered candidates.
    pub fn as_slice(&self) -> &[LanguageModel] {
        &self.candidates
    }

    /// Consume into ordered candidates.
    pub fn into_vec(self) -> Vec<LanguageModel> {
        self.candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str, model_id: &str) -> LanguageModel {
        LanguageModel::Known {
            provider_key: provider.to_string(),
            model_id: model_id.to_string(),
        }
    }

    #[test]
    fn new_rejects_empty_candidates() {
        let err = ModelCandidates::new(Vec::new()).unwrap_err();

        assert!(matches!(err, RociError::Configuration(_)));
    }

    #[test]
    fn new_dedupes_by_provider_and_model_id_first_wins() {
        let candidates = ModelCandidates::new(vec![
            model("openai", "gpt-4o"),
            model("anthropic", "claude"),
            LanguageModel::Custom {
                provider: "openai".to_string(),
                model_id: "gpt-4o".to_string(),
            },
            model("openai", "gpt-4o-mini"),
        ])
        .unwrap();

        assert_eq!(
            candidates.as_slice(),
            &[
                model("openai", "gpt-4o"),
                model("anthropic", "claude"),
                model("openai", "gpt-4o-mini"),
            ]
        );
    }

    #[test]
    fn from_model_is_single_model_migration_constructor() {
        let primary = model("openai", "gpt-4o");
        let candidates = ModelCandidates::from_model(primary.clone());

        assert_eq!(candidates.primary(), &primary);
        assert_eq!(candidates.as_slice(), &[primary]);
    }
}
