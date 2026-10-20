//! Explicit, request-scoped application handling of untrusted provider failures.

use std::{future::Future, sync::Arc};

use adk_core::{AdkError, LlmResponseStream, Result};
use futures::StreamExt;

/// Untrusted failure fields, never attached to ADK's error source or serialized.
/// Applications must redact captured credentials before formatting or truncating them.
#[derive(Default)]
pub struct ProviderErrorDetails {
    /// Provider-defined failure code.
    pub code: Option<String>,
    /// Provider-defined failure type.
    pub kind: Option<String>,
    /// Complete message, omitted when it exceeds 64 KiB.
    pub message: Option<String>,
    /// Invalid parameter reported by the provider.
    pub param: Option<String>,
    /// Actual upstream HTTP status, when available.
    pub status: Option<u16>,
    /// Upstream request identifier, when available.
    pub request_id: Option<String>,
}

impl std::fmt::Debug for ProviderErrorDetails {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ProviderErrorDetails").finish_non_exhaustive()
    }
}

/// Pure application conversion of a model failure, disabled unless explicitly scoped.
/// The callback must not block, perform I/O, or start retries. Returning the original
/// error preserves its classification. Private fields may echo request credentials.
pub type ErrorAdapter = Arc<dyn Fn(AdkError, ProviderErrorDetails) -> AdkError + Send + Sync>;

tokio::task_local! {
    static ADAPTER: ErrorAdapter;
}

/// Apply a private error adapter to one generation, including subsequent stream polls.
///
/// The scope is restored after every poll and when a future or stream is cancelled.
/// It does not propagate to unrelated spawned tasks. No provider details are retained
/// by ADK beyond the synchronous callback. Ordinary generation remains unchanged.
///
/// # Example
///
/// ```no_run
/// use adk_core::{Llm, LlmRequest};
/// use adk_model::{ErrorAdapter, inspect_errors};
/// use std::sync::Arc;
///
/// async fn generate(model: &dyn Llm, request: LlmRequest) -> adk_core::Result<()> {
///     let adapter: ErrorAdapter = Arc::new(|error, _private_fields| error);
///     let _stream = inspect_errors(adapter, model.generate_content(request, true)).await?;
///     Ok(())
/// }
/// ```
pub async fn inspect_errors<F>(adapter: ErrorAdapter, response: F) -> Result<LlmResponseStream>
where
    F: Future<Output = Result<LlmResponseStream>>,
{
    let mut stream = ADAPTER.scope(adapter.clone(), response).await?;
    Ok(Box::pin(async_stream::stream! {
        while let Some(response) = ADAPTER.scope(adapter.clone(), stream.next()).await {
            yield response;
        }
    }))
}

#[cfg(any(
    feature = "openai",
    feature = "anthropic",
    feature = "gemini",
    feature = "deepseek",
    feature = "azure-ai",
    test
))]
pub(crate) fn enabled() -> bool {
    ADAPTER.try_with(|_| ()).is_ok()
}

#[cfg(any(
    feature = "openai",
    feature = "anthropic",
    feature = "gemini",
    feature = "deepseek",
    feature = "azure-ai",
    test
))]
pub(crate) fn adapt(error: AdkError, details: impl FnOnce() -> ProviderErrorDetails) -> AdkError {
    match ADAPTER.try_with(Arc::clone) {
        Ok(adapter) => adapter(error, details()),
        Err(_) => error,
    }
}

#[cfg(any(
    feature = "openai",
    feature = "anthropic",
    feature = "gemini",
    feature = "deepseek",
    feature = "azure-ai",
    test
))]
pub(crate) fn bounded(value: &str) -> Option<String> {
    (value.len() <= 64 * 1024).then(|| value.to_owned())
}

#[cfg(any(
    feature = "openai",
    feature = "gemini",
    feature = "deepseek",
    feature = "azure-ai",
    test
))]
impl ProviderErrorDetails {
    pub(crate) fn from_value(value: &serde_json::Value) -> Self {
        let failure = value
            .pointer("/response/error")
            .or_else(|| value.get("error"))
            .or_else(|| value.get(0).and_then(|item| item.get("error")))
            .unwrap_or(value);
        let field = |name| failure.get(name).and_then(serde_json::Value::as_str).and_then(bounded);
        Self {
            code: field("code").or_else(|| field("status")),
            kind: field("type"),
            message: field("message").or_else(|| field("detail")),
            param: field("param"),
            ..Default::default()
        }
    }

    pub(crate) fn from_body(body: &str) -> Self {
        if body.len() > 64 * 1024 {
            return Self::default();
        }
        let mut details =
            serde_json::from_str(body).ok().as_ref().map(Self::from_value).unwrap_or_default();
        if details.message.is_none() {
            details.message = bounded(body);
        }
        details
    }
}

#[cfg(test)]
mod tests;
