//! Raw request retries stay below generation preparation and stream consumption.
#![cfg(all(feature = "openai", feature = "deepseek", feature = "azure-ai"))]

use adk_core::{
    AdkError, Content, Llm, LlmRequest,
    model_observation::{self, Kind, Outcome},
};
use adk_model::{
    OpenAICompatible, OpenAICompatibleConfig,
    azure_ai::{AzureAIClient, AzureAIConfig},
    deepseek::{DeepSeekClient, DeepSeekConfig},
    openai::{AzureConfig, AzureOpenAIClient},
    request_retry::RequestRetryConfig,
    retry::RetryConfig,
};
use futures::StreamExt;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

fn clients(endpoint: &str, retries: u32) -> Vec<Box<dyn Llm>> {
    let policy = RequestRetryConfig { max_retries: retries, initial_delay: Duration::ZERO };
    let clients: Vec<Box<dyn Llm>> = vec![
        Box::new(
            OpenAICompatible::new(
                OpenAICompatibleConfig::new("key", "model").with_base_url(endpoint),
            )
            .unwrap()
            .with_retry_config(RetryConfig::disabled())
            .with_request_retry(policy),
        ),
        Box::new(
            AzureOpenAIClient::new(AzureConfig::new("key", endpoint, "2024-10-21", "model"))
                .unwrap()
                .with_retry_config(RetryConfig::disabled())
                .with_request_retry(policy),
        ),
        Box::new(
            DeepSeekClient::new(DeepSeekConfig::new("key", "model").with_base_url(endpoint))
                .unwrap()
                .with_retry_config(RetryConfig::disabled())
                .with_request_retry(policy),
        ),
        Box::new(
            AzureAIClient::new(AzureAIConfig::new(endpoint, "key", "model"))
                .unwrap()
                .with_retry_config(RetryConfig::disabled())
                .with_request_retry(policy),
        ),
    ];
    #[cfg(feature = "gemini")]
    let clients = {
        let mut clients = clients;
        clients.push(Box::new(
            adk_model::GeminiModel::new_with_base_url_and_timeouts(
                "key",
                "model",
                format!("{endpoint}/"),
                adk_model::HttpTimeouts::default(),
            )
            .unwrap()
            .with_retry_config(RetryConfig::disabled())
            .with_request_retry(policy)
            .unwrap(),
        ));
        clients
    };
    clients
}

async fn failure(client: &dyn Llm, stream: bool) -> AdkError {
    let request = LlmRequest::new("model", vec![Content::new("user").with_text("Hello")]);
    match client.generate_content(request, stream).await {
        Err(error) => error,
        Ok(mut response) => {
            while let Some(item) = response.next().await {
                if let Err(error) = item {
                    return error;
                }
            }
            panic!("expected upstream failure")
        }
    }
}

#[tokio::test]
async fn bounds_sends_before_headers() {
    for (status, sends) in [(500, 5), (503, 5), (429, 1), (400, 1)] {
        for stream in [false, true] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).insert_header("retry-after", "0")
                    .set_body_json(serde_json::json!({"error":{"code":"upstream_failure","message":"Unavailable"}})))
                .mount(&server).await;
            for client in clients(&server.uri(), 4) {
                let before = server.received_requests().await.unwrap().len();
                let error = failure(client.as_ref(), stream).await;
                assert_eq!(server.received_requests().await.unwrap().len() - before, sends);
                assert_eq!(error.details.upstream_status_code, Some(status));
                assert_eq!(error.details.metadata["provider_error_code"], "upstream_failure");
                assert_eq!(error.details.metadata["transport_phase"], "request");
                assert!(error.retry_deadline().is_some());
            }
        }
    }
}

#[tokio::test]
async fn preserves_prepared_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).insert_header("retry-after", "0"))
        .expect(5)
        .mount(&server)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let client = OpenAICompatible::new(
        OpenAICompatibleConfig::new("key", "model").with_base_url(server.uri()),
    )
    .unwrap()
    .with_retry_config(RetryConfig::disabled())
    .with_request_retry(RequestRetryConfig { max_retries: 4, initial_delay: Duration::ZERO })
    .with_request_adapter(Arc::new(move |body, headers| {
        let revision = observed.fetch_add(1, Ordering::SeqCst);
        body["prepared_revision"] = revision.into();
        headers.insert("x-prepared", "once".parse().unwrap());
        Ok(())
    }));
    let events = Arc::new(Mutex::new(Vec::new()));
    let output = events.clone();
    model_observation::scope(Arc::new(move |event| output.lock().unwrap().push(event)), async {
        Err::<(), _>(failure(&client, true).await)
    })
    .await
    .unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 5);
    for request in &requests {
        assert_eq!(request.body, requests[0].body);
        assert_eq!(request.headers["x-prepared"], "once");
    }
    let events = events.lock().unwrap();
    for attempt in 1..=5 {
        let attempt_events: Vec<_> =
            events.iter().filter(|event| event.attempt == attempt).collect();
        assert_eq!(attempt_events.len(), 4);
        assert!(matches!(attempt_events[0].kind, Kind::Started { .. }));
        assert!(
            matches!(&attempt_events[1].kind, Kind::Prepared { body } if body == &serde_json::from_slice::<serde_json::Value>(&requests[0].body).unwrap())
        );
        assert!(matches!(attempt_events[2].kind, Kind::Headers { status: 500, .. }));
        assert!(matches!(attempt_events[3].kind, Kind::Finished { outcome: Outcome::Failed, .. }));
    }
}

#[tokio::test]
async fn does_not_replay_started_streams() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\n\n"),
        )
        .mount(&server)
        .await;
    for client in clients(&server.uri(), 4) {
        let before = server.received_requests().await.unwrap().len();
        failure(client.as_ref(), true).await;
        assert_eq!(server.received_requests().await.unwrap().len() - before, 1);
    }
}
