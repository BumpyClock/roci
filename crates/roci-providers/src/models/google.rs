//! Google Gemini model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::{ModelCapabilities, ModelInputCapabilities, ReasoningEffortCapabilities};
use roci_core::types::{GoogleThinkingConfig, GoogleThinkingLevel, ReasoningEffort};

/// Google Gemini models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum GoogleModel {
    #[strum(serialize = "gemini-2.5-pro")]
    Gemini25Pro,
    #[strum(serialize = "gemini-2.5-flash")]
    Gemini25Flash,
    #[strum(serialize = "gemini-2.5-flash-lite")]
    Gemini25FlashLite,
    #[strum(serialize = "gemini-2.0-flash")]
    Gemini20Flash,
    #[strum(serialize = "gemini-3-flash")]
    Gemini3Flash,
    #[strum(serialize = "gemini-3-flash-preview")]
    Gemini3FlashPreview,
    #[strum(serialize = "gemini-3-pro-preview")]
    Gemini3ProPreview,
    #[strum(serialize = "gemini-1.5-pro")]
    Gemini15Pro,
    #[strum(serialize = "gemini-1.5-flash")]
    Gemini15Flash,
    /// Custom/unknown Google model.
    #[strum(default)]
    Custom(String),
}

impl GoogleModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gemini25Pro => "gemini-2.5-pro",
            Self::Gemini25Flash => "gemini-2.5-flash",
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::Gemini20Flash => "gemini-2.0-flash",
            Self::Gemini3Flash => "gemini-3-flash",
            Self::Gemini3FlashPreview => "gemini-3-flash-preview",
            Self::Gemini3ProPreview => "gemini-3-pro-preview",
            Self::Gemini15Pro => "gemini-1.5-pro",
            Self::Gemini15Flash => "gemini-1.5-flash",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        let ctx = match self {
            Self::Gemini25Pro | Self::Gemini25Flash | Self::Gemini25FlashLite => 1_000_000,
            Self::Gemini20Flash
            | Self::Gemini3Flash
            | Self::Gemini3FlashPreview
            | Self::Gemini3ProPreview => 1_000_000,
            Self::Gemini15Pro | Self::Gemini15Flash => 2_000_000,
            Self::Custom(_) => 1_000_000,
        };
        let reasoning_effort = self.reasoning_effort_capabilities();
        ModelCapabilities {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: true,
            supports_reasoning: !reasoning_effort.supported.is_empty(),
            reasoning_effort,
            supports_system_messages: true,
            context_length: ctx,
            max_output_tokens: Some(8_192),
            input: ModelInputCapabilities::from_vision_support(true),
        }
    }

    fn reasoning_effort_capabilities(&self) -> ReasoningEffortCapabilities {
        match self {
            Self::Gemini25Pro => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                None,
            ),
            Self::Gemini25Flash => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                None,
            ),
            Self::Gemini3Flash | Self::Gemini3FlashPreview => reasoning_efforts(
                &[
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                Some(ReasoningEffort::High),
            ),
            Self::Gemini3ProPreview => reasoning_efforts(
                &[ReasoningEffort::Low, ReasoningEffort::High],
                Some(ReasoningEffort::High),
            ),
            _ => ReasoningEffortCapabilities::default(),
        }
    }

    pub(crate) fn thinking_config_for_effort(
        &self,
        effort: ReasoningEffort,
    ) -> Option<GoogleThinkingConfig> {
        let thinking_config = match self {
            Self::Gemini25Pro => {
                let budget_tokens = match effort {
                    ReasoningEffort::Low => 1_024,
                    ReasoningEffort::Medium => 8_192,
                    ReasoningEffort::High => 24_576,
                    _ => return None,
                };
                GoogleThinkingConfig {
                    budget_tokens: Some(budget_tokens),
                    include_thoughts: None,
                    thinking_level: None,
                }
            }
            Self::Gemini25Flash => {
                let budget_tokens = match effort {
                    ReasoningEffort::None => 0,
                    ReasoningEffort::Low => 1_024,
                    ReasoningEffort::Medium => 8_192,
                    ReasoningEffort::High => 24_576,
                    _ => return None,
                };
                GoogleThinkingConfig {
                    budget_tokens: Some(budget_tokens),
                    include_thoughts: None,
                    thinking_level: None,
                }
            }
            Self::Gemini3Flash | Self::Gemini3FlashPreview => {
                let thinking_level = match effort {
                    ReasoningEffort::Minimal => GoogleThinkingLevel::Minimal,
                    ReasoningEffort::Low => GoogleThinkingLevel::Low,
                    ReasoningEffort::Medium => GoogleThinkingLevel::Medium,
                    ReasoningEffort::High => GoogleThinkingLevel::High,
                    _ => return None,
                };
                GoogleThinkingConfig {
                    budget_tokens: None,
                    include_thoughts: None,
                    thinking_level: Some(thinking_level),
                }
            }
            Self::Gemini3ProPreview => {
                let thinking_level = match effort {
                    ReasoningEffort::Low => GoogleThinkingLevel::Low,
                    ReasoningEffort::High => GoogleThinkingLevel::High,
                    _ => return None,
                };
                GoogleThinkingConfig {
                    budget_tokens: None,
                    include_thoughts: None,
                    thinking_level: Some(thinking_level),
                }
            }
            _ => return None,
        };

        Some(thinking_config)
    }
}

fn reasoning_efforts(
    supported: &[ReasoningEffort],
    default: Option<ReasoningEffort>,
) -> ReasoningEffortCapabilities {
    ReasoningEffortCapabilities::new(supported.iter().copied(), default)
        .expect("static model reasoning effort metadata is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::types::ReasoningEffort;

    #[test]
    fn gemini_3_models_expose_thinking_levels() {
        let caps = GoogleModel::Gemini3Flash.capabilities();

        assert!(caps.supports_reasoning);
        assert!(caps.supports_reasoning_effort(ReasoningEffort::Minimal));
        assert_eq!(caps.default_reasoning_effort(), Some(ReasoningEffort::High));
    }

    #[test]
    fn gemini_2_5_leaves_dynamic_thinking_default_unset() {
        let pro = GoogleModel::Gemini25Pro.capabilities();
        let flash = GoogleModel::Gemini25Flash.capabilities();

        assert!(pro.supports_reasoning_effort(ReasoningEffort::Medium));
        assert!(!pro.supports_reasoning_effort(ReasoningEffort::None));
        assert!(flash.supports_reasoning_effort(ReasoningEffort::None));
        assert_eq!(pro.default_reasoning_effort(), None);
        assert_eq!(flash.default_reasoning_effort(), None);
    }
}
