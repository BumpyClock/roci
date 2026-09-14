use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::tempdir;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn run_roci_audio_command(
    args: &[&str],
    envs: &[(&str, &str)],
    input: Option<&[u8]>,
) -> std::process::Output {
    let home = tempdir().expect("isolated audio command home");
    std::fs::write(home.path().join(".env"), b"").expect("isolate dotenv lookup");
    let mut command = Command::new(env!("CARGO_BIN_EXE_roci-agent"));
    command.current_dir(home.path()).env("HOME", home.path());
    command.args(args);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
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
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_raw(
                    r#"{"text":"transcribed by mock","language":"en","duration":1.5}"#,
                    "application/json",
                ),
        )
        .expect(1)
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

    assert!(output.status.success(), "{}", output_string(&output.stderr));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["text"], "transcribed by mock");
    assert_eq!(result["language"], "en");
    assert_eq!(result["duration_seconds"], 1.5);
    assert_eq!(output_string(&output.stderr), "");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"));
    assert!(body.contains("name=\"language\"\r\n\r\nen\r\n"));
    assert!(body.contains("Content-Type: audio/wav\r\n\r\nfake-wav-bytes\r\n"));
}

#[tokio::test]
async fn audio_speak_command_writes_output_file_from_local_endpoint() {
    for (voice, speed, text) in [
        ("alloy", None, "hello world"),
        ("nova", Some("1.25"), "Hello from CLI"),
    ] {
        let server = MockServer::start().await;
        let audio_payload = b"mock mp3 bytes";
        let mut expected = serde_json::json!({
            "model": "tts-1",
            "input": text,
            "voice": voice,
            "response_format": "mp3"
        });
        if speed.is_some() {
            expected["speed"] = serde_json::json!(1.25);
        }
        Mock::given(method("POST"))
            .and(path("/audio/speech"))
            .and(header("authorization", "Bearer test-key"))
            .and(body_json(expected))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/mpeg")
                    .set_body_bytes(audio_payload.to_vec()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let root = tempdir().expect("create temp dir");
        let output = root.path().join("speech.mp3");
        let path = output.to_str().unwrap();
        let mut args = vec![
            "audio", "speak", "--output", path, "--voice", voice, "--format", "mp3",
        ];
        if let Some(speed) = speed {
            args.extend(["--speed", speed]);
        }
        args.push(text);
        let process = run_roci_audio_command(
            &args,
            &[
                ("OPENAI_API_KEY", "test-key"),
                ("OPENAI_BASE_URL", server.uri().as_str()),
            ],
            None,
        );

        assert!(
            process.status.success(),
            "{}",
            output_string(&process.stderr)
        );
        assert_eq!(output_string(&process.stdout).trim(), path);
        assert_eq!(std::fs::read(&output).expect("read output"), audio_payload);
        assert_eq!(output_string(&process.stderr), "");
    }
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
