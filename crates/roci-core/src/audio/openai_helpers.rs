use super::types::AudioFormat;

pub(super) fn normalize_mime_type(mime_type: &str) -> Option<&str> {
    let normalized = mime_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

pub(super) fn build_transcription_multipart(
    boundary: &str,
    model: &str,
    audio: &[u8],
    mime_type: &str,
    extension: &str,
    language: Option<&str>,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(audio.len() + 512);

    append_field(&mut body, boundary, "model", model);
    if let Some(lang) = language {
        append_field(&mut body, boundary, "language", lang.trim());
    }

    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"audio.{extension}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(audio);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    body
}

fn append_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

pub(super) fn trim_trailing_slash(url: &str) -> &str {
    url.trim_end_matches('/')
}

pub(super) fn is_supported_transcription_mime(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "audio/mpeg"
            | "audio/mp3"
            | "audio/mp4"
            | "video/mp4"
            | "audio/mpga"
            | "audio/x-m4a"
            | "audio/wav"
            | "audio/x-wav"
            | "audio/wave"
            | "audio/webm"
            | "audio/ogg"
            | "audio/flac"
            | "audio/x-flac"
    )
}

pub(super) fn transcription_extension_for_mime(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "audio/mpeg" | "audio/mp3" | "audio/mpga" => Some("mp3"),
        "audio/mp4" | "video/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/webm" => Some("webm"),
        "audio/ogg" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        _ => None,
    }
}

pub(super) fn tts_format_name(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Opus => "opus",
        AudioFormat::Aac => "aac",
        AudioFormat::Flac => "flac",
        AudioFormat::Wav => "wav",
        AudioFormat::Pcm16 => "pcm",
    }
}

pub(super) fn content_type_matches_expected_audio(content_type: &str, format: AudioFormat) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();

    if mime.is_empty() {
        return false;
    }

    match format {
        AudioFormat::Mp3 => matches!(mime, "audio/mpeg" | "audio/mp3"),
        AudioFormat::Opus => matches!(mime, "audio/opus" | "audio/ogg" | "application/ogg"),
        AudioFormat::Aac => matches!(mime, "audio/aac" | "audio/mp4"),
        AudioFormat::Flac => matches!(mime, "audio/flac" | "audio/x-flac"),
        AudioFormat::Wav => matches!(mime, "audio/wav" | "audio/x-wav" | "audio/wave"),
        AudioFormat::Pcm16 => {
            matches!(mime, "audio/pcm" | "audio/l16" | "application/octet-stream")
        }
    }
}
