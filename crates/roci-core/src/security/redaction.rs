use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKind {
    PrivateKey,
    AuthHeader,
    BearerToken,
    ApiKey,
    EnvSecret,
    GenericSecret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretLocation {
    TextRange { start: usize, end: usize },
    JsonPointer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub kind: SecretKind,
    pub location: SecretLocation,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRedaction<T> {
    pub redacted: T,
    pub matches: Vec<SecretMatch>,
}

pub struct SecretRedactor {
    patterns: Vec<SecretPattern>,
}

struct SecretPattern {
    kind: SecretKind,
    regex: Regex,
    capture_indices: Vec<usize>,
}

impl SecretRedactor {
    pub fn new_default() -> Self {
        Self {
            patterns: vec![
                pattern(
                    SecretKind::PrivateKey,
                    r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
                    &[],
                ),
                pattern(
                    SecretKind::AuthHeader,
                    r"(?im)\bauthorization\s*:\s*(?:bearer|basic)\s+[^\r\n]+",
                    &[],
                ),
                pattern(
                    SecretKind::BearerToken,
                    r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{8,}",
                    &[],
                ),
                pattern(
                    SecretKind::ApiKey,
                    r"\b(?:sk|rk|pk|ghp|xox[baprs])-[A-Za-z0-9][A-Za-z0-9_-]{5,}",
                    &[],
                ),
                pattern(
                    SecretKind::EnvSecret,
                    r#"(?i)\b[A-Z0-9_]*(?:API[_-]?KEY|TOKEN|SECRET|PRIVATE[_-]?KEY|ACCESS[_-]?KEY)[A-Z0-9_]*\s*[:=]\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s"',;]+))"#,
                    &[1, 2, 3],
                ),
                pattern(
                    SecretKind::GenericSecret,
                    r#"(?i)\b(?:password|passwd|pwd)\s*[:=]\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^\s"',;]+))"#,
                    &[1, 2, 3],
                ),
            ],
        }
    }

    pub fn scan_text(&self, input: &str) -> Vec<SecretMatch> {
        let mut candidates = Vec::new();

        for pattern in &self.patterns {
            for captures in pattern.regex.captures_iter(input) {
                let Some(matched) = pattern
                    .capture_indices
                    .iter()
                    .find_map(|capture_index| captures.get(*capture_index))
                else {
                    continue;
                };

                if matched.start() == matched.end() {
                    continue;
                }

                candidates.push(RawSecretMatch {
                    kind: pattern.kind.clone(),
                    start: matched.start(),
                    end: matched.end(),
                });
            }
        }

        candidates.sort_by(|left, right| {
            left.start
                .cmp(&right.start)
                .then_with(|| right.end.cmp(&left.end))
                .then_with(|| kind_priority(&left.kind).cmp(&kind_priority(&right.kind)))
        });

        let mut kept = Vec::new();
        let mut covered_until = 0;
        for candidate in candidates {
            if candidate.start < covered_until {
                continue;
            }
            covered_until = candidate.end;
            kept.push(SecretMatch {
                replacement: replacement_for(&candidate.kind).to_string(),
                kind: candidate.kind,
                location: SecretLocation::TextRange {
                    start: candidate.start,
                    end: candidate.end,
                },
            });
        }

        kept
    }

    pub fn redact_text(&self, input: &str) -> SecretRedaction<String> {
        let matches = self.scan_text(input);
        let mut redacted = String::with_capacity(input.len());
        let mut cursor = 0;

        for secret_match in &matches {
            let SecretLocation::TextRange { start, end } = &secret_match.location else {
                continue;
            };
            redacted.push_str(&input[cursor..*start]);
            redacted.push_str(&secret_match.replacement);
            cursor = *end;
        }
        redacted.push_str(&input[cursor..]);

        SecretRedaction { redacted, matches }
    }

    pub fn redact_json(&self, input: &serde_json::Value) -> SecretRedaction<serde_json::Value> {
        let mut matches = Vec::new();
        let redacted = self.redact_json_value(input, "", &mut matches);

        SecretRedaction { redacted, matches }
    }

    fn redact_json_value(
        &self,
        input: &serde_json::Value,
        pointer: &str,
        matches: &mut Vec<SecretMatch>,
    ) -> serde_json::Value {
        match input {
            serde_json::Value::String(value) => {
                let report = self.redact_text(value);
                for secret_match in report.matches {
                    matches.push(SecretMatch {
                        kind: secret_match.kind,
                        location: SecretLocation::JsonPointer(pointer.to_string()),
                        replacement: secret_match.replacement,
                    });
                }
                serde_json::Value::String(report.redacted)
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        self.redact_json_value(
                            value,
                            &join_json_pointer(pointer, &index.to_string()),
                            matches,
                        )
                    })
                    .collect(),
            ),
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            self.redact_json_value(
                                value,
                                &join_json_pointer(pointer, key),
                                matches,
                            ),
                        )
                    })
                    .collect(),
            ),
            value => value.clone(),
        }
    }
}

struct RawSecretMatch {
    kind: SecretKind,
    start: usize,
    end: usize,
}

fn pattern(kind: SecretKind, regex: &str, capture_indices: &[usize]) -> SecretPattern {
    SecretPattern {
        kind,
        regex: Regex::new(regex).expect("default secret regex should compile"),
        capture_indices: if capture_indices.is_empty() {
            vec![0]
        } else {
            capture_indices.to_vec()
        },
    }
}

fn replacement_for(kind: &SecretKind) -> &'static str {
    match kind {
        SecretKind::PrivateKey => "[REDACTED_PRIVATE_KEY]",
        SecretKind::AuthHeader => "[REDACTED_AUTH_HEADER]",
        SecretKind::BearerToken => "[REDACTED_TOKEN]",
        SecretKind::ApiKey => "[REDACTED_API_KEY]",
        SecretKind::EnvSecret | SecretKind::GenericSecret => "[REDACTED_SECRET]",
    }
}

fn kind_priority(kind: &SecretKind) -> u8 {
    match kind {
        SecretKind::PrivateKey => 0,
        SecretKind::AuthHeader => 1,
        SecretKind::BearerToken => 2,
        SecretKind::ApiKey => 3,
        SecretKind::EnvSecret => 4,
        SecretKind::GenericSecret => 5,
    }
}

fn join_json_pointer(parent: &str, token: &str) -> String {
    let escaped = token.replace('~', "~0").replace('/', "~1");
    if parent.is_empty() {
        format!("/{escaped}")
    } else {
        format!("{parent}/{escaped}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_json_string_values_with_json_pointer_offsets() {
        let value = json!({"api": {"key": "sk-abc123"}, "keep": "visible"});
        let report = SecretRedactor::new_default().redact_json(&value);

        assert_eq!(report.redacted["api"]["key"], "[REDACTED_API_KEY]");
        assert_eq!(report.redacted["keep"], "visible");
        assert_eq!(
            report.matches[0].location,
            SecretLocation::JsonPointer("/api/key".to_string())
        );
    }

    #[test]
    fn redacts_text_with_utf8_byte_offsets() {
        let report = SecretRedactor::new_default().redact_text("token=sk-abc123 café");

        assert_eq!(report.redacted, "token=[REDACTED_API_KEY] café");
        assert_eq!(
            report.matches[0].location,
            SecretLocation::TextRange { start: 6, end: 15 }
        );
    }

    #[test]
    fn preserves_json_keys() {
        let value = json!({"sk-abc123": "value"});
        let report = SecretRedactor::new_default().redact_json(&value);

        assert_eq!(report.redacted["sk-abc123"], "value");
        assert!(report.matches.is_empty());
    }

    #[test]
    fn overlapping_matches_keep_highest_priority() {
        let report = SecretRedactor::new_default().redact_text("Authorization: Bearer sk-abc123");

        assert_eq!(report.redacted, "[REDACTED_AUTH_HEADER]");
        assert_eq!(report.matches[0].kind, SecretKind::AuthHeader);
    }

    #[test]
    fn default_replacement_tokens_match_secret_kinds() {
        let redactor = SecretRedactor::new_default();
        let cases = [
            (
                "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----",
                SecretKind::PrivateKey,
                "[REDACTED_PRIVATE_KEY]",
            ),
            (
                "Authorization: Basic abc123",
                SecretKind::AuthHeader,
                "[REDACTED_AUTH_HEADER]",
            ),
            (
                "Bearer abcdef123456",
                SecretKind::BearerToken,
                "[REDACTED_TOKEN]",
            ),
            ("sk-abc123", SecretKind::ApiKey, "[REDACTED_API_KEY]"),
            (
                "OPENAI_API_KEY=plain-secret",
                SecretKind::EnvSecret,
                "[REDACTED_SECRET]",
            ),
            (
                "password=plain-secret",
                SecretKind::GenericSecret,
                "[REDACTED_SECRET]",
            ),
        ];

        for (input, kind, token) in cases {
            let report = redactor.redact_text(input);
            assert_eq!(report.matches[0].kind, kind);
            assert_eq!(report.matches[0].replacement, token);
            assert!(report.redacted.contains(token));
        }
    }

    #[test]
    fn redacts_quoted_env_secret_inner_value() {
        let report = SecretRedactor::new_default().redact_text(r#"PASSWORD="hunter2""#);

        assert_eq!(report.redacted, r#"PASSWORD="[REDACTED_SECRET]""#);
        assert_eq!(report.matches[0].kind, SecretKind::GenericSecret);
        assert_eq!(
            report.matches[0].location,
            SecretLocation::TextRange { start: 10, end: 17 }
        );
    }

    #[test]
    fn redacts_single_quoted_env_secret_inner_value() {
        let report = SecretRedactor::new_default().redact_text("TOKEN='plain-secret'");

        assert_eq!(report.redacted, "TOKEN='[REDACTED_SECRET]'");
        assert_eq!(report.matches[0].kind, SecretKind::EnvSecret);
        assert_eq!(
            report.matches[0].location,
            SecretLocation::TextRange { start: 7, end: 19 }
        );
    }

    #[test]
    fn default_detector_regression_corpus_covers_each_kind() {
        // Durable corpus: each row asserts the detector kind a default redactor
        // selects for a representative input. Positive rows must redact to the
        // expected kind + replacement token; negative rows must produce no
        // matches so future regex tweaks cannot silently widen scope.
        struct Row {
            input: &'static str,
            expected: Option<(SecretKind, &'static str)>,
        }
        let redactor = SecretRedactor::new_default();
        let rows: &[Row] = &[
            // PrivateKey: PEM blocks of multiple flavors.
            Row {
                input: "-----BEGIN PRIVATE KEY-----\nABCD\n-----END PRIVATE KEY-----",
                expected: Some((SecretKind::PrivateKey, "[REDACTED_PRIVATE_KEY]")),
            },
            Row {
                input: "-----BEGIN RSA PRIVATE KEY-----\nXYZ\n-----END RSA PRIVATE KEY-----",
                expected: Some((SecretKind::PrivateKey, "[REDACTED_PRIVATE_KEY]")),
            },
            Row {
                input: "-----BEGIN OPENSSH PRIVATE KEY-----\nQQ\n-----END OPENSSH PRIVATE KEY-----",
                expected: Some((SecretKind::PrivateKey, "[REDACTED_PRIVATE_KEY]")),
            },
            // AuthHeader: bearer/basic variants are higher priority than the
            // bare BearerToken / ApiKey patterns nested inside them.
            Row {
                input: "Authorization: Bearer abcdef123456",
                expected: Some((SecretKind::AuthHeader, "[REDACTED_AUTH_HEADER]")),
            },
            Row {
                input: "authorization: basic dXNlcjpwYXNz",
                expected: Some((SecretKind::AuthHeader, "[REDACTED_AUTH_HEADER]")),
            },
            // BearerToken: standalone bearer prefix outside an Authorization header.
            Row {
                input: "token=Bearer abcdef123456",
                expected: Some((SecretKind::BearerToken, "[REDACTED_TOKEN]")),
            },
            // ApiKey: all default vendor prefixes.
            Row {
                input: "sk-abc123def",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "rk-abc123def",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "pk-abc123def",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "ghp-abc123def456",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "xoxb-1234-abcdef",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "xoxa-1234-abcdef",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "xoxp-1234-abcdef",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "xoxr-1234-abcdef",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            Row {
                input: "xoxs-1234-abcdef",
                expected: Some((SecretKind::ApiKey, "[REDACTED_API_KEY]")),
            },
            // EnvSecret: assorted *_KEY/_TOKEN/_SECRET assignments with quoting variants.
            Row {
                input: "OPENAI_API_KEY=plain-secret",
                expected: Some((SecretKind::EnvSecret, "[REDACTED_SECRET]")),
            },
            Row {
                input: "AWS_SECRET_ACCESS_KEY=\"plain-secret\"",
                expected: Some((SecretKind::EnvSecret, "[REDACTED_SECRET]")),
            },
            Row {
                input: "GITHUB_TOKEN: ghs_value",
                expected: Some((SecretKind::EnvSecret, "[REDACTED_SECRET]")),
            },
            Row {
                input: "DEPLOY_PRIVATE_KEY='inline-secret'",
                expected: Some((SecretKind::EnvSecret, "[REDACTED_SECRET]")),
            },
            // GenericSecret: password/passwd/pwd.
            Row {
                input: "password=hunter2",
                expected: Some((SecretKind::GenericSecret, "[REDACTED_SECRET]")),
            },
            Row {
                input: "passwd: hunter2",
                expected: Some((SecretKind::GenericSecret, "[REDACTED_SECRET]")),
            },
            Row {
                input: "pwd=\"hunter2\"",
                expected: Some((SecretKind::GenericSecret, "[REDACTED_SECRET]")),
            },
            // Negative cases: must not redact innocuous text. These guard
            // against future regex broadening.
            Row {
                input: "the api documentation is online",
                expected: None,
            },
            Row {
                input: "sk-",
                expected: None,
            },
            Row {
                input: "bearer",
                expected: None,
            },
            Row {
                input: "password reset link sent",
                expected: None,
            },
            Row {
                input: "BEGIN PRIVATE KEY without delimiters",
                expected: None,
            },
        ];

        for row in rows {
            let report = redactor.redact_text(row.input);
            match &row.expected {
                Some((expected_kind, expected_token)) => {
                    assert_eq!(
                        report.matches.len(),
                        1,
                        "expected exactly one match for {:?}, got {:?}",
                        row.input,
                        report.matches
                    );
                    assert_eq!(
                        &report.matches[0].kind, expected_kind,
                        "wrong kind for {:?}",
                        row.input
                    );
                    assert_eq!(
                        report.matches[0].replacement.as_str(),
                        *expected_token,
                        "wrong replacement for {:?}",
                        row.input
                    );
                    assert!(
                        report.redacted.contains(expected_token),
                        "redacted output {:?} missing token {} for {:?}",
                        report.redacted,
                        expected_token,
                        row.input
                    );
                }
                None => {
                    assert!(
                        report.matches.is_empty(),
                        "negative case {:?} unexpectedly matched {:?}",
                        row.input,
                        report.matches
                    );
                    assert_eq!(report.redacted, row.input);
                }
            }
        }
    }

    #[test]
    fn redacts_nested_json_paths_and_arrays() {
        let value = json!({"a/b": [{"token~key": "Bearer abcdef123456"}]});
        let report = SecretRedactor::new_default().redact_json(&value);

        assert_eq!(report.redacted["a/b"][0]["token~key"], "[REDACTED_TOKEN]");
        assert_eq!(
            report.matches[0].location,
            SecretLocation::JsonPointer("/a~1b/0/token~0key".to_string())
        );
    }
}
