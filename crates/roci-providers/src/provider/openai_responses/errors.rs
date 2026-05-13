//! HTTP error handling for the OpenAI Responses API.

use roci_core::error::RociError;

use super::super::openai_errors::status_to_openai_error;

pub(super) async fn success_or_openai_error(
    response: reqwest::Response,
) -> Result<reqwest::Response, RociError> {
    let status = response.status().as_u16();
    if status == 200 {
        return Ok(response);
    }

    let body_text = response.text().await.unwrap_or_default();
    Err(status_to_openai_error(status, &body_text))
}
