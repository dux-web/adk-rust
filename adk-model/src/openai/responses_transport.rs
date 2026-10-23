//! Raw Responses HTTP retries and transport observations.
use adk_core::model_observation::{Attempt, Failure, Transport};
use adk_core::{AdkError, ErrorCategory, ErrorComponent};
use async_openai::{
    error::OpenAIError,
    middleware::{HttpRequestFactory, ReqwestService},
};
use std::time::Instant;
use tower::ServiceExt;

pub use crate::request_retry::RequestRetryConfig;

pub(super) async fn send(
    transport: ReqwestService,
    factory: HttpRequestFactory,
    policy: RequestRetryConfig,
    observer: Option<super::ResponseObserver>,
) -> Result<reqwest_openai::Response, OpenAIError> {
    crate::request_retry::execute(
        policy,
        || {
            let transport = transport.clone();
            let factory = factory.clone();
            let observer = observer.clone();
            async move {
                let observed = std::sync::Arc::new(std::sync::Mutex::new(Attempt::default()));
                let prepared = observed.clone();
                let factory = HttpRequestFactory::new(move || {
                    let factory = factory.clone();
                    let prepared = prepared.clone();
                    async move {
                        let request = factory.build().await?;
                        // Background polling and other SDK resources are not generation attempts.
                        if request.method() == http::Method::POST
                            && request.url().path().ends_with("/responses")
                        {
                            let attempt = Attempt::start(Transport::Http, None);
                            attempt.prepared(request.body().and_then(|body| body.as_bytes()));
                            *prepared.lock().unwrap() = attempt;
                        }
                        Ok(request)
                    }
                });
                let mut result = transport.oneshot(factory).await;
                let attempt = observed.lock().unwrap().clone();
                match &result {
                    Ok(response) => {
                        attempt.headers(response.status().as_u16(), None);
                        if !response.status().is_success() {
                            attempt.finish_failure(crate::request_retry::http::status_failure(
                                response.status().as_u16(),
                            ));
                        }
                    }
                    Err(error) => attempt.finish_failure(request_failure(error)),
                }
                if let Ok(response) = &mut result
                    && let Some(deadline) = retry_at(response.headers())
                {
                    response.extensions_mut().insert(RetryDeadline(deadline));
                }
                if let Ok(response) = &result
                    && response.url().path().ends_with("/responses")
                    && let Some(observer) = observer
                {
                    observer(response.headers());
                }
                result
            }
        },
        |result| match result {
            Ok(response) => response.status().is_server_error(),
            Err(OpenAIError::Reqwest(error)) => !error.is_builder() && !error.is_redirect(),
            Err(_) => false,
        },
        |result| result.as_ref().ok().and_then(response_deadline),
    )
    .await
}

fn request_failure(error: &OpenAIError) -> Failure {
    // The raw send precedes SDK error normalization and its retry loop can hide
    // failures from callers. Inspect the typed source here, never its display.
    let OpenAIError::Reqwest(error) = error else {
        return Failure {
            code: "model.http.request",
            component: ErrorComponent::Model,
            category: ErrorCategory::Internal,
            upstream_status: None,
        };
    };
    let (code, category) = if error.is_timeout() {
        ("model.http.timeout", ErrorCategory::Timeout)
    } else if error.is_connect() {
        ("model.http.connection", ErrorCategory::Unavailable)
    } else if error.is_builder() || error.is_redirect() {
        ("model.http.build", ErrorCategory::InvalidInput)
    } else if error.is_decode() {
        ("model.http.decode", ErrorCategory::Internal)
    } else {
        ("model.http.network", ErrorCategory::Unavailable)
    };
    Failure {
        code,
        component: ErrorComponent::Model,
        category,
        upstream_status: error.status().map(|status| status.as_u16()),
    }
}

#[derive(Clone, Copy)]
struct RetryDeadline(Instant);

pub(super) fn response_deadline(response: &reqwest_openai::Response) -> Option<Instant> {
    response
        .extensions()
        .get::<RetryDeadline>()
        .map(|value| value.0)
        .or_else(|| retry_at(response.headers()))
}

pub(super) fn retry_at(headers: &http::HeaderMap) -> Option<Instant> {
    headers
        .get(http::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(crate::retry::parse_retry_after)
        .and_then(|delay| Instant::now().checked_add(delay))
}

pub(super) fn annotate(mut error: AdkError, transport: &str, phase: &str) -> AdkError {
    error.details.metadata.insert("transport".into(), transport.into());
    error.details.metadata.insert("transport_phase".into(), phase.into());
    if transport == "websocket" && error.details.upstream_status_code.is_none() {
        let kind = match error.code {
            "model.openai_responses.ws_invalid_url"
            | "model.openai_responses.ws_invalid_api_key"
            | "model.openai_responses.ws_request"
            | "model.openai_responses.ws_serialize"
            | "model.openai_responses.ws_tls_config" => Some("build"),
            "model.openai_responses.timeout" => Some("timeout"),
            "model.openai_responses.ws_connect"
            | "model.openai_responses.ws_read"
            | "model.openai_responses.ws_send"
            | "model.openai_responses.ws_tls" => Some("network"),
            _ => None,
        };
        if let Some(kind) = kind {
            error.details.metadata.insert("transport_error_kind".into(), kind.into());
        }
    }
    error
}

pub(super) fn network(mut error: AdkError, source: &reqwest_openai::Error) -> AdkError {
    let kind = if source.is_builder() || source.is_redirect() {
        "build"
    } else if source.is_timeout() {
        "timeout"
    } else if source.is_connect() {
        "connection"
    } else {
        "network"
    };
    error.details.metadata.insert("transport_error_kind".into(), kind.into());
    error
}

pub(super) fn provider(mut error: AdkError, value: &serde_json::Value) -> AdkError {
    for (source, target) in [("code", "provider_error_code"), ("type", "provider_error_type")] {
        if let Some(value) = value.get(source).and_then(serde_json::Value::as_str)
            && !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            error.details.metadata.insert(target.into(), value.into());
        }
    }
    if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
        error.message = adk_core::upstream_excerpt(message).to_owned();
    }
    if let Some(delay) = value
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .and_then(|headers| {
            headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        })
        .and_then(|(_, value)| value.as_str())
        .and_then(crate::retry::parse_retry_after)
        && let Some(deadline) = Instant::now().checked_add(delay)
    {
        error.retry = error.retry.clone().with_retry_after(delay);
        error = error.with_retry_deadline(deadline);
    }
    error
}

pub(super) fn ignore_event(value: &serde_json::Value) -> bool {
    value.get("type").and_then(serde_json::Value::as_str) == Some("error")
        && value
            .pointer("/error/code")
            .or_else(|| value.get("code"))
            .and_then(serde_json::Value::as_str)
            != Some("flex_unavailable")
}

pub(super) fn tool_completed(value: &serde_json::Value) -> bool {
    use async_openai::types::responses::{OutputItem, ResponseStreamEvent};

    if !matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some(
            "response.mcp_call.completed"
                | "response.code_interpreter_call.completed"
                | "response.web_search_call.completed"
                | "response.file_search_call.completed"
                | "response.image_generation_call.completed"
                | "response.output_item.done"
        )
    ) {
        return false;
    }

    match serde_json::from_value::<ResponseStreamEvent>(value.clone()) {
        // Execution can finish before the output item and response terminal events.
        Ok(
            ResponseStreamEvent::ResponseMCPCallCompleted(_)
            | ResponseStreamEvent::ResponseCodeInterpreterCallCompleted(_)
            | ResponseStreamEvent::ResponseWebSearchCallCompleted(_)
            | ResponseStreamEvent::ResponseFileSearchCallCompleted(_)
            | ResponseStreamEvent::ResponseImageGenerationCallCompleted(_),
        ) => true,
        Ok(ResponseStreamEvent::ResponseOutputItemDone(event)) => {
            // Client-owned calls only finish generating instructions. Shell calls
            // also describe commands; provider-emitted shell outputs prove execution.
            let completed = value.pointer("/item/status").and_then(serde_json::Value::as_str)
                == Some("completed");
            match event.item {
                OutputItem::McpCall(call) => {
                    completed
                        || (call.status.is_none() && call.output.is_some() && call.error.is_none())
                }
                OutputItem::WebSearchCall(_)
                | OutputItem::FileSearchCall(_)
                | OutputItem::CodeInterpreterCall(_)
                | OutputItem::ImageGenerationCall(_)
                | OutputItem::ShellCallOutput(_) => completed,
                _ => false,
            }
        }
        _ => false,
    }
}

pub(super) fn committed(mut error: AdkError, tools: bool) -> AdkError {
    if tools {
        error.details.metadata.insert("provider_tool_completed".into(), true.into());
    }
    error
}

pub(super) fn event(
    mut value: serde_json::Value,
    recovery: bool,
) -> Result<super::responses_client::wire::Event, AdkError> {
    if recovery && ignore_event(&value) {
        return Ok(super::responses_client::wire::Event::Activity);
    }
    if recovery
        && value.get("type").and_then(serde_json::Value::as_str) == Some("response.incomplete")
    {
        let reason = value
            .pointer("/response/incomplete_details/reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        if reason != "interrupted" {
            let mut error = AdkError::new(
                adk_core::ErrorComponent::Model,
                adk_core::ErrorCategory::Unavailable,
                "model.openai_responses.incomplete",
                "OpenAI Responses generation is incomplete",
            )
            .with_provider("openai-responses");
            error
                .details
                .metadata
                .insert("provider_event_type".into(), "response.incomplete".into());
            if reason.len() <= 128
                && reason.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
            {
                error.details.metadata.insert("incomplete_reason".into(), reason.into());
            }
            return Err(error);
        }
        value["type"] = "response.completed".into();
        value["response"]["status"] = "completed".into();
    }
    super::responses_client::wire::event(value)
}

pub(super) fn finish(
    state: &mut super::responses_stream::StreamState,
    response: &super::responses_client::wire::Response,
    recovery: bool,
) -> Result<adk_core::LlmResponse, AdkError> {
    let mut result = state.finish(response).map_err(|mut error| {
        error.details.metadata.insert("response_completed".into(), true.into());
        error
    })?;
    if recovery
        && response
            .incomplete_details
            .as_ref()
            .is_some_and(|details| details.reason == "interrupted")
    {
        result.interrupted = true;
        result.provider_metadata.get_or_insert_with(|| serde_json::json!({}))["continue_turn"] =
            true.into();
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::time::Duration;
    #[test]
    fn preserves_server_deadlines() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::RETRY_AFTER, "120".parse().unwrap());
        let start = Instant::now();
        let deadline = retry_at(&headers).unwrap();
        assert!(deadline >= start + Duration::from_secs(120));
        assert!(deadline < start + Duration::from_secs(121));
    }

    #[test]
    fn marks_hosted_execution_events() {
        for tool in ["mcp", "code_interpreter", "web_search", "file_search", "image_generation"] {
            let mut event = json!({"type":format!("response.{tool}_call.completed"),
                "sequence_number":1,"output_index":0,"item_id":"tool-call"});
            assert!(tool_completed(&event), "{tool}");
            event["type"] = format!("response.{tool}_call.in_progress").into();
            assert!(!tool_completed(&event), "{tool}");
            event["type"] = format!("response.{tool}_call.completed").into();
            event.as_object_mut().unwrap().remove("item_id");
            assert!(!tool_completed(&event), "{tool}");
        }
    }

    #[test]
    fn requires_successful_hosted_outputs() {
        for mut item in [
            json!({"type":"web_search_call","id":"search","status":"completed"}),
            json!({"type":"file_search_call","id":"search","queries":[],"status":"completed"}),
            json!({"type":"code_interpreter_call","id":"code","container_id":"container","status":"completed"}),
            json!({"type":"mcp_call","id":"mcp","name":"tool","arguments":"{}","server_label":"fixture","status":"completed"}),
            json!({"type":"image_generation_call","id":"image","status":"completed"}),
            json!({"type":"shell_call_output","id":"shell-output","call_id":"call","status":"completed",
                "output":[{"stdout":"done","stderr":"","type":"exit","exit_code":0}]}),
        ] {
            let event = |item: &Value| {
                json!({"type":"response.output_item.done",
                "sequence_number":1,"output_index":0,"item":item})
            };
            assert!(tool_completed(&event(&item)), "{item}");
            item["status"] = "in_progress".into();
            assert!(!tool_completed(&event(&item)), "{item}");
            item["status"] = "failed".into();
            assert!(!tool_completed(&event(&item)), "{item}");
            item.as_object_mut().unwrap().remove("status");
            assert!(!tool_completed(&event(&item)), "{item}");
        }

        // MCP status is optional; a successful execution result is also definitive.
        assert!(tool_completed(&json!({"type":"response.output_item.done",
            "sequence_number":1,"output_index":0,"item":{"type":"mcp_call",
                "id":"mcp","name":"tool","arguments":"{}","server_label":"fixture","output":"done"}})));
    }

    #[test]
    fn excludes_client_generated_calls() {
        use async_openai::types::responses::ResponseStreamEvent;

        for item in [
            json!({"type":"local_shell_call","id":"local","call_id":"call","status":"completed",
                "action":{"type":"exec","command":["echo","done"],"env":{}}}),
            json!({"type":"computer_call","id":"computer","call_id":"call","status":"completed",
                "pending_safety_checks":[],"action":{"type":"screenshot"}}),
            json!({"type":"apply_patch_call","id":"patch","call_id":"call","status":"completed",
                "operation":{"type":"delete_file","path":"fixture"}}),
            json!({"type":"shell_call","id":"shell","call_id":"call","status":"completed",
                "action":{"commands":["echo done"]},"environment":{"type":"local"}}),
            json!({"type":"shell_call","id":"shell","call_id":"call","status":"completed",
                "action":{"commands":["echo done"]},"environment":{"type":"container_reference","container_id":"container"}}),
        ] {
            let event = json!({"type":"response.output_item.done",
                "sequence_number":1,"output_index":0,"item":item});
            assert!(serde_json::from_value::<ResponseStreamEvent>(event.clone()).is_ok());
            assert!(!tool_completed(&event), "{item}");
        }
    }
}
