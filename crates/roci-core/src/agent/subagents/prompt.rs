//! Sub-agent prompt policy and system prompt composition.

use super::types::{SubagentOverrides, SubagentProfile};

/// Controls how system prompts are composed for child sub-agents.
#[derive(Debug, Clone)]
pub struct SubagentPromptPolicy {
    /// Preamble prepended to every child system prompt.
    pub preamble: String,
}

impl Default for SubagentPromptPolicy {
    fn default() -> Self {
        Self {
            preamble: Self::default_child_preamble().to_string(),
        }
    }
}

impl SubagentPromptPolicy {
    /// Default preamble for child sub-agents.
    pub fn default_child_preamble() -> &'static str {
        "You are a sub-agent. Do not address the user directly. \
         Use `ask_user` when user input is required. \
         Return concise progress and results to the parent."
    }

    /// Build the final system prompt for a child sub-agent.
    ///
    /// Precedence: override > profile > preamble-only.
    pub fn build_system_prompt(
        &self,
        profile: &SubagentProfile,
        overrides: &SubagentOverrides,
    ) -> String {
        let base = overrides
            .system_prompt
            .as_deref()
            .or(profile.system_prompt.as_deref())
            .unwrap_or("");

        if base.is_empty() {
            self.preamble.clone()
        } else {
            format!("{}\n\n{}", self.preamble, base)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preamble_is_non_empty() {
        let policy = SubagentPromptPolicy::default();
        assert!(!policy.preamble.is_empty());
        assert!(policy.preamble.contains("sub-agent"));
    }

    #[test]
    fn build_prompt_uses_profile_when_no_override() {
        let policy = SubagentPromptPolicy::default();
        let profile = SubagentProfile {
            system_prompt: Some("Write tests first.".into()),
            ..Default::default()
        };
        let overrides = SubagentOverrides::default();
        let prompt = policy.build_system_prompt(&profile, &overrides);
        assert!(prompt.contains("sub-agent"));
        assert!(prompt.contains("Write tests first."));
    }

    #[test]
    fn build_prompt_override_takes_precedence() {
        let policy = SubagentPromptPolicy::default();
        let profile = SubagentProfile {
            system_prompt: Some("Profile prompt.".into()),
            ..Default::default()
        };
        let overrides = SubagentOverrides {
            system_prompt: Some("Override prompt.".into()),
            ..Default::default()
        };
        let prompt = policy.build_system_prompt(&profile, &overrides);
        assert!(prompt.contains("Override prompt."));
        assert!(!prompt.contains("Profile prompt."));
    }

    #[test]
    fn build_prompt_preamble_only_when_no_profile_or_override() {
        let policy = SubagentPromptPolicy::default();
        let profile = SubagentProfile::default();
        let overrides = SubagentOverrides::default();
        let prompt = policy.build_system_prompt(&profile, &overrides);
        assert_eq!(prompt, policy.preamble);
    }
}
