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
        Kind::Finished { outcome: Outcome::Cancelled, .. }
    ));
}

#[tokio::test]
async fn isolates_nested_calls_and_unscoped_requests() {
    let (outer, outer_events) = recorder();
    let (inner, inner_events) = recorder();
    Attempt::start(Transport::Http, None).headers(200, None);
    scope(outer, async {
        Attempt::start(Transport::Http, None);
        scope(inner, async {
            Attempt::start(Transport::WebSocket, None);
            Ok::<_, ()>(())
        })
        .await?;
        Ok::<_, ()>(())
    })
    .await
    .unwrap();
    for events in [outer_events, inner_events] {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.attempt == 1));
        assert!(matches!(events[1].kind, Kind::Finished { outcome: Outcome::Succeeded, .. }));
    }
}
