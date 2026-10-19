//! Raw HTTP bytes reset the read deadline without becoming model content.
#![cfg(feature = "openai")]

use adk_core::{AdkError, Content, ErrorCategory, Llm, LlmRequest, LlmResponse, Part};
use adk_model::HttpTimeouts;
use adk_model::openai::{
    OpenAICompatible, OpenAICompatibleConfig, OpenAIResponsesClient, OpenAIResponsesConfig,
};
use adk_model::retry::RetryConfig;
use futures::{StreamExt, TryStreamExt};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(parts: Vec<Vec<u8>>, interval: Duration, stalled: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let read = socket.read(&mut bytes).await.unwrap();
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&bytes[..read]);
                if let Some(end) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .map(str::trim)
                        .unwrap_or("0")
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            count.fetch_add(1, Ordering::SeqCst);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
            for part in parts {
                if socket.write_all(format!("{:x}\r\n", part.len()).as_bytes()).await.is_err() {
                    return;
                }
                if socket.write_all(&part).await.is_err() {
                    return;
                }
                if socket.write_all(b"\r\n").await.is_err() {
                    return;
                }
                tokio::time::sleep(interval).await;
            }
            if stalled {
                std::future::pending::<()>().await;
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        Self { url, requests, task }
    }
}

fn clients(base: &str, idle: Duration) -> Vec<(&'static str, Box<dyn Llm>)> {
    let timeouts = HttpTimeouts::new(Duration::from_secs(1), idle);
    let retry = RetryConfig::disabled();
    vec![
        (
            "responses",
            Box::new(
                OpenAIResponsesClient::new(
                    OpenAIResponsesConfig::new("key", "fixture").with_base_url(base),
                )
                .unwrap()
                .with_timeouts(timeouts)
                .unwrap()
                .with_retry_config(retry.clone()),
            ),
        ),
        (
            "chat",
            Box::new(
                OpenAICompatible::new(
                    OpenAICompatibleConfig::new("key", "fixture").with_base_url(base),
                )
                .unwrap()
                .with_timeouts(timeouts)
                .unwrap()
                .with_retry_config(retry.clone()),
            ),
        ),
        #[cfg(feature = "deepseek")]
        (
            "deepseek",
            Box::new(
                adk_model::deepseek::DeepSeekClient::new(
                    adk_model::deepseek::DeepSeekConfig::new("key", "fixture").with_base_url(base),
                )
                .unwrap()
                .with_timeouts(timeouts)
                .unwrap()
                .with_retry_config(retry.clone()),
            ),
        ),
        #[cfg(feature = "azure-ai")]
        (
            "azure",
            Box::new(
                adk_model::azure_ai::AzureAIClient::new(adk_model::azure_ai::AzureAIConfig::new(
                    base, "key", "fixture",
                ))
                .unwrap()
                .with_timeouts(timeouts)
                .unwrap()
                .with_retry_config(retry.clone()),
            ),
        ),
        #[cfg(feature = "anthropic")]
        (
            "anthropic",
            Box::new(
                adk_model::anthropic::AnthropicClient::new(
                    adk_model::anthropic::AnthropicConfig::new("key", "claude-sonnet-4-6")
                        .with_base_url(base),
                )
                .unwrap()
                .with_timeouts(timeouts)
                .unwrap()
                .with_retry_config(retry.clone()),
            ),
        ),
        #[cfg(feature = "gemini")]
        (
            "gemini",
            Box::new(
                adk_model::GeminiModel::new_with_base_url_and_timeouts(
                    "key",
                    "gemini-2.5-flash",
                    format!("{base}/"),
                    timeouts,
                )
                .unwrap()
                .with_retry_config(retry),
            ),
        ),
    ]
}

fn completion(provider: &str) -> String {
    match provider {
        "responses" => format!("data: {}\n\n", json!({"type":"response.completed","response":{
            "id":"r","status":"completed","output":[{"type":"message","id":"m","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"Hello 🌍","annotations":[]}]}]}})),
        "anthropic" => concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-sonnet-4-6\",\"stop_reason\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hello 🌍\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n").into(),
        "gemini" => format!("data: {}\n\n",json!({"candidates":[{"content":{"role":"model","parts":[{"text":"Hello 🌍"}]},"finishReason":"STOP"}]})),
        _ => format!("data: {}\n\ndata: [DONE]\n\n",json!({"id":"r","object":"chat.completion.chunk","created":0,"model":"fixture",
            "choices":[{"index":0,"delta":{"content":"Hello 🌍"},"finish_reason":"stop"}]})),
    }
}

fn terminal_clients(base: &str) -> Vec<(&'static str, Box<dyn Llm>)> {
    let models: Vec<_> = clients(base, Duration::from_millis(400))
        .into_iter()
        .filter(|(name, _)| matches!(*name, "chat" | "azure" | "gemini" | "deepseek"))
        .collect();
    #[cfg(feature = "opencode")]
    let models = {
        use adk_model::opencode::{OpenCodeApi, OpenCodeClient, OpenCodeConfig, OpenCodeService};
        let mut models = models;
        for (name, api) in [
            ("opencode-chat", OpenCodeApi::ChatCompletions),
            ("opencode-gemini", OpenCodeApi::GenerateContent),
        ] {
            models.push((
                name,
                Box::new(
                    OpenCodeClient::new(
                        OpenCodeConfig::new(OpenCodeService::Zen, "key", "fixture")
                            .with_api(api)
                            .with_base_url(format!("{base}/v1"))
                            .with_user_agent("adk-test")
                            .with_session_id("session")
                            .with_timeouts(HttpTimeouts::new(
                                Duration::from_secs(1),
                                Duration::from_millis(400),
                            ))
                            .with_retry_config(RetryConfig::disabled()),
                    )
                    .unwrap(),
                ),
            ));
        }
        models
    };
    models
}

fn finish(provider: &str, tool: bool, usage: bool) -> Vec<u8> {
    let mut chunk = if provider.ends_with("gemini") {
        let part = if tool {
            json!({"functionCall":{"name":"lookup","args":{"value":"kept"}}})
        } else {
            json!({"text":"kept"})
        };
        json!({"candidates":[{"content":{"role":"model","parts":[part]},"finishReason":"STOP"}]})
    } else {
        let delta = if tool {
            json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"value\":\"kept\"}"}}]})
        } else {
            json!({"content":"kept"})
        };
        json!({"id":"r","object":"chat.completion.chunk","created":0,"model":"fixture",
            "choices":[{"index":0,"delta":delta,"finish_reason":if tool {"tool_calls"} else {"stop"}}]})
    };
    if usage {
        if provider.ends_with("gemini") {
            chunk["usageMetadata"] =
                json!({"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2});
        } else {
            chunk["usage"] = json!({"prompt_tokens":1,"completion_tokens":1,"total_tokens":2});
        }
    }
    format!("data: {chunk}\n\n").into_bytes()
}

async fn until_terminal(model: &dyn Llm) -> Result<Vec<LlmResponse>, AdkError> {
    let mut request = LlmRequest::new("fixture", vec![Content::new("user").with_text("Hello")]);
    request.tools.insert("lookup".into(),json!({"name":"lookup","description":"Read fixture","parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}));
    let mut stream = model.generate_content(request, true).await?;
    let mut responses = Vec::new();
    while let Some(response) = stream.next().await {
        let response = response?;
        let complete = response.turn_complete;
        responses.push(response);
        if complete {
            break;
        }
    }
    Ok(responses)
}

#[tokio::test]
async fn retains_usage_before_protocol_completion() {
    let names: Vec<_> =
        terminal_clients("http://127.0.0.1:1").into_iter().map(|(name, _)| name).collect();
    for (name, tool, initial_usage) in names.into_iter().flat_map(|name| {
        [(name, false, false), (name, false, true), (name, true, false), (name, true, true)]
    }) {
        let gemini = name.ends_with("gemini");
        let usage = if gemini {
            json!({"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15}})
        } else {
            json!({"id":"r","object":"chat.completion.chunk","created":0,"model":"fixture", "choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})
        };
        let mut parts =
            vec![finish(name, tool, initial_usage), format!("data: {usage}\n\n").into_bytes()];
        if !gemini {
            parts.push(b"data: [DONE]\n\n".to_vec());
        }
        // Chat's DONE ends the protocol even while the socket remains open.
        // Gemini has no DONE marker and must consume the real HTTP ending.
        let server = Server::start(parts, Duration::from_millis(20), !gemini).await;
        let (_, model) =
            terminal_clients(&server.url).into_iter().find(|(key, _)| *key == name).unwrap();
        let responses =
            tokio::time::timeout(Duration::from_secs(2), until_terminal(model.as_ref()))
                .await
                .unwrap()
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let finals: Vec<_> = responses.iter().filter(|response| !response.partial).collect();
        assert_eq!(finals.len(), 1, "{name}: {responses:?}");
        let usage = finals[0].usage_metadata.as_ref().expect("final usage is required");
        assert_eq!(
            (usage.prompt_token_count, usage.candidates_token_count, usage.total_token_count),
            (10, 5, 15),
            "{name}"
        );
        let parts: Vec<_> = responses
            .iter()
            .filter_map(|response| response.content.as_ref())
            .flat_map(|content| &content.parts)
            .collect();
        let calls: Vec<_> = parts
            .iter()
            .filter_map(|part| match part {
                Part::FunctionCall { name, args, .. } => Some((name, args)),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), usize::from(tool), "{name}");
        if tool {
            assert_eq!(calls[0].0, "lookup");
            assert_eq!(calls[0].1, &json!({"value":"kept"}));
        }
        let text: String = parts.iter().filter_map(|part| part.text()).collect();
        assert_eq!(text, if tool { "" } else { "kept" }, "{name}");
        assert_eq!(server.requests.load(Ordering::SeqCst), 1, "{name}");
    }
}

#[tokio::test]
async fn rejects_errors_after_the_finish_reason() {
    let names: Vec<_> =
        terminal_clients("http://127.0.0.1:1").into_iter().map(|(name, _)| name).collect();
    for name in names {
        let server = Server::start(vec![finish(name,false,true),
            b"data: {\"error\":{\"code\":503,\"status\":\"UNAVAILABLE\",\"message\":\"late upstream failure\"}}\n\n".to_vec()],Duration::from_millis(20),false).await;
        let (_, model) =
            terminal_clients(&server.url).into_iter().find(|(key, _)| *key == name).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), until_terminal(model.as_ref()))
                .await
                .unwrap()
                .is_err(),
            "{name}"
        );
        assert_eq!(server.requests.load(Ordering::SeqCst), 1, "{name}");
    }
}

#[tokio::test]
async fn bytes_reset_idle_before_decoding() {
    let names: Vec<_> = clients("http://127.0.0.1:1", Duration::from_millis(400))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for name in names {
        let body = completion(name);
        let split = body.find("🌍").unwrap() + 1;
        let mut parts = vec![b": heartbeat\n\n".to_vec(); 5];
        parts.push(body.as_bytes()[..split].to_vec());
        parts.push(body.as_bytes()[split..].to_vec());
        let server = Server::start(parts, Duration::from_millis(100), false).await;
        let (_, client) = clients(&server.url, Duration::from_millis(400))
            .into_iter()
            .find(|(key, _)| *key == name)
            .unwrap();
        let request = LlmRequest::new("fixture", vec![Content::new("user").with_text("Hello")]);
        let responses: Vec<_> = tokio::time::timeout(Duration::from_secs(5), async {
            client.generate_content(request, true).await.unwrap().try_collect().await
        })
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let mut text = String::new();
        for response in &responses {
            let Some(content) = &response.content else { continue };
            let next: String = content.parts.iter().filter_map(Part::text).collect();
            if response
                .provider_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("content_complete"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                text = next;
            } else {
                text.push_str(&next);
            }
        }
        assert_eq!(text, "Hello 🌍", "{name}");
        assert_eq!(server.requests.load(Ordering::SeqCst), 1, "{name}");
    }
}

#[tokio::test]
async fn stalled_body_is_a_typed_timeout() {
    let names: Vec<_> = clients("http://127.0.0.1:1", Duration::from_millis(80))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (name, streaming) in names.into_iter().flat_map(|name| [(name, true), (name, false)]) {
        let server = Server::start(vec![b": half".to_vec()], Duration::ZERO, true).await;
        let (_, client) = clients(&server.url, Duration::from_millis(80))
            .into_iter()
            .find(|(key, _)| *key == name)
            .unwrap();
        let request = LlmRequest::new("fixture", vec![Content::new("user").with_text("Hello")]);
        let result: Result<Vec<_>, _> = tokio::time::timeout(Duration::from_secs(3), async {
            client.generate_content(request, streaming).await?.try_collect().await
        })
        .await
        .unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.category, ErrorCategory::Timeout, "{name}: {error:?}");
        assert_eq!(server.requests.load(Ordering::SeqCst), 1, "{name}");
    }
}

#[tokio::test]
async fn status_and_headers_survive_sdk_boundaries() {
    use adk_model::ProviderErrorDetails;
    use std::error::Error;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let date = (chrono::Utc::now() + chrono::Duration::seconds(120))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    for status in [524, 529] {
        for header in [date.as_str(), "not a date", "18446744073709551616"] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(status)
                .insert_header("Retry-After",header)
                .set_body_json(json!({"error":{"code":"upstream_code","type":"overloaded_error","message":"原样业务错误"}})))
                .mount(&server).await;
            let clients = clients(&server.uri(), Duration::from_secs(1));
            let expected = clients.len();
            for (name, client) in clients {
                let request =
                    LlmRequest::new("fixture", vec![Content::new("user").with_text("Hello")]);
                let result: Result<Vec<_>, _> = match client.generate_content(request, true).await {
                    Ok(stream) => stream.try_collect().await,
                    Err(error) => Err(error),
                };
                let error = result.unwrap_err();
                assert_eq!(error.category, ErrorCategory::Unavailable, "{name}: {error:?}");
                assert_eq!(error.details.upstream_status_code, Some(status), "{name}");
                let details =
                    error.source().unwrap().downcast_ref::<ProviderErrorDetails>().unwrap();
                assert_eq!(details.message.as_deref(), Some("原样业务错误"), "{name}");
                match header {
                    "not a date" | "18446744073709551616" => {
                        assert_eq!(error.retry.retry_after(), None, "{name}")
                    }
                    _ => assert!(
                        error
                            .retry
                            .retry_after()
                            .is_some_and(|delay| delay > Duration::from_secs(100)),
                        "{name}"
                    ),
                }
            }
            assert_eq!(server.received_requests().await.unwrap().len(), expected);
        }
    }
}
