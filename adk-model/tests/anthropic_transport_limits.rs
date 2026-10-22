#![cfg(feature = "anthropic")]

use adk_core::{Content, ErrorCategory, Llm, LlmRequest};
use adk_model::{
    HttpTimeouts,
    anthropic::{AnthropicClient, AnthropicConfig},
    retry::RetryConfig,
};
use futures::StreamExt;
use std::time::Duration;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn request() -> LlmRequest {
    LlmRequest::new("claude-sonnet-4-6", vec![Content::new("user").with_text("Hello")])
}

#[tokio::test]
async fn preserves_http_status_and_retry_delay() {
    for (status, category) in [
        (429, ErrorCategory::RateLimited),
        (500, ErrorCategory::Unavailable),
        (502, ErrorCategory::Unavailable),
        (503, ErrorCategory::Unavailable),
        (504, ErrorCategory::Unavailable),
        (529, ErrorCategory::Unavailable),
        (599, ErrorCategory::Internal),
    ] {
        for stream in [false, true] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/v1/messages"))
                .respond_with(ResponseTemplate::new(status).insert_header("retry-after","17").set_body_json(serde_json::json!({"error":{"type":"api_error","message":"Fixture failure"}})))
                .expect(1).mount(&server).await;
            let model = AnthropicClient::new(
                AnthropicConfig::new("fixture-key", "claude-sonnet-4-6")
                    .with_base_url(server.uri()),
            )
            .unwrap()
            .with_retry_config(RetryConfig::disabled());
            let result = model.generate_content(request(), stream).await;
            let error = match result {
                Err(error) => error,
                Ok(mut events) => events.next().await.unwrap().unwrap_err(),
            };
            assert_eq!(error.details.upstream_status_code, Some(status));
            assert_eq!(error.category, category);
            assert_eq!(
                error.retry.retry_after(),
                error.retry.should_retry.then_some(Duration::from_secs(17))
            );
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn classifies_response_header_timeouts() {
    for stream in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
            .expect(1)
            .mount(&server)
            .await;
        let model = AnthropicClient::new(
            AnthropicConfig::new("fixture-key", "claude-sonnet-4-6").with_base_url(server.uri()),
        )
        .unwrap()
        .with_retry_config(RetryConfig::disabled())
        .with_timeouts(HttpTimeouts::new(Duration::from_secs(1), Duration::from_millis(40)))
        .unwrap();
        let error = match model.generate_content(request(), stream).await {
            Err(error) => error,
            Ok(mut events) => events.next().await.unwrap().unwrap_err(),
        };
        assert_eq!(error.category, ErrorCategory::Timeout);
        assert_eq!(error.code, "model.anthropic.timeout");
        assert!(error.retry.should_retry);
        server.verify().await;
    }
}

#[tokio::test]
async fn raw_retries_bound_each_header_wait() {
    use adk_model::request_retry::RequestRetryConfig;
    for explicit in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .expect(2)
            .mount(&server)
            .await;
        let mut model = AnthropicClient::new(
            AnthropicConfig::new("key", "claude-sonnet-4-6")
                .with_base_url(server.uri())
                .with_request_timeout(Duration::from_secs(1)),
        )
        .unwrap()
        .with_retry_config(RetryConfig::disabled())
        .with_request_retry(RequestRetryConfig { max_retries: 1, initial_delay: Duration::ZERO });
        if explicit {
            // Updating timeouts after installing the executor must also take effect.
            model = model
                .with_timeouts(HttpTimeouts::new(Duration::from_secs(1), Duration::from_millis(40)))
                .unwrap();
        }
        let error = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = model.generate_content(request(), true).await.unwrap();
            events.next().await.unwrap().unwrap_err()
        })
        .await
        .expect("both individual sends must have bounded header waits");
        assert_eq!(error.category, ErrorCategory::Timeout);
        server.verify().await;
    }
}

#[tokio::test]
async fn header_deadline_excludes_server_backoff() {
    use adk_model::request_retry::RequestRetryConfig;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    Mock::given(method("POST")).respond_with(move |_: &wiremock::Request| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            ResponseTemplate::new(503).insert_header("retry-after", "1")
        } else {
            ResponseTemplate::new(400).set_body_json(serde_json::json!({"error":{"type":"invalid_request_error","message":"fixture"}}))
        }
    }).expect(2).mount(&server).await;
    let model = AnthropicClient::new(
        AnthropicConfig::new("key", "claude-sonnet-4-6").with_base_url(server.uri()),
    )
    .unwrap()
    .with_retry_config(RetryConfig::disabled())
    .with_timeouts(HttpTimeouts::new(Duration::from_secs(1), Duration::from_millis(100)))
    .unwrap()
    .with_request_retry(RequestRetryConfig { max_retries: 1, initial_delay: Duration::ZERO });
    let started = std::time::Instant::now();
    let mut events = model.generate_content(request(), true).await.unwrap();
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.details.upstream_status_code, Some(400));
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
