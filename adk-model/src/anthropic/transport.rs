//! Raw generation transport preserving provider errors before SDK classification.
use crate::request_retry::{RequestRetryConfig, http};
use adk_core::{AdkError, ErrorCategory, ErrorComponent};

pub(super) fn executor(policy: RequestRetryConfig) -> adk_anthropic::GenerationExecutor {
    std::sync::Arc::new(move |client, request| {
        Box::pin(async move {
            send(&client, request, policy).await.map_err(|error| {
                adk_anthropic::Error::http_client(
                    "Generation transport failed",
                    Some(Box::new(error)),
                )
            })
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
                if source.is_timeout() {
                    ErrorCategory::Timeout
                } else {
                    ErrorCategory::Unavailable
                },
                "model.anthropic.transport",
                adk_core::upstream_excerpt(&source.to_string()),
            )
            .with_provider("anthropic"),
            &source,
            "request",
        )
        .with_source(source)
    })?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let deadline = http::deadline(&response);
    let body = response.text().await.map_err(|source| {
        let mut error = http::network(
            AdkError::new(
                ErrorComponent::Model,
                crate::retry::category_for_status_code(status),
                "model.anthropic.api",
                "Anthropic error response could not be read",
            )
            .with_provider("anthropic")
            .with_upstream_status(status),
            &source,
            "request",
        )
        .with_source(source);
        error.details.metadata.insert("response_body_error".into(), true.into());
        preserve_deadline(error, deadline)
    })?;
    let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let provider = value.get("error").unwrap_or(&value);
    let message = provider.get("message").and_then(serde_json::Value::as_str).unwrap_or(&body);
    let mut error = http::annotate(
        AdkError::new(
            ErrorComponent::Model,
            crate::retry::category_for_status_code(status),
            "model.anthropic.api",
            adk_core::upstream_excerpt(message),
        )
        .with_provider("anthropic")
        .with_upstream_status(status),
        "request",
    );
    for (key, target) in [("code", "provider_error_code"), ("type", "provider_error_type")] {
        if let Some(value) = provider.get(key).and_then(serde_json::Value::as_str)
            && !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            error.details.metadata.insert(target.into(), value.into());
        }
    }
    Err(preserve_deadline(error, deadline))
}

fn preserve_deadline(mut error: AdkError, deadline: Option<std::time::Instant>) -> AdkError {
    if let Some(deadline) = deadline {
        error.retry = error
            .retry
            .clone()
            .with_retry_after(deadline.saturating_duration_since(std::time::Instant::now()));
        error = error.with_retry_deadline(deadline);
    }
    error
}

pub(super) fn preserved(source: &adk_anthropic::Error) -> Option<AdkError> {
    let error = std::error::Error::source(source)?.downcast_ref::<AdkError>()?;
    let mut result =
        AdkError::new(error.component, error.category, error.code, error.message.clone())
            .with_details((*error.details).clone());
    result.retry = error.retry.clone();
    if let Some(deadline) = error.retry_deadline() {
        result = result.with_retry_deadline(deadline);
    }
    Some(result)
}
