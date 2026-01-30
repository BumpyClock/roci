//! Core Agent struct with execute/stream capabilities.

use futures::stream::BoxStream;

use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider;
use crate::tools::tool::Tool;
use crate::types::*;

use super::conversation::Conversation;

/// An AI agent that maintains conversation state and can use tools.
pub struct Agent {
    model: LanguageModel,
    config: RociConfig,
    system_prompt: Option<String>,
    tools: Vec<Box<dyn Tool>>,
    settings: GenerationSettings,
    conversation: Conversation,
}

impl Agent {
    /// Create a new agent.
    pub fn new(model: LanguageModel) -> Self {
        Self {
            model,
            config: RociConfig::from_env(),
            system_prompt: None,
            tools: Vec::new(),
            settings: GenerationSettings::default(),
            conversation: Conversation::new(),
        }
    }

    /// Set system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set config.
    pub fn with_config(mut self, config: RociConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a tool.
    pub fn with_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Set generation settings.
    pub fn with_settings(mut self, settings: GenerationSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Execute a user message and get a response (with tool loop).
    pub async fn execute(&mut self, message: impl Into<String>) -> Result<String, RociError> {
        let provider = provider::create_provider(&self.model, &self.config)?;

        self.conversation.add_user_message(message);

        let mut messages = Vec::new();
        if let Some(ref sys) = self.system_prompt {
            messages.push(ModelMessage::system(sys.clone()));
        }
        messages.extend(self.conversation.messages().iter().cloned());

        let result = crate::generation::text::generate_text(
            provider.as_ref(),
            messages,
            self.settings.clone(),
            &self.tools,
        )
        .await?;

        self.conversation.add_assistant_message(&result.text);

        Ok(result.text)
    }

    /// Stream a response to a user message.
    pub async fn stream(
        &mut self,
        message: impl Into<String>,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        let provider = provider::create_provider(&self.model, &self.config)?;

        self.conversation.add_user_message(message);

        let mut messages = Vec::new();
        if let Some(ref sys) = self.system_prompt {
            messages.push(ModelMessage::system(sys.clone()));
        }
        messages.extend(self.conversation.messages().iter().cloned());

        crate::generation::stream::stream_text(
            provider.as_ref(),
            messages,
            self.settings.clone(),
            &[],
        )
        .await
    }

    /// Get the conversation history.
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// Clear conversation history.
    pub fn clear_history(&mut self) {
        self.conversation.clear();
    }
}
