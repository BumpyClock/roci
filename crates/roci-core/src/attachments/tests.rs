use std::fs;

use tempfile::tempdir;

use super::*;

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
fn opaque_blob_fails_preflight() {
    let blob = BlobAttachment::new([0, 159, 146, 150]).with_name("opaque");

    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect_err("opaque blob should fail");

    assert!(matches!(
        err,
        AttachmentResolveError::UnsupportedBinary { name } if name == "opaque"
    ));
}

#[test]
fn non_text_non_image_mime_fails_even_when_utf8() {
    let blob = BlobAttachment::new("pdf-ish")
        .with_name("doc.pdf")
        .with_mime_type("application/pdf");

    let err = DefaultAttachmentResolver
        .resolve_attachments(
            &[Attachment::Blob(blob)],
            &AttachmentResolveOptions::default(),
        )
        .expect_err("non-text MIME should fail");

    assert!(matches!(
        err,
        AttachmentResolveError::UnsupportedMime { name, mime_type }
            if name == "doc.pdf" && mime_type == "application/pdf"
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
