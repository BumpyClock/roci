use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::tempdir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn binary_path() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_roci-agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/roci-agent"))
}

fn run_roci_audio_command(
    args: &[&str],
    envs: &[(&str, &str)],
    input: Option<&[u8]>,
) -> std::process::Output {
    let mut command = Command::new(binary_path());
    command.args(args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    if input.is_some() {
        command.stdin(Stdio::piped());
    }

    for (key, value) in envs {
        command.env(key, value);
    }

    if let Some(payload) = input {
        let mut child = command.spawn().expect("failed to spawn roci-agent");
        let mut stdin = child.stdin.take().expect("failed to capture stdin");
        stdin.write_all(payload).expect("failed to write stdin");
        drop(stdin);
        return child
            .wait_with_output()
            .expect("failed to read command output");
    }

    command.output().expect("failed to run roci-agent command")
}

fn output_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[tokio::test]
async fn audio_transcribe_command_hits_local_openai_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_raw(
                    r#"{"text":"transcribed by mock","language":"en","duration":1.5}"#,
                    "application/json",
                ),
        )
        .mount(&server)
        .await;

    let root = tempdir().expect("create temp dir");
    let input = root.path().join("clip.wav");
    std::fs::write(&input, b"fake-wav-bytes").expect("write fake wav");

    let output = run_roci_audio_command(
        &[
            "audio",
            "transcribe",
            "--input",
            input.to_string_lossy().as_ref(),
            "--language",
            "en",
            "--model",
            "whisper-1",
            "--json",
        ],
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", server.uri().as_str()),
        ],
        None,
    );

    assert!(output.status.success());
    let stdout = output_string(&output.stdout);
    assert!(stdout.contains("\"text\": \"transcribed by mock\""));
    assert!(stdout.contains("\"language\": \"en\""));
    assert_eq!(output_string(&output.stderr), "");
}

#[tokio::test]
async fn audio_speak_command_writes_output_file_from_local_endpoint() {
    let server = MockServer::start().await;
    let audio_payload = b"mock mp3 bytes";
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(audio_payload.to_vec()),
        )
        .mount(&server)
        .await;

    let root = tempdir().expect("create temp dir");
    let output = root.path().join("speech.mp3");

    let process = run_roci_audio_command(
        &[
            "audio",
            "speak",
            "--output",
            output.to_string_lossy().as_ref(),
            "--voice",
            "nova",
            "--speed",
            "1.25",
            "--format",
            "mp3",
            "Hello from CLI",
        ],
        &[
            ("OPENAI_API_KEY", "test-key"),
            ("OPENAI_BASE_URL", server.uri().as_str()),
        ],
        None,
    );

    assert!(process.status.success());
    assert_eq!(
        output_string(&process.stdout).trim(),
        output.display().to_string()
    );
    assert_eq!(std::fs::read(&output).expect("read output"), audio_payload);
    assert_eq!(output_string(&process.stderr), "");
}

#[tokio::test]
async fn audio_transcribe_stdin_requires_mime_type_error() {
    let output = run_roci_audio_command(
        &["audio", "transcribe", "--input", "-", "--language", "en"],
        &[("OPENAI_API_KEY", "test-key")],
        Some(b"audio-bytes"),
    );

    assert!(!output.status.success());
    let stderr = output_string(&output.stderr);
    assert!(
        stderr.contains("MIME type is required when reading audio from stdin"),
        "stderr: {stderr}"
    );
}

#[test]
fn audio_speak_invalid_speed_exits_with_clap_error_message() {
    let output = run_roci_audio_command(
        &[
            "audio",
            "speak",
            "--output",
            "out.mp3",
            "--speed",
            "0.24",
            "bad speed",
        ],
        &[],
        None,
    );

    assert!(!output.status.success());
    assert!(output.status.code().is_some());
    let stderr = output_string(&output.stderr);
    assert!(
        stderr.contains("speech speed must be a finite number between 0.25 and 4.0"),
        "stderr: {stderr}"
    );
}
