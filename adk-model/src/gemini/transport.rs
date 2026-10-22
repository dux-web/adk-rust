//! Generation-only raw HTTP execution; preparation and file operations remain outside retries.
use crate::request_retry::{RequestRetryConfig, http};
use adk_core::{AdkError, ErrorComponent};
use adk_gemini::{ClientError, backend::GenerationExecutor};
use std::sync::Arc;

pub(super) fn executor(policy: RequestRetryConfig) -> GenerationExecutor {
    Arc::new(move |client, request| {
        Box::pin(async move {
            send(&client, request, policy)
                .await
                .map_err(|source| ClientError::GenerationTransport { source: Box::new(source) })
        })
    })
}

async fn send(
    client: &reqwest::Client,
    request: reqwest::Request,
    policy: RequestRetryConfig,
) -> Result<reqwest::Response, AdkError> {
    let response = http::send(client, request, policy).await.map_err(|source| {
        http::network(
            AdkError::new(
                ErrorComponent::Model,
                crate::http_timeouts::transport_category(&source),
                "model.gemini.request",
                adk_core::upstream_excerpt(&source.to_string()),
            )
            .with_provider("gemini"),
            &source,
            "request",
        )
    })?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let deadline = http::deadline(&response);
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let description = response.text().await.ok();
    let original = ClientError::BadResponse { code: status, description, retry_after };
    let mut error = http::annotate(super::client::gemini_error_to_adk(&original), "request");
    if let Some(deadline) = deadline {
        error = error.with_retry_deadline(deadline);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_deadline_and_typed_cause() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut original = AdkError::new(
            ErrorComponent::Model,
            adk_core::ErrorCategory::Unavailable,
            "model.gemini.request",
            "Original upstream failure",
        )
        .with_upstream_status(503)
        .with_provider("gemini")
        .with_retry_deadline(deadline);
        original.details.metadata.insert("provider_error_code".into(), "UNAVAILABLE".into());
        let wrapped = ClientError::GenerationTransport { source: Box::new(original) };
        let recovered = super::super::client::gemini_error_to_adk(&wrapped);
        assert_eq!(recovered.code, "model.gemini.request");
        assert_eq!(recovered.message, "Original upstream failure");
        assert_eq!(recovered.details.upstream_status_code, Some(503));
        assert_eq!(recovered.details.metadata["provider_error_code"], "UNAVAILABLE");
        assert_eq!(recovered.retry_deadline(), Some(deadline));
    }
}
