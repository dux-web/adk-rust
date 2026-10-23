use super::*;
use futures::FutureExt;

fn recorder() -> (Observer, Arc<Mutex<Vec<Event>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let output = events.clone();
    (Arc::new(move |event| output.lock().unwrap().push(event)), events)
}

#[tokio::test]
async fn separates_raw_attempts_and_stream_completion() {
    let (observer, events) = recorder();
    scope(observer, async {
        let failed = Attempt::start(Transport::Http, None);
        failed.prepared(Some(br#"{"messages":[]}"#));
        failed.headers(503, None);
        failed.finish(Outcome::Failed);
        failed.finish(Outcome::Failed);
        let current = Attempt::start(Transport::Http, None);
        current.prepared(Some(br#"{"messages":[{"role":"user","content":"private"}]}"#));
        current.headers(200, None);
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event.kind, Kind::Finished { .. }))
                .count(),
            1
        );
        current.finish(Outcome::Failed);
        Err::<(), _>(())
    })
    .await
    .unwrap_err();
    let events = events.lock().unwrap();
    let endings: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind {
            Kind::Finished { outcome, .. } => Some((event.attempt, outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(endings, [(1, Outcome::Failed), (2, Outcome::Failed)]);
    assert!(
        matches!(&events[1].kind, Kind::Prepared { body } if body["messages"] == serde_json::json!([]))
    );
}

#[tokio::test]
async fn cancels_pending_stream_without_fabricating_headers() {
    let (observer, events) = recorder();
    let mut pending = Box::pin(scope(observer, async {
        let request = Attempt::start(Transport::WebSocket, None);
        request.connection(true);
        request.prepared_json(&serde_json::json!({"type":"response.create","input":[]}));
        std::future::pending::<Result<(), ()>>().await
    }));
    assert!(pending.as_mut().now_or_never().is_none());
    drop(pending);
    let events = events.lock().unwrap();
    assert!(!events.iter().any(|event| matches!(event.kind, Kind::Headers { .. })));
    assert!(matches!(
        events.last().unwrap().kind,
        Kind::Finished { outcome: Outcome::Cancelled, failure: None, .. }
    ));
}

#[tokio::test]
async fn isolates_nested_calls_and_unscoped_requests() {
    let (outer, outer_events) = recorder();
    let (inner, inner_events) = recorder();
    Attempt::start(Transport::Http, None).headers(200, None);
    scope(outer, async {
        let outer_attempt = Attempt::start(Transport::Http, None);
        scope(inner, async {
            Attempt::start(Transport::WebSocket, None).finish(Outcome::Succeeded);
            Ok::<_, ()>(())
        })
        .await?;
        outer_attempt.finish(Outcome::Succeeded);
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    for events in [outer_events, inner_events] {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.attempt == 1));
        assert!(matches!(
            events[1].kind,
            Kind::Finished { outcome: Outcome::Succeeded, failure: None, .. }
        ));
    }
}

#[tokio::test]
async fn successful_scope_does_not_complete_abandoned_attempts() {
    let (observer, events) = recorder();
    scope(observer, async {
        Attempt::start(Transport::Http, None).headers(200, None);
        let next = Attempt::start(Transport::Http, None);
        next.finish(Outcome::Succeeded);
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    let events = events.lock().unwrap();
    let endings: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind {
            Kind::Finished { outcome, .. } => Some((event.attempt, outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(endings, [(2, Outcome::Succeeded), (1, Outcome::Cancelled)]);
}

#[tokio::test]
async fn projects_only_fixed_failure_fields() {
    use crate::{AdkError, ErrorCategory, ErrorComponent};
    let (observer, events) = recorder();
    let mut error = AdkError::new(
        ErrorComponent::Model,
        ErrorCategory::Unavailable,
        "model.fixture.failed",
        "private response body",
    )
    .with_provider("private provider")
    .with_request_id("private upstream request")
    .with_upstream_status(503);
    error.details.metadata.insert("provider_error_code".into(), "private code".into());
    let expected = Failure {
        code: "model.fixture.failed",
        component: ErrorComponent::Model,
        category: ErrorCategory::Unavailable,
        upstream_status: Some(503),
    };
    scope(observer, async {
        let attempt = Attempt::start(Transport::Http, None);
        attempt.fail(&error);
        attempt.finish(Outcome::Succeeded);
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[1].kind,
        Kind::Finished { outcome: Outcome::Failed, failure: Some(failure), .. }
            if failure == expected
    ));
    assert!(!format!("{:?}", events[1]).contains("private"));
}

#[tokio::test]
async fn retains_failure_before_successful_retry() {
    use crate::{AdkError, ErrorCategory, ErrorComponent};
    let (observer, events) = recorder();
    let error = AdkError::new(
        ErrorComponent::Model,
        ErrorCategory::Timeout,
        "model.fixture.timeout",
        "private timeout detail",
    );
    scope(observer, async {
        operation_with_failure(
            async {
                Attempt::start(Transport::Http, None);
                Err::<(), _>(error)
            },
            |error| Failure::from(error),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            events.lock().unwrap().last().unwrap().kind,
            Kind::Finished { outcome: Outcome::Failed, failure: Some(_), .. }
        ));
        Attempt::start(Transport::Http, None).finish(Outcome::Succeeded);
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    let events = events.lock().unwrap();
    let endings: Vec<_> = events
        .iter()
        .filter_map(|event| match event.kind {
            Kind::Finished { outcome, failure, .. } => Some((event.attempt, outcome, failure)),
            _ => None,
        })
        .collect();
    assert_eq!(
        endings,
        [
            (
                1,
                Outcome::Failed,
                Some(Failure {
                    code: "model.fixture.timeout",
                    component: ErrorComponent::Model,
                    category: ErrorCategory::Timeout,
                    upstream_status: None,
                }),
            ),
            (2, Outcome::Succeeded, None),
        ]
    );
}

#[tokio::test]
async fn preserves_typed_stream_failure() {
    use crate::{AdkError, ErrorCategory, ErrorComponent};
    use futures::StreamExt;
    let (observer, events) = recorder();
    let error = AdkError::new(
        ErrorComponent::Model,
        ErrorCategory::Internal,
        "model.fixture.parse",
        "private malformed output",
    );
    let expected = Failure::from(&error);
    scope(observer, async {
        Attempt::start(Transport::Http, None);
        let mut stream = observe_stream(Box::pin(futures::stream::iter([Err(error)])));
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.message, "private malformed output");
        drop(stream);
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[1].kind,
        Kind::Finished { outcome: Outcome::Failed, failure: Some(failure), .. }
            if failure == expected
    ));
    assert!(!format!("{:?}", events[1]).contains("private"));
}
