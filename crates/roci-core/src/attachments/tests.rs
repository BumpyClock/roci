use std::fs;

use tempfile::tempdir;

use super::*;
use crate::models::{ImageInputCapabilities, ModelCapabilities, ModelInputCapabilities};
use crate::types::ContentPart;

fn tight_options() -> AttachmentResolveOptions {
    AttachmentResolveOptions {
        max_attachments: 2,
        max_attachment_bytes: 16,
        max_total_bytes: 24,
    }
}

#[test]
fn selection_resolves_to_model_visible_text() {
    let selection = SelectionAttachment::new("selected text").with_name("Selection A");
    let input = PromptInput::new("Inspect this").with_attachment(Attachment::Selection(selection));

    let resolved = DefaultAttachmentResolver
        .resolve_prompt_input(&input, &AttachmentResolveOptions::default())
        .expect("selection should resolve");

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].as_text(), Some("selected text"));
    let rendered = render_prompt_input_text(&input, &resolved);
    assert!(rendered.contains("Inspect this"));
    assert!(rendered.contains("--- Attachment: Selection A"));
    assert!(rendered.contains("selected text"));
}

#[test]
fn text_file_uses_mime_guess_and_renders_text() {
    let dir = tempdir().expect("tempdir should be created");
    let path = dir.path().join("notes.txt");
    fs::write(&path, "hello from file").expect("fixture should be written");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::file(&path)],
            &AttachmentResolveOptions::default(),
        )
        .expect("text file should resolve");

    assert_eq!(resolved[0].as_text(), Some("hello from file"));
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("text/plain")
    );
}

#[test]
fn unknown_utf8_blob_falls_back_to_text() {
    let blob = BlobAttachment::new("plain utf8".as_bytes()).with_name("scratch");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("utf8 blob should resolve as text");

    assert_eq!(resolved[0].as_text(), Some("plain utf8"));
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );
}

#[test]
fn image_blob_preserves_data() {
    let image = BlobAttachment::new([137, 80, 78, 71])
        .with_name("pixel.png")
        .with_mime_type("image/png");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(image)],
            &AttachmentResolveOptions::default(),
        )
        .expect("image should resolve");

    assert_eq!(resolved[0].as_image_data(), Some(&[137, 80, 78, 71][..]));
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("image/png")
    );
}

#[test]
fn unsupported_media_opaque_blob_resolves_to_marker() {
    let blob = BlobAttachment::new([0, 159, 146, 150]).with_name("opaque.bin");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("opaque blob should resolve as marker text");

    let text = resolved[0].as_text().expect("marker should be text");
    assert_eq!(
        text,
        "User attached unsupported media: opaque.bin (application/octet-stream, 4 bytes). Content omitted."
    );
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("application/octet-stream")
    );
}

#[test]
fn unsupported_media_non_text_non_image_mime_resolves_to_marker() {
    let blob = BlobAttachment::new("pdf-ish")
        .with_name("doc.pdf")
        .with_mime_type("application/pdf");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("unsupported MIME should resolve as marker text");

    let text = resolved[0].as_text().expect("marker should be text");
    assert_eq!(
        text,
        "User attached unsupported media: doc.pdf (application/pdf, 7 bytes). Content omitted."
    );
    assert_eq!(
        resolved[0].metadata().mime_type.as_deref(),
        Some("application/pdf")
    );
}

#[test]
fn unsupported_media_marker_bounds_display_name() {
    let long_name = format!("{}{}", "/tmp/private/", "x".repeat(512));
    let blob = BlobAttachment::new([0, 159, 146, 150])
        .with_name(long_name)
        .with_mime_type("application/pdf");

    let resolved = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect("unsupported media should resolve as bounded marker text");

    let text = resolved[0].as_text().expect("marker should be text");
    assert!(text.contains("User attached unsupported media: "));
    assert!(text.contains("application/pdf"));
    assert!(!text.contains("/tmp/private"));
    assert!(text.len() < 256);
}

#[test]
fn unsupported_media_path_like_mime_errors() {
    let blob = BlobAttachment::new([0, 159, 146, 150])
        .with_name("opaque")
        .with_mime_type("/tmp/private");

    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect_err("malformed MIME should not produce marker");

    assert!(matches!(
        err,
        AttachmentResolveError::UnsupportedMime { name, mime_type }
            if name == "opaque" && mime_type == "/tmp/private"
    ));
}

#[test]
fn unsupported_media_overlong_mime_errors() {
    let mime_type = format!("application/{}", "x".repeat(256));
    let blob = BlobAttachment::new([0, 159, 146, 150])
        .with_name("opaque")
        .with_mime_type(mime_type.clone());

    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect_err("overlong MIME should not produce marker");

    assert!(matches!(
        err,
        AttachmentResolveError::UnsupportedMime { name, mime_type: actual }
            if name == "opaque" && actual == mime_type
    ));
}

#[test]
fn declared_text_mime_with_invalid_utf8_still_errors() {
    let blob = BlobAttachment::new([0xff])
        .with_name("bad.txt")
        .with_mime_type("text/plain");

    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect_err("declared text must be valid UTF-8");

    assert!(matches!(
        err,
        AttachmentResolveError::InvalidUtf8 { name } if name == "bad.txt"
    ));
}

#[test]
fn count_limit_fails_before_resolution() {
    let attachments = vec![
        Attachment::selection("one"),
        Attachment::selection("two"),
        Attachment::selection("three"),
    ];

    let err = DefaultAttachmentResolver
        .resolve_attachments(&attachments, &tight_options())
        .expect_err("count limit should fail");

    assert!(matches!(
        err,
        AttachmentResolveError::CountLimitExceeded { count: 3, max: 2 }
    ));
}

#[test]
fn per_attachment_size_limit_fails() {
    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::selection("this selection is too long")],
            &tight_options(),
        )
        .expect_err("size limit should fail");

    assert!(matches!(
        err,
        AttachmentResolveError::AttachmentLimitExceeded { name, size, max }
            if name == "selection" && size > max
    ));
}

#[test]
fn total_size_limit_fails() {
    let options = AttachmentResolveOptions {
        max_attachments: 3,
        max_attachment_bytes: 16,
        max_total_bytes: 10,
    };
    let attachments = vec![
        Attachment::selection("123456"),
        Attachment::selection("abcdef"),
    ];

    let err = DefaultAttachmentResolver
        .resolve_attachments(&attachments, &options)
        .expect_err("total size limit should fail");

    assert!(matches!(
        err,
        AttachmentResolveError::TotalLimitExceeded { size: 12, max: 10 }
    ));
}

#[test]
fn compile_prompt_input_renders_text_and_sanitizes_metadata() {
    let input = PromptInput::new("Inspect").with_attachment(Attachment::Selection(
        SelectionAttachment::new("selected text").with_name("Selection A"),
    ));
    let caps = ModelCapabilities::default();

    let compiled = compile_prompt_input(&input, &caps).expect("input should compile");

    assert_eq!(compiled.message.role, crate::types::Role::User);
    assert!(compiled.message.text().contains("Inspect"));
    assert!(compiled.message.text().contains("selected text"));
    let metadata = compiled
        .message
        .metadata
        .as_ref()
        .expect("metadata should exist");
    assert_eq!(metadata.attachments.len(), 1);
    assert_eq!(
        metadata.attachments[0].source_kind,
        AttachmentSourceKind::Selection
    );
    assert_eq!(
        metadata.attachments[0].content_kind,
        AttachmentContentKind::Text
    );
    assert_eq!(metadata.attachments[0].name.as_deref(), Some("Selection A"));
}

#[test]
fn compile_prompt_input_encodes_images_when_model_supports_vision() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([137, 80, 78, 71]).with_mime_type("image/png"),
    ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities::from_vision_support(true),
        ..ModelCapabilities::default()
    };

    let compiled = compile_prompt_input(&input, &caps).expect("image should compile");

    assert!(matches!(
        &compiled.message.content[1],
        ContentPart::Image(image) if image.mime_type == "image/png" && !image.data.is_empty()
    ));
    let metadata = compiled.message.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.attachments[0].source_kind,
        AttachmentSourceKind::Blob
    );
    assert_eq!(
        metadata.attachments[0].content_kind,
        AttachmentContentKind::Image
    );
}

#[test]
fn unsupported_media_file_marker_uses_file_name_not_raw_path() {
    let dir = tempdir().expect("tempdir should be created");
    let path = dir.path().join("private.pdf");
    fs::write(&path, [0, 159, 146, 150]).expect("fixture should be written");
    let input = PromptInput::new("Inspect").with_attachment(Attachment::file(&path));

    let compiled =
        compile_prompt_input(&input, &ModelCapabilities::default()).expect("marker should compile");

    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: private.pdf"));
    assert!(text.contains("application/pdf"));
    assert!(!text.contains(dir.path().to_string_lossy().as_ref()));

    let json = serde_json::to_string(&compiled.message.metadata).expect("metadata serializes");
    assert!(json.contains("private.pdf"));
    assert!(!json.contains(dir.path().to_string_lossy().as_ref()));
}

#[test]
fn unsupported_media_image_converts_to_marker_for_non_vision_model() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([137, 80, 78, 71])
            .with_name("pixel.png")
            .with_mime_type("image/png"),
    ));

    let compiled =
        compile_prompt_input(&input, &ModelCapabilities::default()).expect("marker should compile");

    assert_eq!(compiled.message.content.len(), 1);
    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: pixel.png"));
    assert!(text.contains("image/png"));
    assert!(text.contains("Content omitted."));
    let metadata = compiled.message.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.attachments[0].content_kind,
        AttachmentContentKind::Text
    );
}

#[test]
fn unsupported_media_image_mime_converts_to_marker_for_vision_model() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([71, 73, 70, 56])
            .with_name("motion.gif")
            .with_mime_type("image/gif"),
    ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities {
            image: Some(ImageInputCapabilities {
                supported_mime_types: vec!["image/png".to_string()],
                ..ImageInputCapabilities::default()
            }),
            ..ModelInputCapabilities::default()
        },
        ..ModelCapabilities::default()
    };

    let compiled = compile_prompt_input(&input, &caps).expect("marker should compile");

    let text = compiled.message.text();
    assert!(text.contains("User attached unsupported media: motion.gif"));
    assert!(text.contains("image/gif"));
    let metadata = compiled.message.metadata.as_ref().expect("metadata");
    assert_eq!(
        metadata.attachments[0].content_kind,
        AttachmentContentKind::Text
    );
}

#[test]
fn unsupported_media_oversized_image_still_errors() {
    let input = PromptInput::new("Describe").with_attachment(Attachment::Blob(
        BlobAttachment::new([71, 73, 70, 56])
            .with_name("large.gif")
            .with_mime_type("image/gif"),
    ));
    let caps = ModelCapabilities {
        supports_vision: true,
        input: ModelInputCapabilities {
            image: Some(ImageInputCapabilities {
                max_images: 8,
                max_image_bytes: Some(2),
                max_total_image_bytes: Some(10),
                supported_mime_types: vec!["image/png".to_string()],
                image_token_estimate: 85,
            }),
            ..ModelInputCapabilities::default()
        },
        ..ModelCapabilities::default()
    };

    let err = compile_prompt_input(&input, &caps).expect_err("oversized image should fail");

    assert!(err.to_string().contains("too large"));
}

#[test]
fn compile_prompt_input_file_metadata_omits_raw_path() {
    let dir = tempdir().expect("tempdir should be created");
    let path = dir.path().join("secret-notes.txt");
    fs::write(&path, "private path should stay local").expect("fixture should be written");
    let input = PromptInput::new("Inspect").with_attachment(Attachment::file(&path));

    let compiled =
        compile_prompt_input(&input, &ModelCapabilities::default()).expect("file should compile");

    let json = serde_json::to_string(&compiled.message.metadata).expect("metadata serializes");
    assert!(json.contains("secret-notes.txt"));
    assert!(!json.contains(dir.path().to_string_lossy().as_ref()));
}
