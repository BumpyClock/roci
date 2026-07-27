use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::credential::{
    ProviderApiKey, ProviderCredentialRecord, ProviderCredentialStore,
    ProviderCredentialStoreError, ProviderEndpoint,
};
use super::file_credential::FileProviderCredentialStore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommitFailure {
    WriteAfterPrefix,
    Rename,
}

const CHILD_ROOT: &str = "ROCI_TEST_CREDENTIAL_ROOT";
const CHILD_PROVIDER: &str = "ROCI_TEST_CREDENTIAL_PROVIDER";
const CHILD_KEY: &str = "ROCI_TEST_CREDENTIAL_KEY";
const CHILD_READY: &str = "ROCI_TEST_FILE_CREDENTIAL_READY_AFTER_READ";
const CHILD_RELEASE: &str = "ROCI_TEST_FILE_CREDENTIAL_RELEASE_AFTER_READ";
const CHILD_TIMEOUT: Duration = Duration::from_secs(5);

fn sample_record(key: &str, endpoint: &str) -> ProviderCredentialRecord {
    ProviderCredentialRecord::new(
        ProviderApiKey::new(key),
        Some(ProviderEndpoint::new(endpoint)),
    )
}

fn root(temp: &TempDir) -> PathBuf {
    temp.path().join(".roci")
}

fn store(temp: &TempDir) -> FileProviderCredentialStore {
    FileProviderCredentialStore::new(root(temp))
}

fn has_temp_files(root: &Path) -> bool {
    fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("auth.json.tmp.")
    })
}

#[test]
fn explicit_root_uses_auth_file_and_only_chmods_final_directory() {
    let temp = TempDir::new().unwrap();
    let shared_parent = temp.path().join("credentials");
    fs::create_dir(&shared_parent).unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&shared_parent, fs::Permissions::from_mode(0o755)).unwrap();
    let record = sample_record("sk-secret", "https://example.test/v1");
    let store = FileProviderCredentialStore::new(&shared_parent);
    store.save("openai", &record).unwrap();

    let paths = [
        temp.path().to_path_buf(),
        shared_parent.clone(),
        shared_parent.join("auth.json"),
        shared_parent.join("auth.json.lock"),
    ];
    let modes = paths.map(|path| fs::metadata(path).unwrap().permissions().mode() & 0o777);
    assert_eq!(modes, [0o755, 0o700, 0o600, 0o600]);
    assert_eq!(store.load("openai").unwrap(), Some(record));
}

#[test]
fn file_store_round_trips_strict_versioned_map_and_clear() {
    let temp = TempDir::new().unwrap();
    let store = store(&temp);
    let openai = sample_record("sk-openai", "https://openai.test/v1");
    let anthropic = sample_record("sk-anthropic", "https://anthropic.test/v1");
    store.save("openai", &openai).unwrap();
    store.save("anthropic", &anthropic).unwrap();
    store.clear("openai").unwrap();

    let actual = BTreeMap::from([
        ("openai", store.load("openai").unwrap()),
        ("anthropic", store.load("anthropic").unwrap()),
    ]);
    let expected = BTreeMap::from([("openai", None), ("anthropic", Some(anthropic))]);
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(root(&temp).join("auth.json")).unwrap()
        )
        .unwrap(),
        serde_json::json!({
            "anthropic": {
                "version": 1,
                "api_key": "sk-anthropic",
                "endpoint": "https://anthropic.test/v1"
            }
        })
    );
}

#[test]
fn rejects_oversized_input_before_creating_root_or_files() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let record = ProviderCredentialRecord::new(
        ProviderApiKey::new("x".repeat(1024 * 1024)),
        Some(ProviderEndpoint::new("https://example.test")),
    );

    assert_eq!(
        store(&temp).save("openai", &record),
        Err(ProviderCredentialStoreError::InvalidRecord)
    );
    assert!(!root.exists());
    assert!(!root.join("auth.json").exists());
    assert!(!root.join("auth.json.lock").exists());
}

#[test]
fn rejects_final_root_symlink() {
    let temp = TempDir::new().unwrap();
    let destination = temp.path().join("destination");
    let linked_root = temp.path().join("linked-root");
    fs::create_dir(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&destination, &linked_root).unwrap();
    let record = sample_record("sk", "https://example.test");

    assert_eq!(
        FileProviderCredentialStore::new(linked_root).save("openai", &record),
        Err(ProviderCredentialStoreError::Unavailable)
    );
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(fs::read_dir(destination).unwrap().next().is_none());
}

#[test]
fn rejects_symlink_and_non_regular_children_without_touching_destination() {
    for child in ["auth.json.lock", "auth.json"] {
        let temp = TempDir::new().unwrap();
        let symlink_root = root(&temp);
        let destination = temp.path().join("destination");
        fs::create_dir(&symlink_root).unwrap();
        fs::write(&destination, b"unchanged").unwrap();
        symlink(&destination, symlink_root.join(child)).unwrap();
        assert_eq!(
            store(&temp).save("openai", &sample_record("sk", "https://example.test")),
            Err(ProviderCredentialStoreError::Unavailable)
        );
        assert_eq!(fs::read(destination).unwrap(), b"unchanged");

        let temp = TempDir::new().unwrap();
        let root = root(&temp);
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join(child)).unwrap();
        assert_eq!(
            store(&temp).save("openai", &sample_record("sk", "https://example.test")),
            Err(ProviderCredentialStoreError::Unavailable)
        );
    }
}

#[test]
fn invalid_unsupported_unknown_and_oversized_files_fail_closed() {
    let cases: &[(&[u8], ProviderCredentialStoreError)] = &[
        (b"{not-json", ProviderCredentialStoreError::InvalidRecord),
        (
            br#"{"openai":{"version":7,"api_key":"old","endpoint":null}}"#,
            ProviderCredentialStoreError::UnsupportedVersion(7),
        ),
        (
            br#"{"openai":{"version":1,"api_key":"old","endpoint":null,"extra":true}}"#,
            ProviderCredentialStoreError::InvalidRecord,
        ),
    ];
    for (raw, expected) in cases {
        let temp = TempDir::new().unwrap();
        let root = root(&temp);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("auth.json"), raw).unwrap();
        assert_eq!(
            store(&temp).save("other", &sample_record("sk", "https://example.test")),
            Err(expected.clone())
        );
        assert_eq!(fs::read(root.join("auth.json")).unwrap(), *raw);
    }

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    fs::create_dir(&root).unwrap();
    fs::write(root.join("auth.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        store(&temp).load("openai"),
        Err(ProviderCredentialStoreError::InvalidRecord)
    );
}

#[test]
fn injected_commit_failures_preserve_target_and_clean_temps() {
    for failure in [CommitFailure::WriteAfterPrefix, CommitFailure::Rename] {
        let temp = TempDir::new().unwrap();
        let root = root(&temp);
        let initial = sample_record("sk-original", "https://original.test");
        store(&temp).save("openai", &initial).unwrap();
        let original = fs::read(root.join("auth.json")).unwrap();

        assert_eq!(
            FileProviderCredentialStore::with_failure(&root, failure).save(
                "anthropic",
                &sample_record("sk-replacement", "https://replacement.test")
            ),
            Err(ProviderCredentialStoreError::Unavailable)
        );
        assert_eq!(fs::read(root.join("auth.json")).unwrap(), original);
        assert!(!has_temp_files(&root));
        assert_eq!(store(&temp).load("openai").unwrap(), Some(initial));
    }
}

#[test]
fn separate_process_writers_preserve_both_records() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let ready = temp.path().join("ready");
    let release = temp.path().join("release");
    let mut first = spawn_writer(&root, "openai", "sk-openai", Some((&ready, &release)));
    wait_for_marker(&mut first, &ready);
    let second = spawn_writer(&root, "anthropic", "sk-anthropic", None);
    fs::write(release, b"release").unwrap();

    for (name, output) in [
        ("first", wait_for_output(first)),
        ("second", wait_for_output(second)),
    ] {
        assert!(
            output.status.success(),
            "{name} writer failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let store = FileProviderCredentialStore::new(root);
    assert_eq!(
        BTreeMap::from([
            ("openai", store.load("openai").unwrap()),
            ("anthropic", store.load("anthropic").unwrap()),
        ]),
        BTreeMap::from([
            (
                "openai",
                Some(sample_record("sk-openai", "https://child.test/v1"))
            ),
            (
                "anthropic",
                Some(sample_record("sk-anthropic", "https://child.test/v1"))
            ),
        ])
    );
}

fn spawn_writer(root: &Path, provider: &str, key: &str, hold: Option<(&Path, &Path)>) -> Child {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "auth::file_credential_tests::subprocess_writer_helper",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_ROOT, root)
        .env(CHILD_PROVIDER, provider)
        .env(CHILD_KEY, key)
        .env_remove(CHILD_READY)
        .env_remove(CHILD_RELEASE)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some((ready, release)) = hold {
        command.env(CHILD_READY, ready).env(CHILD_RELEASE, release);
    }
    command.spawn().unwrap()
}

fn wait_for_marker(child: &mut Child, marker: &Path) {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    while !marker.exists() {
        assert_eq!(
            child.try_wait().unwrap(),
            None,
            "writer exited before ready"
        );
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("writer never became ready");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_output(mut child: Child) -> Output {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "writer timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn pause_after_read_for_test() {
    let Some(ready) = std::env::var_os(CHILD_READY) else {
        return;
    };
    fs::write(ready, b"ready").unwrap();
    let release = PathBuf::from(std::env::var_os(CHILD_RELEASE).unwrap());
    let deadline = Instant::now() + CHILD_TIMEOUT;
    while !release.exists() {
        assert!(Instant::now() < deadline, "release marker timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "subprocess helper"]
fn subprocess_writer_helper() {
    FileProviderCredentialStore::new(PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap()))
        .save(
            &std::env::var(CHILD_PROVIDER).unwrap(),
            &sample_record(&std::env::var(CHILD_KEY).unwrap(), "https://child.test/v1"),
        )
        .unwrap();
}
