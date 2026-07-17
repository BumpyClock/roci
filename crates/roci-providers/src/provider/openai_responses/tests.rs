use super::*;

fn settings() -> GenerationSettings {
    GenerationSettings {
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: None,
        presence_penalty: None,
        frequency_penalty: None,
        seed: None,
        reasoning_effort: None,
        text_verbosity: None,
        response_format: None,
        openai_responses: None,
        user: None,
        anthropic: None,
        google: None,
        tool_choice: None,
        stream_idle_timeout_ms: None,
    }
}

#[test]
fn gpt5_rejects_sampling_settings() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        temperature: Some(0.7),
        ..Default::default()
    };
    let err = provider.validate_settings(&settings).unwrap_err();
    assert!(matches!(err, RociError::InvalidArgument(_)));
}

#[test]
fn gpt52_allows_sampling_with_default_reasoning_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        temperature: Some(0.7),
        ..Default::default()
    };
    assert!(provider.validate_settings(&settings).is_ok());
}

#[test]
fn gpt51_allows_sampling_with_default_or_explicit_reasoning_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt51, "test-key".to_string(), None, None);
    let default_settings = GenerationSettings {
        temperature: Some(0.7),
        ..Default::default()
    };
    let explicit_settings = GenerationSettings {
        reasoning_effort: Some(ReasoningEffort::None),
        ..default_settings.clone()
    };

    assert!(provider.validate_settings(&default_settings).is_ok());
    assert!(provider.validate_settings(&explicit_settings).is_ok());
}

#[test]
fn gpt52_allows_sampling_with_reasoning_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        temperature: Some(0.7),
        reasoning_effort: Some(ReasoningEffort::None),
        ..Default::default()
    };
    assert!(provider.validate_settings(&settings).is_ok());
}

#[test]
fn public_gpt54_allows_sampling_with_default_reasoning_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt54, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        temperature: Some(0.7),
        ..Default::default()
    };

    assert!(provider.validate_settings(&settings).is_ok());
}

#[test]
fn public_gpt55_allows_sampling_only_with_explicit_reasoning_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt55, "test-key".to_string(), None, None);
    let default_settings = GenerationSettings {
        temperature: Some(0.7),
        ..Default::default()
    };
    let non_reasoning_settings = GenerationSettings {
        reasoning_effort: Some(ReasoningEffort::None),
        ..default_settings.clone()
    };

    assert!(provider.validate_settings(&default_settings).is_err());
    assert!(provider.validate_settings(&non_reasoning_settings).is_ok());
}

#[test]
fn public_gpt56_models_allow_sampling_with_explicit_reasoning_none() {
    for model in [
        OpenAiModel::Gpt56Sol,
        OpenAiModel::Gpt56Terra,
        OpenAiModel::Gpt56Luna,
    ] {
        let provider = OpenAiResponsesProvider::new(model, "test-key".to_string(), None, None);
        let settings = GenerationSettings {
            temperature: Some(0.7),
            reasoning_effort: Some(ReasoningEffort::None),
            ..Default::default()
        };

        assert!(provider.validate_settings(&settings).is_ok());
    }
}

#[test]
fn gpt52_request_defaults_reasoning_to_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt52, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["reasoning"]["effort"], "none");
}

#[test]
fn public_gpt54_defaults_reasoning_to_none() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt54, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["reasoning"]["effort"], "none");
}

#[test]
fn codex_gpt54_rejects_public_only_reasoning_none() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt54,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        None,
    );
    let settings = GenerationSettings {
        reasoning_effort: Some(ReasoningEffort::None),
        ..Default::default()
    };

    let err = provider.validate_settings(&settings).unwrap_err();

    assert!(matches!(err, RociError::InvalidArgument(_)));
}

#[test]
fn codex_gpt54_defaults_reasoning_to_medium() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt54,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        None,
    );
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["reasoning"]["effort"], "medium");
}

#[test]
fn gpt41_allows_sampling_settings() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        temperature: Some(0.7),
        top_p: Some(0.9),
        ..Default::default()
    };
    assert!(provider.validate_settings(&settings).is_ok());
}

#[test]
fn gpt41_rejects_text_verbosity_setting() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt41Nano, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        text_verbosity: Some(TextVerbosity::Low),
        ..Default::default()
    };
    let err = provider.validate_settings(&settings).unwrap_err();
    assert!(matches!(err, RociError::InvalidArgument(_)));
}

#[test]
fn gpt5_allows_text_verbosity_setting() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let settings = GenerationSettings {
        text_verbosity: Some(TextVerbosity::High),
        ..Default::default()
    };
    assert!(provider.validate_settings(&settings).is_ok());
}

#[test]
fn request_body_includes_text_verbosity_and_format() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: GenerationSettings {
            text_verbosity: Some(TextVerbosity::Low),
            ..Default::default()
        },
        tools: None,
        response_format: Some(ResponseFormat::JsonObject),
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    assert_eq!(body["text"]["verbosity"], "low");
    assert_eq!(body["text"]["format"]["type"], "json_object");
}

#[test]
fn request_body_maps_system_to_developer_for_reasoning_models() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![
            ModelMessage::system("Use this system message"),
            ModelMessage::user("hello"),
        ],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    assert_eq!(body["input"][0]["role"], "developer");
    assert_eq!(body["input"][0]["content"], "Use this system message");
}

#[test]
fn provider_attachment_payload_openai_responses_maps_text_and_image_parts() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage {
            role: Role::User,
            content: vec![
                ContentPart::Text {
                    text: "Inspect attachment".to_string(),
                },
                ContentPart::Image(ImageContent {
                    data: "aW1hZ2U=".to_string(),
                    mime_type: "image/png".to_string(),
                }),
            ],
            name: None,
            timestamp: None,
            metadata: None,
        }],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    let content = &body["input"][0]["content"];

    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "Inspect attachment");
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "data:image/png;base64,aW1hZ2U=");
    assert!(content[1].get("file").is_none());
    assert!(content[1].get("file_id").is_none());
    assert!(content[1].get("input_file").is_none());
    assert!(content[1].get("document").is_none());
}

#[test]
fn provider_attachment_payload_openai_responses_preserves_unsupported_media_marker_text() {
    let marker =
        "User attached unsupported media: doc.pdf (application/pdf, 7 bytes). Content omitted.";
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user(marker)],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    assert_eq!(body["input"][0]["content"].as_str(), Some(marker));
    assert!(!body["input"][0]["content"]
        .as_str()
        .unwrap()
        .contains("/tmp/"));
}

#[test]
fn request_body_defaults_reasoning_and_text_for_gpt5() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["text"]["verbosity"], "high");
    assert!(body.get("truncation").is_none());
}

#[test]
fn gpt5_chat_request_omits_reasoning_without_explicit_effort() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt5ChatLatest,
        "test-key".to_string(),
        None,
        None,
    );
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert!(body.get("reasoning").is_none());
}

#[test]
fn codex_request_uses_the_selected_models_default_reasoning_effort() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt56Sol,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        None,
    );
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);

    assert_eq!(body["model"], "gpt-5.6-sol");
    assert_eq!(body["reasoning"]["effort"], "low");
}

#[test]
fn request_body_defaults_truncation_for_reasoning_models() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::O3, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["truncation"], "auto");
    assert!(body.get("text").is_none());
}

#[test]
fn request_body_includes_openai_responses_options() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("tag".to_string(), "value".to_string());
    let settings = GenerationSettings {
        user: Some("user-1".to_string()),
        openai_responses: Some(OpenAiResponsesOptions {
            parallel_tool_calls: Some(false),
            previous_response_id: Some("resp_1".to_string()),
            instructions: Some("Be brief".to_string()),
            metadata: Some(metadata),
            service_tier: Some(OpenAiServiceTier::Flex),
            truncation: Some(OpenAiTruncation::Auto),
            store: Some(true),
        }),
        ..Default::default()
    };
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings,
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };
    let body = provider.build_request_body(&request, false);
    assert_eq!(body["user"], "user-1");
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["previous_response_id"], "resp_1");
    assert_eq!(body["instructions"], "Be brief");
    assert_eq!(body["metadata"]["tag"], "value");
    assert_eq!(body["service_tier"], "flex");
    assert_eq!(body["truncation"], "auto");
    assert_eq!(body["store"], true);
}

#[test]
fn request_body_merges_request_metadata_and_includes_prompt_cache_key() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);

    let mut options_metadata = std::collections::HashMap::new();
    options_metadata.insert("tag".to_string(), "settings".to_string());
    let mut request_metadata = std::collections::HashMap::new();
    request_metadata.insert("tag".to_string(), "request".to_string());
    request_metadata.insert("trace_id".to_string(), "trace-1".to_string());
    let session_id = "session-abc";
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: GenerationSettings {
            openai_responses: Some(OpenAiResponsesOptions {
                metadata: Some(options_metadata),
                ..Default::default()
            }),
            ..Default::default()
        },
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: request_metadata,
        payload_callback: None,
        session_id: Some(session_id.to_string()),
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    assert_eq!(body["prompt_cache_key"], session_id);
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["metadata"]["tag"], "request");
    assert_eq!(body["metadata"]["trace_id"], "trace-1");
    assert!(body["metadata"].get("roci_session_id").is_none());
}

#[test]
fn request_body_omits_prompt_cache_key_when_no_session_id() {
    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), None, None);
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: None,
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    assert!(body.get("prompt_cache_key").is_none());
    assert!(body.get("previous_response_id").is_none());
}

#[test]
fn codex_request_body_includes_prompt_cache_key() {
    let provider = OpenAiResponsesProvider::new(
        OpenAiModel::Gpt5Nano,
        "test-key".to_string(),
        Some("https://chatgpt.com/backend-api/codex".to_string()),
        Some("acct-123".to_string()),
    );
    let session_id = "codex-session-1";
    let request = ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: settings(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: reqwest::header::HeaderMap::new(),
        metadata: std::collections::HashMap::new(),
        payload_callback: None,
        session_id: Some(session_id.to_string()),
        transport: None,
    };

    let body = provider.build_request_body(&request, false);
    assert_eq!(body["prompt_cache_key"], session_id);
}
