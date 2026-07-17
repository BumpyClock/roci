//! OpenAI model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::{ModelCapabilities, ModelInputCapabilities, ReasoningEffortCapabilities};
use roci_core::types::ReasoningEffort;

/// OpenAI models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum OpenAiModel {
    #[strum(serialize = "gpt-4o")]
    Gpt4o,
    #[strum(serialize = "gpt-4o-mini")]
    Gpt4oMini,
    #[strum(serialize = "gpt-4-turbo")]
    Gpt4Turbo,
    #[strum(serialize = "gpt-4")]
    Gpt4,
    #[strum(serialize = "gpt-3.5-turbo")]
    Gpt35Turbo,
    #[strum(serialize = "o1")]
    O1,
    #[strum(serialize = "o1-mini")]
    O1Mini,
    #[strum(serialize = "o1-pro")]
    O1Pro,
    #[strum(serialize = "o3")]
    O3,
    #[strum(serialize = "o3-mini")]
    O3Mini,
    #[strum(serialize = "o4-mini")]
    O4Mini,
    #[strum(serialize = "gpt-4.1")]
    Gpt41,
    #[strum(serialize = "gpt-4.1-mini")]
    Gpt41Mini,
    #[strum(serialize = "gpt-4.1-nano")]
    Gpt41Nano,
    #[strum(serialize = "gpt-5")]
    Gpt5,
    #[strum(serialize = "gpt-5.1")]
    Gpt51,
    #[strum(serialize = "gpt-5.2")]
    Gpt52,
    #[strum(serialize = "gpt-5.4")]
    Gpt54,
    #[strum(serialize = "gpt-5.4-mini")]
    Gpt54Mini,
    #[strum(serialize = "gpt-5.5")]
    Gpt55,
    #[strum(serialize = "gpt-5.6-sol")]
    Gpt56Sol,
    #[strum(serialize = "gpt-5.6-terra")]
    Gpt56Terra,
    #[strum(serialize = "gpt-5.6-luna")]
    Gpt56Luna,
    #[strum(serialize = "gpt-5-pro")]
    Gpt5Pro,
    #[strum(serialize = "gpt-5-mini")]
    Gpt5Mini,
    #[strum(serialize = "gpt-5-nano")]
    Gpt5Nano,
    #[strum(serialize = "gpt-5-thinking")]
    Gpt5Thinking,
    #[strum(serialize = "gpt-5-thinking-mini")]
    Gpt5ThinkingMini,
    #[strum(serialize = "gpt-5-thinking-nano")]
    Gpt5ThinkingNano,
    #[strum(serialize = "gpt-5-chat-latest")]
    Gpt5ChatLatest,
    #[strum(serialize = "gpt-4o-realtime-preview")]
    Gpt4oRealtimePreview,
    /// Custom/unknown OpenAI model by ID.
    #[strum(default)]
    Custom(String),
}

impl OpenAiModel {
    /// Get the API model identifier.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gpt4o => "gpt-4o",
            Self::Gpt4oMini => "gpt-4o-mini",
            Self::Gpt4Turbo => "gpt-4-turbo",
            Self::Gpt4 => "gpt-4",
            Self::Gpt35Turbo => "gpt-3.5-turbo",
            Self::O1 => "o1",
            Self::O1Mini => "o1-mini",
            Self::O1Pro => "o1-pro",
            Self::O3 => "o3",
            Self::O3Mini => "o3-mini",
            Self::O4Mini => "o4-mini",
            Self::Gpt41 => "gpt-4.1",
            Self::Gpt41Mini => "gpt-4.1-mini",
            Self::Gpt41Nano => "gpt-4.1-nano",
            Self::Gpt5 => "gpt-5",
            Self::Gpt51 => "gpt-5.1",
            Self::Gpt52 => "gpt-5.2",
            Self::Gpt54 => "gpt-5.4",
            Self::Gpt54Mini => "gpt-5.4-mini",
            Self::Gpt55 => "gpt-5.5",
            Self::Gpt56Sol => "gpt-5.6-sol",
            Self::Gpt56Terra => "gpt-5.6-terra",
            Self::Gpt56Luna => "gpt-5.6-luna",
            Self::Gpt5Pro => "gpt-5-pro",
            Self::Gpt5Mini => "gpt-5-mini",
            Self::Gpt5Nano => "gpt-5-nano",
            Self::Gpt5Thinking => "gpt-5-thinking",
            Self::Gpt5ThinkingMini => "gpt-5-thinking-mini",
            Self::Gpt5ThinkingNano => "gpt-5-thinking-nano",
            Self::Gpt5ChatLatest => "gpt-5-chat-latest",
            Self::Gpt4oRealtimePreview => "gpt-4o-realtime-preview",
            Self::Custom(s) => s,
        }
    }

    /// Whether this model uses the Responses API (vs Chat Completions).
    pub fn uses_responses_api(&self) -> bool {
        match self {
            Self::O3
            | Self::O3Mini
            | Self::O4Mini
            | Self::Gpt5
            | Self::Gpt51
            | Self::Gpt52
            | Self::Gpt54
            | Self::Gpt54Mini
            | Self::Gpt55
            | Self::Gpt56Sol
            | Self::Gpt56Terra
            | Self::Gpt56Luna
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano
            | Self::Gpt5ChatLatest => true,
            Self::Custom(id) => {
                let lower = id.to_ascii_lowercase();
                lower.starts_with("gpt-5")
                    || lower.starts_with("o3")
                    || lower.starts_with("o4")
                    || lower.contains("codex")
            }
            _ => false,
        }
    }

    /// Whether this is a reasoning model.
    pub fn is_reasoning(&self) -> bool {
        matches!(
            self,
            Self::O1 | Self::O1Mini | Self::O1Pro | Self::O3 | Self::O3Mini | Self::O4Mini
        )
    }

    /// Whether the model accepts sampling parameters in the Responses API.
    pub fn supports_sampling_params(&self, reasoning_effort: Option<ReasoningEffort>) -> bool {
        match self {
            Self::Gpt51
            | Self::Gpt52
            | Self::Gpt54
            | Self::Gpt54Mini
            | Self::Gpt55
            | Self::Gpt56Sol
            | Self::Gpt56Terra
            | Self::Gpt56Luna => {
                matches!(reasoning_effort, Some(ReasoningEffort::None))
            }
            Self::Gpt5
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano
            | Self::Gpt5ChatLatest => false,
            _ => true,
        }
    }

    /// Whether the model supports GPT-5 text verbosity controls.
    pub fn supports_text_verbosity(&self) -> bool {
        matches!(
            self,
            Self::Gpt5
                | Self::Gpt51
                | Self::Gpt52
                | Self::Gpt54
                | Self::Gpt54Mini
                | Self::Gpt55
                | Self::Gpt56Sol
                | Self::Gpt56Terra
                | Self::Gpt56Luna
                | Self::Gpt5Pro
                | Self::Gpt5Mini
                | Self::Gpt5Nano
                | Self::Gpt5Thinking
                | Self::Gpt5ThinkingMini
                | Self::Gpt5ThinkingNano
        )
    }

    /// Whether this model is in the GPT-5 family.
    pub fn is_gpt5_family(&self) -> bool {
        matches!(
            self,
            Self::Gpt5
                | Self::Gpt51
                | Self::Gpt52
                | Self::Gpt54
                | Self::Gpt54Mini
                | Self::Gpt55
                | Self::Gpt56Sol
                | Self::Gpt56Terra
                | Self::Gpt56Luna
                | Self::Gpt5Pro
                | Self::Gpt5Mini
                | Self::Gpt5Nano
                | Self::Gpt5Thinking
                | Self::Gpt5ThinkingMini
                | Self::Gpt5ThinkingNano
        )
    }

    /// Whether this model should be treated as GPT-5 for request shaping.
    pub fn is_gpt5_family_id(&self) -> bool {
        match self {
            Self::Custom(id) => id.starts_with("gpt-5"),
            _ => self.is_gpt5_family(),
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        self.capabilities_for(false)
    }

    pub(crate) fn codex_capabilities(&self) -> ModelCapabilities {
        self.capabilities_for(true)
    }

    fn capabilities_for(&self, codex: bool) -> ModelCapabilities {
        let (ctx, vision, tools, reasoning, json_schema) = match self {
            Self::Gpt4o | Self::Gpt4oMini => (128_000, true, true, false, true),
            Self::Gpt4Turbo => (128_000, true, true, false, true),
            Self::Gpt4 => (8_192, false, true, false, false),
            Self::Gpt35Turbo => (16_385, false, true, false, false),
            Self::O1 | Self::O1Pro => (200_000, true, false, true, false),
            Self::O1Mini => (128_000, false, false, true, false),
            Self::O3 | Self::O3Mini | Self::O4Mini => (200_000, true, true, true, true),
            Self::Gpt41 | Self::Gpt41Mini | Self::Gpt41Nano => (1_000_000, true, true, false, true),
            Self::Gpt54 | Self::Gpt54Mini | Self::Gpt55 => (
                if codex {
                    272_000
                } else if matches!(self, Self::Gpt54 | Self::Gpt55) {
                    1_050_000
                } else {
                    400_000
                },
                true,
                true,
                true,
                true,
            ),
            Self::Gpt56Sol | Self::Gpt56Terra | Self::Gpt56Luna => (
                if codex { 372_000 } else { 1_050_000 },
                true,
                true,
                true,
                true,
            ),
            Self::Gpt5ChatLatest => (128_000, true, true, false, true),
            Self::Gpt5
            | Self::Gpt51
            | Self::Gpt52
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano => (400_000, true, true, true, true),
            Self::Gpt4oRealtimePreview => (128_000, true, true, false, true),
            Self::Custom(_) => (128_000, false, true, false, true),
        };
        ModelCapabilities {
            supports_vision: vision,
            supports_tools: tools,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: json_schema,
            supports_reasoning: reasoning,
            reasoning_effort: self.reasoning_effort_capabilities(codex),
            supports_system_messages: !self.is_reasoning()
                || matches!(self, Self::O3 | Self::O3Mini | Self::O4Mini),
            context_length: ctx,
            max_output_tokens: if codex
                && matches!(
                    self,
                    Self::Gpt54
                        | Self::Gpt54Mini
                        | Self::Gpt55
                        | Self::Gpt56Sol
                        | Self::Gpt56Terra
                        | Self::Gpt56Luna
                ) {
                None
            } else {
                Some(match self {
                    Self::O1
                    | Self::O1Mini
                    | Self::O1Pro
                    | Self::O3
                    | Self::O3Mini
                    | Self::O4Mini => 100_000,
                    Self::Gpt5
                    | Self::Gpt51
                    | Self::Gpt52
                    | Self::Gpt54
                    | Self::Gpt54Mini
                    | Self::Gpt55
                    | Self::Gpt56Sol
                    | Self::Gpt56Terra
                    | Self::Gpt56Luna
                    | Self::Gpt5Pro
                    | Self::Gpt5Mini
                    | Self::Gpt5Nano
                    | Self::Gpt5Thinking
                    | Self::Gpt5ThinkingMini
                    | Self::Gpt5ThinkingNano => 128_000,
                    Self::Gpt5ChatLatest => 16_384,
                    _ => 16_384,
                })
            },
            input: ModelInputCapabilities::from_vision_support(vision),
        }
    }

    fn reasoning_effort_capabilities(&self, codex: bool) -> ReasoningEffortCapabilities {
        match self {
            Self::Gpt5Pro => {
                reasoning_efforts(&[ReasoningEffort::High], Some(ReasoningEffort::High))
            }
            Self::Gpt51 => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                Some(ReasoningEffort::None),
            ),
            Self::Gpt52 => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ],
                Some(ReasoningEffort::None),
            ),
            Self::Gpt54 | Self::Gpt54Mini if codex => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ],
                Some(ReasoningEffort::Medium),
            ),
            Self::Gpt54 | Self::Gpt54Mini => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ],
                Some(ReasoningEffort::None),
            ),
            Self::Gpt55 if codex => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ],
                Some(ReasoningEffort::Medium),
            ),
            Self::Gpt55 => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                ],
                Some(ReasoningEffort::Medium),
            ),
            Self::Gpt56Sol | Self::Gpt56Terra | Self::Gpt56Luna if !codex => reasoning_efforts(
                &[
                    ReasoningEffort::None,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                    ReasoningEffort::Max,
                ],
                None,
            ),
            Self::Gpt56Sol | Self::Gpt56Terra => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                    ReasoningEffort::Max,
                    ReasoningEffort::Ultra,
                ],
                Some(if matches!(self, Self::Gpt56Sol) {
                    ReasoningEffort::Low
                } else {
                    ReasoningEffort::Medium
                }),
            ),
            Self::Gpt56Luna => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                    ReasoningEffort::XHigh,
                    ReasoningEffort::Max,
                ],
                Some(ReasoningEffort::Medium),
            ),
            Self::Gpt5
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano => reasoning_efforts(
                &[
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                Some(ReasoningEffort::Medium),
            ),
            Self::O1Pro => reasoning_efforts(&[ReasoningEffort::High], Some(ReasoningEffort::High)),
            Self::O1 | Self::O1Mini | Self::O3 | Self::O3Mini | Self::O4Mini => reasoning_efforts(
                &[
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                Some(ReasoningEffort::Medium),
            ),
            _ => ReasoningEffortCapabilities::default(),
        }
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
    fn gpt4o_has_vision_input_capabilities() {
        let caps = OpenAiModel::Gpt4o.capabilities();

        assert!(caps.supports_vision);
        assert!(caps.input.image.is_some());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn gpt4_text_model_has_no_image_input_capabilities() {
        let caps = OpenAiModel::Gpt4.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn custom_model_has_safe_text_only_input_capabilities() {
        let caps = OpenAiModel::Custom("unknown-compatible-model".to_string()).capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn reasoning_models_expose_model_specific_effort_options() {
        let gpt5 = OpenAiModel::Gpt5.capabilities();
        let gpt51 = OpenAiModel::Gpt51.capabilities();
        let gpt52 = OpenAiModel::Gpt52.capabilities();
        let gpt5_pro = OpenAiModel::Gpt5Pro.capabilities();
        let o1_pro = OpenAiModel::O1Pro.capabilities();
        let gpt56_sol = OpenAiModel::Gpt56Sol.capabilities();
        let codex_gpt56_sol = OpenAiModel::Gpt56Sol.codex_capabilities();

        assert!(gpt5.supports_reasoning_effort(ReasoningEffort::Minimal));
        assert_eq!(
            gpt5.default_reasoning_effort(),
            Some(ReasoningEffort::Medium)
        );
        assert!(gpt51.supports_reasoning_effort(ReasoningEffort::None));
        assert_eq!(
            gpt51.default_reasoning_effort(),
            Some(ReasoningEffort::None)
        );
        assert_eq!(
            gpt52.default_reasoning_effort(),
            Some(ReasoningEffort::None)
        );
        assert_eq!(
            gpt5_pro.reasoning_effort_options(),
            &[ReasoningEffort::High]
        );
        assert_eq!(o1_pro.reasoning_effort_options(), &[ReasoningEffort::High]);
        assert_eq!(
            o1_pro.default_reasoning_effort(),
            Some(ReasoningEffort::High)
        );
        assert_eq!(gpt56_sol.context_length, 1_050_000);
        assert_eq!(gpt56_sol.default_reasoning_effort(), None);
        assert!(gpt56_sol.supports_reasoning_effort(ReasoningEffort::None));
        assert!(!gpt56_sol.supports_reasoning_effort(ReasoningEffort::Ultra));
        assert_eq!(codex_gpt56_sol.context_length, 372_000);
        assert_eq!(
            codex_gpt56_sol.default_reasoning_effort(),
            Some(ReasoningEffort::Low)
        );
        assert!(codex_gpt56_sol.supports_reasoning_effort(ReasoningEffort::Ultra));
    }

    #[test]
    fn same_id_models_use_provider_specific_reasoning_defaults() {
        let public = OpenAiModel::Gpt54.capabilities();
        let codex = OpenAiModel::Gpt54.codex_capabilities();
        let codex_gpt55 = OpenAiModel::Gpt55.codex_capabilities();
        let gpt5_chat = OpenAiModel::Gpt5ChatLatest.capabilities();

        assert_eq!(public.context_length, 1_050_000);
        assert_eq!(
            public.default_reasoning_effort(),
            Some(ReasoningEffort::None)
        );
        assert!(public.supports_reasoning_effort(ReasoningEffort::None));
        assert_eq!(codex.context_length, 272_000);
        assert_eq!(
            codex.default_reasoning_effort(),
            Some(ReasoningEffort::Medium)
        );
        assert!(!codex.supports_reasoning_effort(ReasoningEffort::None));
        assert!(!codex_gpt55.supports_reasoning_effort(ReasoningEffort::None));
        assert!(!gpt5_chat.supports_reasoning);
        assert!(gpt5_chat.reasoning_effort_options().is_empty());
        assert_eq!(gpt5_chat.context_length, 128_000);
        assert_eq!(gpt5_chat.max_output_tokens, Some(16_384));
    }
}
