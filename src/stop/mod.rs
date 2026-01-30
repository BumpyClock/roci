//! Stop conditions for streaming generation.

use async_trait::async_trait;

/// Trait for conditions that can stop a text stream early.
#[async_trait]
pub trait StopCondition: Send + Sync {
    /// Check if generation should stop given the accumulated text and current delta.
    async fn should_stop(&self, text: &str, delta: Option<&str>) -> bool;

    /// Reset internal state (for reuse across generations).
    async fn reset(&self);
}

/// Stop when a specific string is found in the output.
pub struct StringStop {
    pattern: String,
}

impl StringStop {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }
}

#[async_trait]
impl StopCondition for StringStop {
    async fn should_stop(&self, text: &str, _delta: Option<&str>) -> bool {
        text.contains(&self.pattern)
    }
    async fn reset(&self) {}
}

/// Stop when a regex pattern matches.
pub struct RegexStop {
    regex: regex::Regex,
}

impl RegexStop {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            regex: regex::Regex::new(pattern)?,
        })
    }
}

#[async_trait]
impl StopCondition for RegexStop {
    async fn should_stop(&self, text: &str, _delta: Option<&str>) -> bool {
        self.regex.is_match(text)
    }
    async fn reset(&self) {}
}

/// Stop after a certain number of tokens (estimated by character count / 4).
pub struct TokenCountStop {
    max_tokens: usize,
}

impl TokenCountStop {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }
}

#[async_trait]
impl StopCondition for TokenCountStop {
    async fn should_stop(&self, text: &str, _delta: Option<&str>) -> bool {
        // Rough estimate: 1 token ≈ 4 chars
        text.len() / 4 >= self.max_tokens
    }
    async fn reset(&self) {}
}

/// Stop after a timeout duration.
pub struct TimeoutStop {
    deadline: std::sync::Mutex<Option<std::time::Instant>>,
    duration: std::time::Duration,
}

impl TimeoutStop {
    pub fn new(duration: std::time::Duration) -> Self {
        Self {
            deadline: std::sync::Mutex::new(None),
            duration,
        }
    }
}

#[async_trait]
impl StopCondition for TimeoutStop {
    async fn should_stop(&self, _text: &str, _delta: Option<&str>) -> bool {
        let mut deadline = self.deadline.lock().unwrap();
        let dl = *deadline.get_or_insert_with(|| std::time::Instant::now() + self.duration);
        std::time::Instant::now() >= dl
    }

    async fn reset(&self) {
        *self.deadline.lock().unwrap() = None;
    }
}

/// Stop when a custom predicate returns true.
pub struct PredicateStop<F: Fn(&str) -> bool + Send + Sync> {
    predicate: F,
}

impl<F: Fn(&str) -> bool + Send + Sync> PredicateStop<F> {
    pub fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

#[async_trait]
impl<F: Fn(&str) -> bool + Send + Sync> StopCondition for PredicateStop<F> {
    async fn should_stop(&self, text: &str, _delta: Option<&str>) -> bool {
        (self.predicate)(text)
    }
    async fn reset(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn string_stop_matches() {
        let stop = StringStop::new("END");
        assert!(!stop.should_stop("Hello", None).await);
        assert!(stop.should_stop("Hello END world", None).await);
    }

    #[tokio::test]
    async fn regex_stop_matches() {
        let stop = RegexStop::new(r"\d{3}").unwrap();
        assert!(!stop.should_stop("abc", None).await);
        assert!(stop.should_stop("abc123", None).await);
    }

    #[tokio::test]
    async fn token_count_stop() {
        let stop = TokenCountStop::new(5);
        assert!(!stop.should_stop("hi", None).await); // 2 chars ≈ 0 tokens
        assert!(stop.should_stop("a]".repeat(10).as_str(), None).await);
    }

    #[tokio::test]
    async fn timeout_stop() {
        let stop = TimeoutStop::new(std::time::Duration::from_millis(10));
        assert!(!stop.should_stop("", None).await);
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        assert!(stop.should_stop("", None).await);
    }
}
