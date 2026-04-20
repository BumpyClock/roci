//! Micro-compaction: provider-agnostic, deterministic, no-LLM content reduction.
//!
//! Performs fine-grained, within-message compaction of high-cost payloads:
//! - **Images**: elided and replaced with a text marker.
//! - **Thinking / redacted-thinking blocks**: elided with a visible marker.
//! - **Oversized tool results**: truncated while preserving tool identity and
//!   error semantics.
//!
//! All passes are idempotent — re-running on already-compacted output produces
//! identical output.

use crate::context::tokens::estimate_message_tokens;
use crate::types::{
    AgentToolResult, ContentPart, ImageContent, ModelMessage, RedactedThinkingContent,
    ThinkingContent,
};

use super::types::{MicroCompactionRequest, MicroCompactionResult};

// ---------------------------------------------------------------------------
// Marker constants
// ---------------------------------------------------------------------------

/// Marker prefix for elided images.
const IMAGE_ELIDED_PREFIX: &str = "[image elided: ";
/// Marker suffix for elided images.
const IMAGE_ELIDED_SUFFIX: &str = "]";

/// Marker for elided thinking blocks.
const THINKING_ELIDED: &str = "[thinking elided]";

/// Marker for elided redacted-thinking blocks.
const REDACTED_THINKING_ELIDED: &str = "[redacted thinking elided]";

/// Marker inserted when a tool result body is truncated.
const TOOL_RESULT_TRUNCATED_MARKER: &str = "\n...[truncated]";

/// Default character threshold above which a tool result payload is truncated.
const DEFAULT_TOOL_RESULT_CHAR_LIMIT: usize = 2_000;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tuning knobs for micro compaction passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroCompactionConfig {
    /// Character limit for tool result payloads. Results exceeding this are
    /// truncated with a visible marker.
    pub tool_result_char_limit: usize,
}

impl Default for MicroCompactionConfig {
    fn default() -> Self {
        Self {
            tool_result_char_limit: DEFAULT_TOOL_RESULT_CHAR_LIMIT,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run micro compaction on a request using the default configuration.
pub fn compact_micro(request: &MicroCompactionRequest) -> MicroCompactionResult {
    compact_micro_with_config(request, &MicroCompactionConfig::default())
}

/// Run micro compaction with explicit configuration.
pub fn compact_micro_with_config(
    request: &MicroCompactionRequest,
    config: &MicroCompactionConfig,
) -> MicroCompactionResult {
    let all_messages: Vec<&ModelMessage> = request
        .prepared
        .messages_to_summarize
        .iter()
        .chain(request.prepared.turn_prefix_messages.iter())
        .chain(request.prepared.kept_messages.iter())
        .collect();

    let tokens_before: usize = all_messages
        .iter()
        .copied()
        .map(estimate_message_tokens)
        .sum();

    let compacted: Vec<ModelMessage> = all_messages
        .into_iter()
        .map(|m| compact_message(m, config))
        .collect();

    let tokens_after: usize = compacted.iter().map(estimate_message_tokens).sum();

    let mut suffix = request.suffix.clone();
    suffix.compaction_count += 1;

    MicroCompactionResult {
        messages: compacted,
        tokens_before,
        tokens_after,
        entries_removed: 0, // micro compaction never removes whole messages
        suffix,
    }
}

// ---------------------------------------------------------------------------
// Per-message compaction
// ---------------------------------------------------------------------------

/// Compact a single message by transforming its content parts.
fn compact_message(message: &ModelMessage, config: &MicroCompactionConfig) -> ModelMessage {
    let compacted_content: Vec<ContentPart> = message
        .content
        .iter()
        .map(|part| compact_part(part, config))
        .collect();

    ModelMessage {
        role: message.role,
        content: compacted_content,
        name: message.name.clone(),
        timestamp: message.timestamp,
    }
}

// ---------------------------------------------------------------------------
// Per-part compaction
// ---------------------------------------------------------------------------

/// Compact a single content part.
fn compact_part(part: &ContentPart, config: &MicroCompactionConfig) -> ContentPart {
    match part {
        ContentPart::Image(img) => compact_image(img),
        ContentPart::Thinking(t) => compact_thinking(t),
        ContentPart::RedactedThinking(rt) => compact_redacted_thinking(rt),
        ContentPart::ToolResult(tr) => compact_tool_result(tr, config),
        // Text and ToolCall pass through unchanged.
        other => other.clone(),
    }
}

/// Replace an image with a text marker. Idempotent: if the image data is
/// already the marker text, return it unchanged.
fn compact_image(img: &ImageContent) -> ContentPart {
    let marker = format!(
        "{IMAGE_ELIDED_PREFIX}{}{IMAGE_ELIDED_SUFFIX}",
        img.mime_type
    );
    // Idempotence: if data is already the marker, pass through.
    if img.data == marker {
        return ContentPart::Image(img.clone());
    }
    ContentPart::Text { text: marker }
}

/// Replace a thinking block with a text marker.
fn compact_thinking(t: &ThinkingContent) -> ContentPart {
    // Idempotence: if the thinking text is already the marker, pass through.
    if t.thinking == THINKING_ELIDED {
        return ContentPart::Thinking(t.clone());
    }
    ContentPart::Text {
        text: THINKING_ELIDED.to_string(),
    }
}

/// Replace a redacted-thinking block with a text marker.
fn compact_redacted_thinking(rt: &RedactedThinkingContent) -> ContentPart {
    // Idempotence: if the data is already the marker, pass through.
    if rt.data == REDACTED_THINKING_ELIDED {
        return ContentPart::RedactedThinking(rt.clone());
    }
    ContentPart::Text {
        text: REDACTED_THINKING_ELIDED.to_string(),
    }
}

/// Truncate an oversized string tool result payload while preserving identity
/// and error semantics.
///
/// Only `Value::String` payloads are truncated. Non-string structured results
/// (objects, arrays, numbers, bools, null) pass through unchanged to avoid
/// breaking provider and sanitizer semantics that depend on the JSON shape.
fn compact_tool_result(tr: &AgentToolResult, config: &MicroCompactionConfig) -> ContentPart {
    // Only truncate string payloads. Structured results pass through as-is.
    let payload = match &tr.result {
        serde_json::Value::String(s) => s,
        _ => return ContentPart::ToolResult(tr.clone()),
    };

    let char_count = payload.chars().count();

    // Already within budget — pass through.
    if char_count <= config.tool_result_char_limit {
        return ContentPart::ToolResult(tr.clone());
    }

    // Find the byte offset of the char-limit boundary so we never slice
    // mid-codepoint.
    let byte_limit = payload
        .char_indices()
        .nth(config.tool_result_char_limit)
        .map_or(payload.len(), |(idx, _)| idx);

    // Truncate: keep the first `limit` characters and append the marker.
    let truncated = format!("{}{}", &payload[..byte_limit], TOOL_RESULT_TRUNCATED_MARKER);

    ContentPart::ToolResult(AgentToolResult {
        tool_call_id: tr.tool_call_id.clone(),
        result: serde_json::Value::String(truncated),
        is_error: tr.is_error,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::compaction::types::{
        CompactionSuffix, MicroCompactionRequest, PreparedCompaction,
    };
    use crate::types::{
        AgentToolCall, AgentToolResult, ContentPart, ImageContent, ModelMessage,
        RedactedThinkingContent, Role, ThinkingContent,
    };

    // -- helpers -----------------------------------------------------------

    /// Build a minimal `MicroCompactionRequest` from a flat message list.
    fn request_from_messages(messages: Vec<ModelMessage>) -> MicroCompactionRequest {
        MicroCompactionRequest {
            prepared: PreparedCompaction {
                messages_to_summarize: Vec::new(),
                turn_prefix_messages: Vec::new(),
                kept_messages: messages,
                split_turn: false,
                cut_index: 0,
            },
            suffix: CompactionSuffix::default(),
        }
    }

    fn make_image_part(mime: &str) -> ContentPart {
        ContentPart::Image(ImageContent {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAUA".repeat(100),
            mime_type: mime.to_string(),
        })
    }

    fn make_thinking_part(text: &str) -> ContentPart {
        ContentPart::Thinking(ThinkingContent {
            thinking: text.to_string(),
            signature: "sig123".to_string(),
        })
    }

    fn make_redacted_thinking_part(data: &str) -> ContentPart {
        ContentPart::RedactedThinking(RedactedThinkingContent {
            data: data.to_string(),
            signature: "sig456".to_string(),
        })
    }

    fn make_tool_result(call_id: &str, payload: &str, is_error: bool) -> ContentPart {
        ContentPart::ToolResult(AgentToolResult {
            tool_call_id: call_id.to_string(),
            result: serde_json::Value::String(payload.to_string()),
            is_error,
        })
    }

    // -- image elision -----------------------------------------------------

    #[test]
    fn image_is_replaced_with_text_marker() {
        let msg = ModelMessage {
            role: Role::User,
            content: vec![make_image_part("image/png")],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].content.len(), 1);
        match &result.messages[0].content[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "[image elided: image/png]");
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn image_elision_is_idempotent() {
        let msg = ModelMessage {
            role: Role::User,
            content: vec![make_image_part("image/jpeg")],
            name: None,
            timestamp: None,
        };
        let first = compact_micro(&request_from_messages(vec![msg]));
        let second = compact_micro(&request_from_messages(first.messages.clone()));
        assert_eq!(first.messages, second.messages);
    }

    // -- thinking elision --------------------------------------------------

    #[test]
    fn thinking_block_is_replaced_with_marker() {
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![
                make_thinking_part("let me reason about this carefully..."),
                ContentPart::Text {
                    text: "The answer is 42.".to_string(),
                },
            ],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        assert_eq!(result.messages[0].content.len(), 2);
        match &result.messages[0].content[0] {
            ContentPart::Text { text } => assert_eq!(text, THINKING_ELIDED),
            other => panic!("expected Text marker, got {other:?}"),
        }
        // The text part is preserved.
        match &result.messages[0].content[1] {
            ContentPart::Text { text } => assert_eq!(text, "The answer is 42."),
            other => panic!("expected preserved Text, got {other:?}"),
        }
    }

    #[test]
    fn thinking_elision_is_idempotent() {
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![make_thinking_part("deep thought")],
            name: None,
            timestamp: None,
        };
        let first = compact_micro(&request_from_messages(vec![msg]));
        let second = compact_micro(&request_from_messages(first.messages.clone()));
        assert_eq!(first.messages, second.messages);
    }

    // -- redacted thinking elision -----------------------------------------

    #[test]
    fn redacted_thinking_is_replaced_with_marker() {
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![make_redacted_thinking_part("encrypted-blob-data")],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        match &result.messages[0].content[0] {
            ContentPart::Text { text } => assert_eq!(text, REDACTED_THINKING_ELIDED),
            other => panic!("expected Text marker, got {other:?}"),
        }
    }

    #[test]
    fn redacted_thinking_elision_is_idempotent() {
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![make_redacted_thinking_part("encrypted-blob-data")],
            name: None,
            timestamp: None,
        };
        let first = compact_micro(&request_from_messages(vec![msg]));
        let second = compact_micro(&request_from_messages(first.messages.clone()));
        assert_eq!(first.messages, second.messages);
    }

    // -- tool result truncation --------------------------------------------

    #[test]
    fn small_tool_result_passes_through_unchanged() {
        let payload = "ok";
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_1", payload, false)],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg.clone()]));

        assert_eq!(result.messages[0].content, msg.content);
    }

    #[test]
    fn oversized_tool_result_is_truncated_with_marker() {
        let big_payload = "x".repeat(3_000);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_2", &big_payload, false)],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_2", "tool_call_id preserved");
                assert!(!tr.is_error, "is_error preserved");
                let text = tr.result.as_str().expect("result is a string");
                assert!(
                    text.ends_with(TOOL_RESULT_TRUNCATED_MARKER),
                    "truncation marker present"
                );
                assert!(
                    text.len() < big_payload.len() + 100,
                    "payload significantly reduced"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_error_flag_preserved_after_truncation() {
        let big_payload = "e".repeat(3_000);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_err", &big_payload, true)],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                assert!(tr.is_error, "error flag must be preserved");
                assert_eq!(tr.tool_call_id, "call_err");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_truncation_is_idempotent() {
        let big_payload = "y".repeat(5_000);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_3", &big_payload, false)],
            name: None,
            timestamp: None,
        };
        let first = compact_micro(&request_from_messages(vec![msg]));
        let second = compact_micro(&request_from_messages(first.messages.clone()));
        assert_eq!(first.messages, second.messages);
    }

    #[test]
    fn tool_result_truncation_handles_multibyte_utf8() {
        // Each char is 4 bytes (U+1F600 GRINNING FACE). A byte-index slice
        // at the char limit would land mid-codepoint and panic.
        let emoji_payload = "😀".repeat(3_000);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_utf8", &emoji_payload, false)],
            name: None,
            timestamp: None,
        };
        let config = MicroCompactionConfig {
            tool_result_char_limit: 500,
        };
        let result = compact_micro_with_config(&request_from_messages(vec![msg]), &config);

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_utf8", "tool_call_id preserved");
                assert!(!tr.is_error, "is_error preserved");
                let text = tr.result.as_str().expect("result is a string");
                assert!(
                    text.ends_with(TOOL_RESULT_TRUNCATED_MARKER),
                    "truncation marker present"
                );
                // Kept portion should be exactly 500 emoji chars (2000 bytes).
                let kept = text.trim_end_matches(TOOL_RESULT_TRUNCATED_MARKER);
                assert_eq!(kept.chars().count(), 500, "char limit respected");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }

        // Idempotence still holds with multibyte content.
        let second =
            compact_micro_with_config(&request_from_messages(result.messages.clone()), &config);
        assert_eq!(result.messages, second.messages);
    }

    #[test]
    fn tool_result_with_natural_truncated_suffix_still_compacts() {
        let payload = format!("{}{}", "n".repeat(500), TOOL_RESULT_TRUNCATED_MARKER);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("call_suffix", &payload, false)],
            name: None,
            timestamp: None,
        };
        let config = MicroCompactionConfig {
            tool_result_char_limit: 100,
        };

        let result = compact_micro_with_config(&request_from_messages(vec![msg]), &config);

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                let text = tr.result.as_str().expect("result is a string");
                assert_eq!(tr.tool_call_id, "call_suffix");
                assert!(!tr.is_error);
                assert_ne!(text, payload, "oversized payload must be reduced");
                assert!(
                    text.ends_with(TOOL_RESULT_TRUNCATED_MARKER),
                    "truncation marker present"
                );
                let kept = text.trim_end_matches(TOOL_RESULT_TRUNCATED_MARKER);
                assert_eq!(kept.chars().count(), 100, "char limit respected");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // -- text and tool-call pass-through -----------------------------------

    #[test]
    fn plain_text_messages_pass_through_unchanged() {
        let msg = ModelMessage::user("Hello, world!");
        let result = compact_micro(&request_from_messages(vec![msg.clone()]));
        assert_eq!(result.messages[0].text(), msg.text());
    }

    #[test]
    fn structured_object_tool_result_passes_through_unchanged() {
        let structured = serde_json::json!({
            "status": "ok",
            "files": ["a.rs", "b.rs"],
            "metadata": { "count": 42, "big_blob": "x".repeat(5_000) }
        });
        let tr = AgentToolResult {
            tool_call_id: "call_obj".to_string(),
            result: structured.clone(),
            is_error: false,
        };
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult(tr)],
            name: None,
            timestamp: None,
        };
        let config = MicroCompactionConfig {
            tool_result_char_limit: 10, // aggressively low — must still not touch it
        };
        let result = compact_micro_with_config(&request_from_messages(vec![msg]), &config);

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_obj");
                assert_eq!(tr.result, structured, "structured result must be unchanged");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn structured_array_tool_result_passes_through_unchanged() {
        let array = serde_json::json!(["alpha", "beta", "gamma".repeat(1_000)]);
        let tr = AgentToolResult {
            tool_call_id: "call_arr".to_string(),
            result: array.clone(),
            is_error: true,
        };
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![ContentPart::ToolResult(tr)],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                assert_eq!(tr.tool_call_id, "call_arr");
                assert!(tr.is_error, "error flag preserved on structured result");
                assert_eq!(tr.result, array, "array result must be unchanged");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn numeric_and_bool_tool_results_pass_through_unchanged() {
        for (label, value) in [
            ("number", serde_json::json!(999_999)),
            ("bool", serde_json::json!(true)),
            ("null", serde_json::Value::Null),
        ] {
            let tr = AgentToolResult {
                tool_call_id: format!("call_{label}"),
                result: value.clone(),
                is_error: false,
            };
            let msg = ModelMessage {
                role: Role::Tool,
                content: vec![ContentPart::ToolResult(tr)],
                name: None,
                timestamp: None,
            };
            let out = compact_micro(&request_from_messages(vec![msg]));
            match &out.messages[0].content[0] {
                ContentPart::ToolResult(tr) => {
                    assert_eq!(tr.result, value, "{label} result must be unchanged");
                }
                other => panic!("{label}: expected ToolResult, got {other:?}"),
            }
        }
    }

    #[test]
    fn tool_call_parts_pass_through_unchanged() {
        let tc = ContentPart::ToolCall(AgentToolCall {
            id: "call_tc".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({"path": "/tmp/a.txt"}),
            recipient: None,
        });
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![tc.clone()],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));
        assert_eq!(result.messages[0].content[0], tc);
    }

    // -- token accounting --------------------------------------------------

    #[test]
    fn tokens_after_less_than_or_equal_to_tokens_before_when_compacted() {
        let messages = vec![
            ModelMessage {
                role: Role::User,
                content: vec![make_image_part("image/png")],
                name: None,
                timestamp: None,
            },
            ModelMessage {
                role: Role::Assistant,
                content: vec![
                    make_thinking_part(&"long reasoning chain ".repeat(50)),
                    ContentPart::Text {
                        text: "answer".to_string(),
                    },
                ],
                name: None,
                timestamp: None,
            },
            ModelMessage {
                role: Role::Tool,
                content: vec![make_tool_result("call_big", &"z".repeat(5_000), false)],
                name: None,
                timestamp: None,
            },
        ];
        let result = compact_micro(&request_from_messages(messages));
        assert!(
            result.tokens_after <= result.tokens_before,
            "tokens_after ({}) should be <= tokens_before ({})",
            result.tokens_after,
            result.tokens_before,
        );
    }

    #[test]
    fn token_accounting_is_zero_for_empty_input() {
        let result = compact_micro(&request_from_messages(vec![]));
        assert_eq!(result.tokens_before, 0);
        assert_eq!(result.tokens_after, 0);
        assert_eq!(result.messages.len(), 0);
    }

    // -- suffix metadata ---------------------------------------------------

    #[test]
    fn suffix_compaction_count_incremented() {
        let result = compact_micro(&request_from_messages(vec![ModelMessage::user("hi")]));
        assert_eq!(result.suffix.compaction_count, 1);
    }

    #[test]
    fn suffix_preserves_prior_count() {
        let request = MicroCompactionRequest {
            prepared: PreparedCompaction {
                messages_to_summarize: Vec::new(),
                turn_prefix_messages: Vec::new(),
                kept_messages: vec![ModelMessage::user("hi")],
                split_turn: false,
                cut_index: 0,
            },
            suffix: CompactionSuffix {
                compaction_count: 5,
                ..Default::default()
            },
        };
        let result = compact_micro(&request);
        assert_eq!(result.suffix.compaction_count, 6);
    }

    // -- entries_removed always zero for micro compaction -------------------

    #[test]
    fn entries_removed_is_always_zero() {
        let messages = vec![
            ModelMessage {
                role: Role::User,
                content: vec![make_image_part("image/png")],
                name: None,
                timestamp: None,
            },
            ModelMessage {
                role: Role::Tool,
                content: vec![make_tool_result("c", &"a".repeat(5_000), false)],
                name: None,
                timestamp: None,
            },
        ];
        let result = compact_micro(&request_from_messages(messages));
        assert_eq!(result.entries_removed, 0);
    }

    // -- mixed content in a single message ---------------------------------

    #[test]
    fn mixed_content_message_compacts_selectively() {
        let msg = ModelMessage {
            role: Role::Assistant,
            content: vec![
                make_thinking_part("reasoning"),
                ContentPart::Text {
                    text: "visible answer".to_string(),
                },
                make_image_part("image/webp"),
            ],
            name: None,
            timestamp: None,
        };
        let result = compact_micro(&request_from_messages(vec![msg]));
        let parts = &result.messages[0].content;

        assert_eq!(parts.len(), 3);
        // Thinking → marker
        assert!(matches!(&parts[0], ContentPart::Text { text } if text == THINKING_ELIDED));
        // Text → unchanged
        assert!(matches!(&parts[1], ContentPart::Text { text } if text == "visible answer"));
        // Image → marker
        assert!(
            matches!(&parts[2], ContentPart::Text { text } if text == "[image elided: image/webp]")
        );
    }

    // -- custom config -----------------------------------------------------

    #[test]
    fn custom_char_limit_is_respected() {
        let payload = "a".repeat(500);
        let msg = ModelMessage {
            role: Role::Tool,
            content: vec![make_tool_result("c1", &payload, false)],
            name: None,
            timestamp: None,
        };
        let config = MicroCompactionConfig {
            tool_result_char_limit: 100,
        };
        let result = compact_micro_with_config(&request_from_messages(vec![msg]), &config);

        match &result.messages[0].content[0] {
            ContentPart::ToolResult(tr) => {
                let text = tr.result.as_str().unwrap();
                assert!(text.ends_with(TOOL_RESULT_TRUNCATED_MARKER));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // -- messages across all prepared segments ------------------------------

    #[test]
    fn compaction_covers_all_prepared_segments() {
        let request = MicroCompactionRequest {
            prepared: PreparedCompaction {
                messages_to_summarize: vec![ModelMessage {
                    role: Role::User,
                    content: vec![make_image_part("image/png")],
                    name: None,
                    timestamp: None,
                }],
                turn_prefix_messages: vec![ModelMessage {
                    role: Role::Assistant,
                    content: vec![make_thinking_part("thought")],
                    name: None,
                    timestamp: None,
                }],
                kept_messages: vec![ModelMessage {
                    role: Role::Tool,
                    content: vec![make_tool_result("c", &"b".repeat(5_000), false)],
                    name: None,
                    timestamp: None,
                }],
                split_turn: true,
                cut_index: 1,
            },
            suffix: CompactionSuffix::default(),
        };
        let result = compact_micro(&request);

        assert_eq!(result.messages.len(), 3);
        // All three segments were compacted.
        assert!(matches!(
            &result.messages[0].content[0],
            ContentPart::Text { text } if text.starts_with("[image elided")
        ));
        assert!(matches!(
            &result.messages[1].content[0],
            ContentPart::Text { text } if text == THINKING_ELIDED
        ));
        match &result.messages[2].content[0] {
            ContentPart::ToolResult(tr) => {
                assert!(tr
                    .result
                    .as_str()
                    .unwrap()
                    .ends_with(TOOL_RESULT_TRUNCATED_MARKER));
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // -- full round-trip idempotence across mixed messages ------------------

    #[test]
    fn full_round_trip_idempotence() {
        let messages = vec![
            ModelMessage {
                role: Role::User,
                content: vec![
                    ContentPart::Text {
                        text: "please analyze".to_string(),
                    },
                    make_image_part("image/png"),
                ],
                name: None,
                timestamp: None,
            },
            ModelMessage {
                role: Role::Assistant,
                content: vec![
                    make_thinking_part(&"deep thought ".repeat(200)),
                    make_redacted_thinking_part("encrypted-data"),
                    ContentPart::Text {
                        text: "here is my analysis".to_string(),
                    },
                ],
                name: None,
                timestamp: None,
            },
            ModelMessage {
                role: Role::Tool,
                content: vec![make_tool_result("c1", &"data".repeat(2_000), false)],
                name: None,
                timestamp: None,
            },
            ModelMessage::user("thanks"),
        ];

        let first = compact_micro(&request_from_messages(messages));
        let second = compact_micro(&request_from_messages(first.messages.clone()));
        let third = compact_micro(&request_from_messages(second.messages.clone()));

        assert_eq!(first.messages, second.messages, "pass 1 → 2 must be stable");
        assert_eq!(second.messages, third.messages, "pass 2 → 3 must be stable");
    }
}
