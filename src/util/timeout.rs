//! Timeout helper.

use std::future::Future;
use std::time::Duration;

use crate::error::RociError;

/// Wrap a future with a timeout.
pub async fn with_timeout<T>(
    duration: Duration,
    future: impl Future<Output = Result<T, RociError>>,
) -> Result<T, RociError> {
    match tokio::time::timeout(duration, future).await {
        Ok(result) => result,
        Err(_) => Err(RociError::Timeout(duration.as_millis() as u64)),
    }
}
