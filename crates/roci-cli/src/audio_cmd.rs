use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use roci::audio::{
    AudioFormat, AudioProvider, OpenAiTtsProvider, OpenAiWhisperTranscriptionProvider,
    SpeechProvider, SpeechRequest, TranscriptionResult, Voice,
};
use roci::config::RociConfig;
use roci::error::RociError;
use roci::models::ProviderKey;

use crate::cli::{AudioFormatArg, SpeakArgs, TranscribeArgs};

pub async fn handle_transcribe(args: TranscribeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let result = transcribe_audio(&args).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", result.text);
    }

    Ok(())
}

pub async fn handle_speak(args: SpeakArgs) -> Result<(), Box<dyn std::error::Error>> {
    let audio = synthesize_speech(&args).await?;
    write_output_bytes(&args.output, &audio)?;

    if !is_stdio_path(&args.output) {
        println!("{}", args.output.display());
    }

    Ok(())
}

async fn transcribe_audio(
    args: &TranscribeArgs,
) -> Result<TranscriptionResult, Box<dyn std::error::Error>> {
    let audio = read_input_bytes(&args.input)?;
    let mime_type = resolve_mime_type(&args.input, args.mime_type.as_deref())?;
    let provider = build_transcription_provider(&args.model)?;
    Ok(provider
        .transcribe(&audio, &mime_type, args.language.as_deref())
        .await?)
}

async fn synthesize_speech(args: &SpeakArgs) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let provider = build_tts_provider(&args.model)?;
    let request = SpeechRequest {
        text: args.text.clone(),
        voice: Voice {
            id: args.voice.clone(),
            name: None,
            provider: ProviderKey::OpenAi.as_str().to_string(),
        },
        format: args.format.clone().into(),
        speed: args.speed,
    };
    Ok(provider.generate_speech(&request).await?)
}

fn build_transcription_provider(
    model: &str,
) -> Result<OpenAiWhisperTranscriptionProvider, Box<dyn std::error::Error>> {
    let (api_key, base_url) = require_openai_credentials()?;
    let provider = match base_url {
        Some(base_url) => OpenAiWhisperTranscriptionProvider::new_with_base_url(api_key, base_url),
        None => OpenAiWhisperTranscriptionProvider::new(api_key),
    };
    Ok(provider.with_model(model.to_string()))
}

fn build_tts_provider(model: &str) -> Result<OpenAiTtsProvider, Box<dyn std::error::Error>> {
    let (api_key, base_url) = require_openai_credentials()?;
    let provider = match base_url {
        Some(base_url) => OpenAiTtsProvider::new_with_base_url(api_key, base_url),
        None => OpenAiTtsProvider::new(api_key),
    };
    Ok(provider.with_model(model.to_string()))
}

fn require_openai_credentials() -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
    let config = RociConfig::from_env();
    let api_key = config
        .get_api_key_for(ProviderKey::OpenAi)
        .ok_or_else(|| RociError::Authentication("Missing OPENAI_API_KEY".into()))?;
    let base_url = config.get_base_url_for(ProviderKey::OpenAi);
    Ok((api_key, base_url))
}

fn read_input_bytes(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if is_stdio_path(path) {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes)?;
        return Ok(bytes);
    }

    Ok(fs::read(path)?)
}

fn write_output_bytes(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if is_stdio_path(path) {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(bytes)?;
        stdout.flush()?;
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn resolve_mime_type(input: &Path, explicit: Option<&str>) -> Result<String, RociError> {
    if let Some(mime_type) = explicit {
        let trimmed = mime_type.trim();
        if trimmed.is_empty() {
            return Err(RociError::InvalidArgument(
                "MIME type cannot be empty".to_string(),
            ));
        }
        return Ok(trimmed.to_string());
    }

    infer_mime_type(input).map(str::to_string).ok_or_else(|| {
        if is_stdio_path(input) {
            RociError::InvalidArgument(
                "MIME type is required when reading audio from stdin".to_string(),
            )
        } else {
            RociError::InvalidArgument(format!(
                "Could not infer MIME type from '{}'; pass --mime-type",
                input.display()
            ))
        }
    })
}

fn infer_mime_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "mp3" => Some("audio/mpeg"),
        "mp4" | "m4a" => Some("audio/mp4"),
        "wav" | "wave" => Some("audio/wav"),
        "webm" => Some("audio/webm"),
        "ogg" | "oga" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        _ => None,
    }
}

fn is_stdio_path(path: &Path) -> bool {
    path.as_os_str() == "-"
}

impl From<AudioFormatArg> for AudioFormat {
    fn from(value: AudioFormatArg) -> Self {
        match value {
            AudioFormatArg::Mp3 => Self::Mp3,
            AudioFormatArg::Opus => Self::Opus,
            AudioFormatArg::Aac => Self::Aac,
            AudioFormatArg::Flac => Self::Flac,
            AudioFormatArg::Wav => Self::Wav,
            AudioFormatArg::Pcm16 => Self::Pcm16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn infer_mime_type_from_supported_extensions() {
        assert_eq!(infer_mime_type(Path::new("sample.mp3")), Some("audio/mpeg"));
        assert_eq!(infer_mime_type(Path::new("sample.m4a")), Some("audio/mp4"));
        assert_eq!(infer_mime_type(Path::new("sample.wav")), Some("audio/wav"));
        assert_eq!(
            infer_mime_type(Path::new("sample.webm")),
            Some("audio/webm")
        );
        assert_eq!(infer_mime_type(Path::new("sample.ogg")), Some("audio/ogg"));
        assert_eq!(
            infer_mime_type(Path::new("sample.flac")),
            Some("audio/flac")
        );
    }

    #[test]
    fn resolve_mime_type_requires_explicit_value_for_stdin() {
        let error = resolve_mime_type(Path::new("-"), None).unwrap_err();
        assert!(error
            .to_string()
            .contains("MIME type is required when reading audio from stdin"));
    }

    #[test]
    fn write_output_bytes_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("audio.mp3");

        write_output_bytes(&path, b"audio").unwrap();

        assert_eq!(fs::read(path).unwrap(), b"audio");
    }

    #[test]
    fn audio_format_arg_maps_to_core_format() {
        assert_eq!(AudioFormat::from(AudioFormatArg::Mp3), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from(AudioFormatArg::Pcm16), AudioFormat::Pcm16);
    }

    #[test]
    fn unsupported_extension_requires_explicit_mime_type() {
        let error = resolve_mime_type(&PathBuf::from("sample.bin"), None).unwrap_err();
        assert!(error.to_string().contains("pass --mime-type"));
    }
}
