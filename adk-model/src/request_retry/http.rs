//! Request retries for native clients using reqwest 0.12.
use super::RequestRetryConfig;
use adk_core::model_observation::{Attempt, Failure, Transport};
use adk_core::{AdkError, ErrorCategory, ErrorComponent};
use reqwest::{Client, Error, Request, Response};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
struct RetryDeadline(Instant);

pub(crate) async fn send(
    client: &Client,
    request: Request,
    policy: RequestRetryConfig,
) -> Result<Response, Error> {
    // Streaming uploads cannot be replayed. Model JSON requests are buffered.
    if request.try_clone().is_none() {
        return observed(client, request).await;
    }
    super::execute(
        policy,
        || async {
            let mut response =
                observed(client, request.try_clone().expect("buffered request remains cloneable"))
                    .await?;
            if let Some(deadline) = deadline(&response) {
                response.extensions_mut().insert(RetryDeadline(deadline));
            }
            Ok::<_, Error>(response)
        },
        |result| match result {
            Ok(response) => response.status().is_server_error(),
            Err(error) => !error.is_builder() && !error.is_redirect(),
        },
        |result| result.as_ref().ok().and_then(deadline),
    )
    .await
}

#[derive(Debug)]
pub(crate) enum SendError {
    Request(Error),
    HeadersTimeout,
}

pub(crate) async fn send_timed(
    client: &Client,
    request: Request,
    policy: RequestRetryConfig,
    headers_timeout: Duration,
) -> Result<Response, SendError> {
    if request.try_clone().is_none() {
        return observed_timed(client, request, Some(headers_timeout)).await;
    }
    super::execute(
        policy,
        || {
            observed_timed(
                client,
                request.try_clone().expect("buffered request remains cloneable"),
                Some(headers_timeout),
            )
        },
        |result| match result {
            Ok(response) => response.status().is_server_error(),
            Err(SendError::HeadersTimeout) => true,
            Err(SendError::Request(error)) => !error.is_builder() && !error.is_redirect(),
        },
        |result| result.as_ref().ok().and_then(deadline),
    )
    .await
}

async fn observed(client: &Client, request: Request) -> Result<Response, Error> {
    match observed_timed(client, request, None).await {
        Ok(response) => Ok(response),
        Err(SendError::Request(error)) => Err(error),
        Err(SendError::HeadersTimeout) => unreachable!("no explicit header timeout"),
    }
}

async fn observed_timed(
    client: &Client,
    request: Request,
    timeout: Option<Duration>,
) -> Result<Response, SendError> {
    let attempt = Attempt::start(Transport::Http, None);
    attempt.prepared(request.body().and_then(|body| body.as_bytes()));
    let send = client.execute(request);
    let mut result = match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, send).await {
            Ok(result) => result.map_err(SendError::Request),
            Err(_) => Err(SendError::HeadersTimeout),
        },
        None => send.await.map_err(SendError::Request),
    };
    match &mut result {
        Ok(response) => {
            attempt.headers(response.status().as_u16(), None);
            if let Some(deadline) = deadline(response) {
                response.extensions_mut().insert(RetryDeadline(deadline));
            }
            if !response.status().is_success() {
                attempt.finish_failure(status_failure(response.status().as_u16()));
            }
        }
        Err(SendError::Request(error)) => attempt.finish_failure(request_failure(error)),
        Err(SendError::HeadersTimeout) => attempt.finish_failure(Failure {
            code: "model.http.timeout",
            component: ErrorComponent::Model,
            category: ErrorCategory::Timeout,
            upstream_status: None,
        }),
    }
    result
}

pub(crate) fn status_failure(status: u16) -> Failure {
    Failure {
        code: "model.http.status",
        component: ErrorComponent::Model,
        category: crate::retry::category_for_status_code(status),
        upstream_status: Some(status),
    }
}

fn request_failure(error: &Error) -> Failure {
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

pub(crate) fn deadline(response: &Response) -> Option<Instant> {
    response.extensions().get::<RetryDeadline>().map(|value| value.0).or_else(|| {
        response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(crate::retry::parse_retry_after)
            .and_then(|delay| Instant::now().checked_add(delay))
    })
}

pub(crate) fn annotate(error: AdkError, phase: &str) -> AdkError {
    crate::error_metadata::transport(error, phase)
}

pub(crate) fn network(error: AdkError, source: &Error, phase: &str) -> AdkError {
    let mut error = annotate(error, phase);
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

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::model_observation::{self, Event, Kind, Outcome};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn recorder() -> (model_observation::Observer, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        (Arc::new(move |event| captured.lock().unwrap().push(event)), events)
    }

    fn endings(events: &[Event]) -> Vec<(u64, Outcome, Option<Failure>)> {
        events
            .iter()
            .filter_map(|event| match event.kind {
                Kind::Finished { outcome, failure, .. } => Some((event.attempt, outcome, failure)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn retains_timeout_after_recovery() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stalled, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            stalled.read(&mut buffer).await.unwrap();
            // Keep the first request open until the timed retry connects.
            let (mut recovered, _) = listener.accept().await.unwrap();
            recovered.read(&mut buffer).await.unwrap();
            recovered
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            drop(stalled);
        });
        let client = Client::builder().retry(reqwest::retry::never()).build().unwrap();
        let request = client.post(format!("http://{address}/generate")).body("{}").build().unwrap();
        let (observer, events) = recorder();
        model_observation::scope(observer, async {
            let response = send_timed(
                &client,
                request,
                RequestRetryConfig { max_retries: 1, initial_delay: Duration::ZERO },
                Duration::from_millis(250),
            )
            .await
            .unwrap();
            assert!(response.status().is_success());
            Attempt::current().finish(Outcome::Succeeded);
            Ok::<_, ()>(())
        })
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(
            endings(&events.lock().unwrap()),
            [
                (
                    1,
                    Outcome::Failed,
                    Some(Failure {
                        code: "model.http.timeout",
                        component: ErrorComponent::Model,
                        category: ErrorCategory::Timeout,
                        upstream_status: None,
                    })
                ),
                (2, Outcome::Succeeded, None),
            ]
        );
    }

    #[tokio::test]
    async fn identifies_connection_failures() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = Client::builder().retry(reqwest::retry::never()).build().unwrap();
        let request = client.post(format!("http://{address}/generate")).body("{}").build().unwrap();
        let (observer, events) = recorder();
        model_observation::scope(observer, async {
            let error = send(&client, request, RequestRetryConfig::default()).await.unwrap_err();
            assert!(error.is_connect());
            Err::<(), _>(error)
        })
        .await
        .unwrap_err();
        assert_eq!(
            endings(&events.lock().unwrap()),
            [(
                1,
                Outcome::Failed,
                Some(Failure {
                    code: "model.http.connection",
                    component: ErrorComponent::Model,
                    category: ErrorCategory::Unavailable,
                    upstream_status: None,
                })
            ),]
        );
    }

    #[tokio::test]
    async fn bounds_failed_network_sends() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..5 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                socket.read(&mut buffer).await.unwrap();
                // Disconnect before sending response headers.
            }
        });
        let client = Client::builder().retry(reqwest::retry::never()).build().unwrap();
        let request = client
            .post(format!("http://{address}/generate"))
            .json(&serde_json::json!({"input":"test"}))
            .build()
            .unwrap();
        let policy =
            RequestRetryConfig { max_retries: 4, initial_delay: std::time::Duration::ZERO };
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), send(&client, request, policy))
                .await
                .unwrap();
        assert!(result.is_err());
        server.await.unwrap();
    }
}
