//! Request customization and response observation at the HTTP boundary.

/// Applies provider-specific JSON fields and headers before a generation request.
///
/// The adapter must preserve the requested response mode and must not perform I/O.
/// It runs for each HTTP attempt under the model's configured retry policy.
///
/// # Example
///
/// ```
/// use adk_model::openai::{OpenAICompatible, OpenAICompatibleConfig, RequestAdapter};
/// use std::sync::Arc;
///
/// let adapter: RequestAdapter = Arc::new(|body, headers| {
///     body["metadata"] = serde_json::json!({"application": "example"});
///     headers.insert("x-request-source", "example".parse().expect("valid header"));
///     Ok(())
/// });
/// let client = OpenAICompatible::new(OpenAICompatibleConfig::new("sk-key", "gpt-5-mini"))?
///     .with_request_adapter(adapter);
/// # Ok::<(), adk_core::AdkError>(())
/// ```
pub type RequestAdapter = std::sync::Arc<
    dyn Fn(
            &mut serde_json::Value,
            &mut reqwest::header::HeaderMap,
        ) -> Result<(), adk_core::AdkError>
        + Send
        + Sync,
>;

/// Observes response headers before status handling or body consumption.
///
/// The callback runs once for each Responses generation HTTP attempt, including
/// unsuccessful statuses. It should not perform I/O or retain sensitive headers.
pub type ResponseObserver = std::sync::Arc<dyn Fn(&http::HeaderMap) + Send + Sync>;
