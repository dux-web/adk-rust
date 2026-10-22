//! Request retries for native clients using reqwest 0.12.
use super::RequestRetryConfig;
use adk_core::AdkError;
use reqwest::{Client, Error, Request, Response};
use std::time::Instant;

#[derive(Clone, Copy)]
struct RetryDeadline(Instant);

pub(crate) async fn send(
    client: &Client,
    request: Request,
    policy: RequestRetryConfig,
) -> Result<Response, Error> {
    // Streaming uploads cannot be replayed. Model JSON requests are buffered.
    if request.try_clone().is_none() {
        return client.execute(request).await;
    }
    super::execute(
        policy,
        || async {
            let mut response = client
                .execute(request.try_clone().expect("buffered request remains cloneable"))
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
    use tokio::io::AsyncReadExt;

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
