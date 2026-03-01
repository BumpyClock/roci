use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::{
    AgentRuntime, SessionBeforeCompactHook, SessionBeforeCompactPayload, SessionBeforeTreePayload,
    SessionCompactionOverride, SessionSummaryHookOutcome, SummaryPreparationData,
};
use crate::agent::message::{AgentMessage, AgentMessageExt};
use crate::agent_loop::compaction::{
    estimate_message_tokens, extract_cumulative_file_operations, extract_file_operations,
    prepare_compaction, select_messages_with_token_budget_newest_first,
    serialize_messages_for_summary, serialize_pi_mono_summary, PiMonoSummary, PreparedCompaction,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::{ProviderRegistry, ProviderRequest};
use crate::resource::{BranchSummarySettings, CompactionSettings};
use crate::types::{GenerationSettings, ModelMessage, Role};

impl AgentRuntime {
    /// Compact the current conversation history in place using the configured
    /// compaction policy and summary model.
    ///
    /// # Errors
    ///
    /// Returns [`RociError::InvalidState`] if the runtime is not idle.
    pub async fn compact(&self) -> Result<(), RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let model = self
            .model
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?
            .clone();
        let messages = self
            .messages
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (messages lock contended)".into()))?
            .clone();
        drop(state_guard);

        let compacted = Self::compact_messages_with_model(
            messages,
            &model,
            &self.config.compaction,
            self.config.session_before_compact.as_ref(),
            &self.registry,
            &self.roci_config,
        )
        .await?;

        if let Some(compacted_messages) = compacted {
            *self.messages.lock().await = compacted_messages;
            self.broadcast_snapshot().await;
        }

        Ok(())
    }

    /// Generate a branch summary message for explicitly selected branch entries
    ///
    /// This method is intentionally explicit and does not auto-trigger from
    /// runtime execution paths
    pub async fn summarize_branch_entries(
        &self,
        entries_between_branches: Vec<ModelMessage>,
        settings: &BranchSummarySettings,
    ) -> Result<AgentMessage, RociError> {
        let state_guard = self.lock_state_for_idle_mutation()?;
        let model = self
            .model
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (model lock contended)".into()))?
            .clone();
        let existing_messages = self
            .messages
            .try_lock()
            .map_err(|_| RociError::InvalidState("Agent is busy (messages lock contended)".into()))?
            .clone();
        drop(state_guard);

        let selected_entries = select_messages_with_token_budget_newest_first(
            &entries_between_branches,
            settings.reserve_tokens,
        );
        if selected_entries.is_empty() {
            return Err(RociError::InvalidState(
                "branch summary requires at least one entry within token budget".to_string(),
            ));
        }
        let tree_payload = SessionBeforeTreePayload {
            to_summarize: SummaryPreparationData::from_messages(selected_entries.clone()),
            settings: settings.clone(),
        };
        if let Some(hook) = self.config.session_before_tree.as_ref() {
            match hook(tree_payload).await? {
                SessionSummaryHookOutcome::Continue => {}
                SessionSummaryHookOutcome::Cancel => {
                    return Err(RociError::InvalidState(
                        "branch summary canceled by session_before_tree hook".to_string(),
                    ));
                }
                SessionSummaryHookOutcome::OverrideSummary(summary) => {
                    let summary = summary.trim().to_string();
                    if summary.is_empty() {
                        return Err(RociError::InvalidState(
                            "branch summary text must not be empty".to_string(),
                        ));
                    }
                    return Ok(AgentMessage::branch_summary(summary));
                }
                SessionSummaryHookOutcome::OverrideCompaction(_) => {
                    return Err(RociError::InvalidState(
                        "branch summary hook does not accept compaction override object"
                            .to_string(),
                    ));
                }
            }
        }

        let summary_model = match settings.model.as_deref() {
            Some(model) => LanguageModel::from_str(model)?,
            None => model,
        };
        let provider = self.registry.create_provider(
            summary_model.provider_name(),
            summary_model.model_id(),
            &self.roci_config,
        )?;

        let transcript = serialize_messages_for_summary(&selected_entries);
        let summary_prompt = format!(
            "Summarize the branch transition transcript into concise bullets focused on user goals, constraints, progress, decisions, next steps, and critical context.\n\nTranscript:\n{transcript}"
        );
        let summary_response = provider
            .generate_text(&ProviderRequest {
                messages: vec![
                    ModelMessage::system("You create precise branch transition summaries"),
                    ModelMessage::user(summary_prompt),
                ],
                settings: GenerationSettings::default(),
                tools: None,
                response_format: None,
                api_key_override: None,
                headers: reqwest::header::HeaderMap::new(),
                metadata: HashMap::new(),
                payload_callback: None,
                session_id: None,
                transport: None,
            })
            .await?;

        let summary_text = summary_response.text.trim().to_string();
        if summary_text.is_empty() {
            return Err(RociError::InvalidState(
                "branch summary model returned empty output".to_string(),
            ));
        }

        let cumulative_file_ops =
            extract_cumulative_file_operations(&existing_messages, &selected_entries);
        let summary = PiMonoSummary {
            progress: vec![summary_text],
            read_files: cumulative_file_ops.read_files,
            modified_files: cumulative_file_ops.modified_files,
            ..PiMonoSummary::default()
        };
        Ok(AgentMessage::branch_summary(serialize_pi_mono_summary(
            &summary,
        )))
    }

    fn split_messages_for_compaction(
        conversation_messages: &[ModelMessage],
        first_kept_entry_id: usize,
    ) -> (
        Vec<ModelMessage>,
        Vec<ModelMessage>,
        Vec<ModelMessage>,
        bool,
    ) {
        if first_kept_entry_id >= conversation_messages.len() {
            return (
                conversation_messages[..first_kept_entry_id].to_vec(),
                Vec::new(),
                conversation_messages[first_kept_entry_id..].to_vec(),
                false,
            );
        }

        let turn_start = conversation_messages[..first_kept_entry_id]
            .iter()
            .rposition(|message| message.role == Role::User)
            .unwrap_or(first_kept_entry_id);
        let split_turn = turn_start < first_kept_entry_id;

        if split_turn {
            (
                conversation_messages[..turn_start].to_vec(),
                conversation_messages[turn_start..first_kept_entry_id].to_vec(),
                conversation_messages[first_kept_entry_id..].to_vec(),
                true,
            )
        } else {
            (
                conversation_messages[..first_kept_entry_id].to_vec(),
                Vec::new(),
                conversation_messages[first_kept_entry_id..].to_vec(),
                false,
            )
        }
    }

    fn count_tokens_before_entry(
        conversation_messages: &[ModelMessage],
        first_kept_entry_id: usize,
    ) -> usize {
        conversation_messages[..first_kept_entry_id]
            .iter()
            .map(estimate_message_tokens)
            .sum::<usize>()
    }

    fn legacy_summary_override(
        summary: String,
        prepared: &PreparedCompaction,
        conversation_messages: &[ModelMessage],
    ) -> SessionCompactionOverride {
        let first_kept_entry_id = prepared.cut_index.min(conversation_messages.len());
        SessionCompactionOverride {
            summary,
            first_kept_entry_id,
            tokens_before: Self::count_tokens_before_entry(
                conversation_messages,
                first_kept_entry_id,
            ),
            details: None,
        }
    }

    fn validate_compaction_override(
        override_data: SessionCompactionOverride,
        conversation_messages: &[ModelMessage],
    ) -> Result<SessionCompactionOverride, RociError> {
        let summary = override_data.summary.trim().to_string();
        if summary.is_empty() {
            return Err(RociError::InvalidState(
                "compaction override summary must not be empty".to_string(),
            ));
        }

        let first_kept_entry_id = override_data.first_kept_entry_id;
        if first_kept_entry_id == 0 || first_kept_entry_id > conversation_messages.len() {
            return Err(RociError::InvalidState(format!(
                "compaction override first_kept_entry_id must be within 1..={} (got {})",
                conversation_messages.len(),
                first_kept_entry_id
            )));
        }
        if first_kept_entry_id < conversation_messages.len()
            && conversation_messages[first_kept_entry_id].role == Role::Tool
        {
            return Err(RociError::InvalidState(format!(
                "compaction override first_kept_entry_id={} cannot point to a tool result entry",
                first_kept_entry_id
            )));
        }

        let expected_tokens_before =
            Self::count_tokens_before_entry(conversation_messages, first_kept_entry_id);
        if override_data.tokens_before != expected_tokens_before {
            return Err(RociError::InvalidState(format!(
                "compaction override tokens_before={} does not match expected {} for first_kept_entry_id={}",
                override_data.tokens_before, expected_tokens_before, first_kept_entry_id
            )));
        }

        Ok(SessionCompactionOverride {
            summary,
            first_kept_entry_id,
            tokens_before: override_data.tokens_before,
            details: override_data.details.and_then(|value| {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }),
        })
    }

    pub(super) async fn compact_messages_with_model(
        messages: Vec<ModelMessage>,
        run_model: &LanguageModel,
        compaction: &CompactionSettings,
        session_before_compact: Option<&SessionBeforeCompactHook>,
        registry: &Arc<ProviderRegistry>,
        roci_config: &RociConfig,
    ) -> Result<Option<Vec<ModelMessage>>, RociError> {
        let system_prefix_len = messages
            .iter()
            .take_while(|message| message.role == Role::System)
            .count();
        let system_prefix = messages[..system_prefix_len].to_vec();
        let conversation_messages = messages[system_prefix_len..].to_vec();

        if conversation_messages.len() < 2 {
            return Ok(None);
        }

        let prepared = prepare_compaction(&conversation_messages, compaction.keep_recent_tokens);
        if prepared.messages_to_summarize.is_empty() {
            return Ok(None);
        }

        let cancellation_token = CancellationToken::new();
        let compaction_payload = SessionBeforeCompactPayload::from_prepared(
            &prepared,
            compaction.clone(),
            cancellation_token.child_token(),
        );
        let compaction_override = match session_before_compact {
            Some(hook) => match hook(compaction_payload).await? {
                SessionSummaryHookOutcome::Continue => None,
                SessionSummaryHookOutcome::Cancel => {
                    cancellation_token.cancel();
                    return Err(RociError::InvalidState(
                        "compaction canceled by session_before_compact hook".to_string(),
                    ));
                }
                SessionSummaryHookOutcome::OverrideSummary(summary) => Some(
                    Self::legacy_summary_override(summary, &prepared, &conversation_messages),
                ),
                SessionSummaryHookOutcome::OverrideCompaction(override_data) => Some(override_data),
            },
            None => None,
        };

        let mut messages_to_summarize = prepared.messages_to_summarize.clone();
        let mut turn_prefix_messages = prepared.turn_prefix_messages.clone();
        let mut kept_messages = prepared.kept_messages.clone();
        let mut split_turn = prepared.split_turn;
        let mut summary_override = None;
        let mut override_details = None;

        if let Some(override_data) = compaction_override {
            let override_data =
                Self::validate_compaction_override(override_data, &conversation_messages)?;
            let (
                override_messages_to_summarize,
                override_turn_prefix,
                override_kept,
                override_split_turn,
            ) = Self::split_messages_for_compaction(
                &conversation_messages,
                override_data.first_kept_entry_id,
            );
            if override_messages_to_summarize.is_empty() {
                return Err(RociError::InvalidState(format!(
                    "compaction override first_kept_entry_id={} leaves no entries to summarize",
                    override_data.first_kept_entry_id
                )));
            }
            messages_to_summarize = override_messages_to_summarize;
            turn_prefix_messages = override_turn_prefix;
            kept_messages = override_kept;
            split_turn = override_split_turn;
            summary_override = Some(override_data.summary);
            override_details = override_data.details;
        }

        let file_ops = extract_file_operations(&messages_to_summarize);
        let summary_model = match compaction.model.as_deref() {
            Some(model) => LanguageModel::from_str(model)?,
            None => run_model.clone(),
        };
        let summary_text = match summary_override {
            Some(summary) => summary,
            None => {
                let provider = registry.create_provider(
                    summary_model.provider_name(),
                    summary_model.model_id(),
                    roci_config,
                )?;
                let transcript = serialize_messages_for_summary(&messages_to_summarize);
                let summary_prompt = format!(
                    "Summarize the conversation transcript into concise bullets focused on user goals, constraints, progress, decisions, next steps, and critical context.\n\nTranscript:\n{transcript}"
                );
                let summary_response = provider
                    .generate_text(&ProviderRequest {
                        messages: vec![
                            ModelMessage::system(
                                "You create precise conversation compaction summaries",
                            ),
                            ModelMessage::user(summary_prompt),
                        ],
                        settings: GenerationSettings::default(),
                        tools: None,
                        response_format: None,
                        api_key_override: None,
                        headers: reqwest::header::HeaderMap::new(),
                        metadata: HashMap::new(),
                        payload_callback: None,
                        session_id: None,
                        transport: None,
                    })
                    .await?;
                summary_response.text.trim().to_string()
            }
        };
        if summary_text.is_empty() {
            return Err(RociError::InvalidState(
                "compaction summary model returned empty output".to_string(),
            ));
        }

        let mut critical_context = if split_turn {
            vec!["A turn split was preserved to avoid cutting a user/tool exchange".to_string()]
        } else {
            Vec::new()
        };
        if let Some(details) = override_details {
            critical_context.push(format!("Compaction override details: {details}"));
        }
        let summary = PiMonoSummary {
            progress: vec![summary_text],
            critical_context,
            read_files: file_ops.read_files,
            modified_files: file_ops.modified_files,
            ..PiMonoSummary::default()
        };
        let summary_message = AgentMessage::compaction_summary(serialize_pi_mono_summary(&summary))
            .to_llm_message()
            .ok_or_else(|| {
                RociError::InvalidState("compaction summary message failed to convert".to_string())
            })?;

        let mut compacted = Vec::with_capacity(
            system_prefix.len() + 1 + turn_prefix_messages.len() + kept_messages.len(),
        );
        compacted.extend(system_prefix);
        compacted.push(summary_message);
        compacted.extend(turn_prefix_messages);
        compacted.extend(kept_messages);
        Ok(Some(compacted))
    }
}
