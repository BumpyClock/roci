use super::types::{PromptInput, ResolvedAttachment};

/// Renders resolved text attachments into model-visible prompt text.
#[derive(Debug, Default, Clone, Copy)]
pub struct AttachmentTextRenderer;

impl AttachmentTextRenderer {
    /// Renders `input.text` followed by text attachments. Images are preserved by
    /// `ResolvedAttachment` and intentionally skipped by this text renderer.
    pub fn render_prompt_input_text(
        &self,
        input: &PromptInput,
        resolved: &[ResolvedAttachment],
    ) -> String {
        render_prompt_input_text(input, resolved)
    }

    /// Renders only text attachments. Images return no text.
    pub fn render_resolved_text(&self, resolved: &[ResolvedAttachment]) -> String {
        render_resolved_text(resolved)
    }
}

/// Renders prompt text plus resolved text attachments.
pub fn render_prompt_input_text(input: &PromptInput, resolved: &[ResolvedAttachment]) -> String {
    let attachment_text = render_resolved_text(resolved);
    if attachment_text.is_empty() {
        return input.text.clone();
    }

    if input.text.is_empty() {
        return attachment_text;
    }

    format!("{}\n\n{}", input.text, attachment_text)
}

/// Renders text attachments in a stable model-visible format.
pub fn render_resolved_text(resolved: &[ResolvedAttachment]) -> String {
    let mut output = String::new();

    for attachment in resolved {
        let ResolvedAttachment::Text { text, metadata } = attachment else {
            continue;
        };
        let name = metadata.name.as_deref().unwrap_or("attachment");
        let mime_type = metadata.mime_type.as_deref().unwrap_or("text/plain");

        if !output.is_empty() {
            output.push_str("\n\n");
        }

        output.push_str("--- Attachment: ");
        output.push_str(name);
        output.push_str(" (");
        output.push_str(mime_type);
        output.push_str(") ---\n");
        output.push_str(text);
        output.push_str("\n--- End attachment ---");
    }

    output
}
