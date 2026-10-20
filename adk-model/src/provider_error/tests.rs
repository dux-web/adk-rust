use super::*;
use adk_core::{ErrorCategory, ErrorComponent};
use std::sync::atomic::{AtomicUsize, Ordering};

fn failure(message: &str) -> AdkError {
    adapt(
        AdkError::new(
            ErrorComponent::Model,
            ErrorCategory::Unavailable,
            "model.fixture",
            "provider request failed",
        ),
        || ProviderErrorDetails { message: Some(message.into()), ..Default::default() },
    )
}

fn adapter(secret: &'static str) -> ErrorAdapter {
    Arc::new(move |mut error, details| {
        assert!(!format!("{details:?}").contains(secret));
        error.message = details.message.unwrap().replace(secret, "[redacted]");
        error
    })
}

#[tokio::test]
async fn isolates_opening_and_stream_polls() {
    async fn run(secret: &'static str, opening: bool) -> AdkError {
        let future = async move {
            tokio::task::yield_now().await;
            if opening {
                return Err(failure(secret));
            }
            Ok(Box::pin(async_stream::stream! {
                tokio::task::yield_now().await;
                yield Err(failure(secret));
            }) as LlmResponseStream)
        };
        match inspect_errors(adapter(secret), future).await {
            Err(error) => error,
            Ok(mut stream) => stream.next().await.unwrap().unwrap_err(),
        }
    }
    for opening in [false, true] {
        let (first, second) = tokio::join!(run("FIRST", opening), run("SECOND", opening));
        for error in [first, second] {
            assert_eq!(error.message, "[redacted]");
            assert_eq!(error.category, ErrorCategory::Unavailable);
            assert!(std::error::Error::source(&error).is_none());
        }
        assert!(!enabled());
    }
}

#[tokio::test]
async fn restores_scope_after_cancellation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let adapter: ErrorAdapter = Arc::new(move |error, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        error
    });
    let pending = async {
        let _error = failure("opening");
        std::future::pending::<Result<LlmResponseStream>>().await
    };
    let mut future = Box::pin(inspect_errors(adapter.clone(), pending));
    assert!(futures::poll!(&mut future).is_pending());
    drop(future);
    assert!(!enabled());
    let _outside = failure("outside");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let mut stream = inspect_errors(adapter, async {
        Ok(Box::pin(async_stream::stream! {
            let _error = failure("stream");
            std::future::pending::<()>().await;
            yield Err(failure("unreachable"));
        }) as LlmResponseStream)
    })
    .await
    .unwrap();
    assert!(futures::poll!(stream.next()).is_pending());
    drop(stream);
    assert!(!enabled());
    let _outside = failure("outside");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn omits_oversized_fields_without_cutting_secrets() {
    assert!(ProviderErrorDetails::from_body(&"x".repeat(64 * 1024 + 1)).message.is_none());
    let value =
        serde_json::json!({"error": {"message": "x".repeat(64 * 1024 + 1), "code": "limit"}});
    let details = ProviderErrorDetails::from_value(&value);
    assert!(details.message.is_none());
    assert_eq!(details.code.as_deref(), Some("limit"));
    assert!(!enabled());
}

#[test]
fn retains_only_structured_messages() {
    for body in [
        "private provider response SECRET",
        "<html>private provider response SECRET</html>",
        r#"{"error":{"code":"denied","debug":"SECRET"}}"#,
        r#"{"debug":"SECRET"}"#,
    ] {
        assert!(ProviderErrorDetails::from_body(body).message.is_none());
    }
    for body in [
        r#"{"message":"provider message"}"#,
        r#"{"error":{"message":"provider message","code":"denied"}}"#,
        r#"{"response":{"error":{"detail":"provider message"}}}"#,
    ] {
        assert_eq!(
            ProviderErrorDetails::from_body(body).message.as_deref(),
            Some("provider message")
        );
    }
    let details = ProviderErrorDetails::from_body(r#"{"error":{"code":"denied"}}"#);
    assert_eq!(details.code.as_deref(), Some("denied"));
}
