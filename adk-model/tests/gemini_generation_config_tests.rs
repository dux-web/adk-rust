//! Gemini provider options reach the real generateContent wire without changing core sampling.
#![cfg(feature = "gemini")]

use adk_core::{Content, ErrorCategory, GenerateContentConfig, Llm, LlmRequest, Part};
use adk_model::GeminiModel;
use futures::StreamExt;
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "gemini-2.5-flash";

fn model(server: &MockServer) -> GeminiModel {
    GeminiModel::new_with_base_url("test-key", MODEL, format!("{}/v1beta/", server.uri()))
        .unwrap()
        .with_retry_config(adk_model::retry::RetryConfig::disabled())
}

fn request(extensions: Value) -> LlmRequest {
    LlmRequest::new(MODEL, vec![Content::new("user").with_text("Draw a small tree")]).with_config(
        GenerateContentConfig {
            temperature: Some(0.25),
            max_output_tokens: Some(64),
            extensions: extensions.as_object().unwrap().clone(),
            ..Default::default()
        },
    )
}

fn reply(streaming: bool) -> ResponseTemplate {
    let body = json!({
        "candidates":[{"content":{"role":"model","parts":[
            {"text":"A small tree"},
            {"inlineData":{"mimeType":"image/png","data":"AQID"}}
        ]},"finishReason":"STOP"}],
        "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}
    });
    if streaming {
        ResponseTemplate::new(200)
            .set_body_raw(format!("data: {body}\r\n\r\n"), "text/event-stream")
    } else {
        ResponseTemplate::new(200).set_body_json(body)
    }
}

#[tokio::test]
async fn sends_requested_modalities_on_both_transports() {
    for streaming in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("POST")).respond_with(reply(streaming)).expect(1).mount(&server).await;
        let model = model(&server);
        let request = request(json!({"gemini":{"response_modalities":["TEXT","IMAGE"]}}));
        let mut responses = model.generate_content(request, streaming).await.unwrap();
        let mut image = None;
        let mut usage = None;
        let mut complete = false;
        while let Some(response) = responses.next().await {
            let response = response.unwrap();
            complete |= response.turn_complete && !response.partial;
            if let Some(metadata) = response.usage_metadata {
                usage = Some((
                    metadata.prompt_token_count,
                    metadata.candidates_token_count,
                    metadata.total_token_count,
                ));
            }
            for part in response.content.into_iter().flat_map(|content| content.parts) {
                if let Part::InlineData { mime_type, data, .. } = part {
                    image = Some((mime_type, data));
                }
            }
        }
        assert!(complete);
        assert_eq!(image, Some(("image/png".into(), vec![1, 2, 3])));
        assert_eq!(usage, Some((3, 2, 5)));
        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0].url.path(),
            format!(
                "/v1beta/models/{MODEL}:{}",
                if streaming { "streamGenerateContent" } else { "generateContent" }
            )
        );
        let body: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            body["generationConfig"],
            json!({"temperature":0.25,"maxOutputTokens":64,"responseModalities":["TEXT","IMAGE"]})
        );
    }
}

#[tokio::test]
async fn leaves_other_namespaces_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(reply(false)).expect(2).mount(&server).await;
    let model = model(&server);
    for extensions in [json!({}), json!({"openai":{"response_modalities":"not a Gemini option"}})] {
        let mut responses = model.generate_content(request(extensions), false).await.unwrap();
        while let Some(response) = responses.next().await {
            response.unwrap();
        }
    }
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2);
    for request in received {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["generationConfig"], json!({"temperature":0.25,"maxOutputTokens":64}));
    }
}

#[tokio::test]
async fn rejects_malformed_modalities_before_http() {
    let server = MockServer::start().await;
    let model = model(&server);
    for value in [Value::Null, json!("IMAGE"), json!(7), json!(["TEXT", 1])] {
        let error = model
            .generate_content(request(json!({"gemini":{"response_modalities":value}})), false)
            .await
            .err()
            .expect("invalid modalities must fail before HTTP");
        assert_eq!(error.category, ErrorCategory::InvalidInput);
        assert_eq!(error.code, "model.gemini.invalid_config");
        assert!(!error.is_retryable());
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
