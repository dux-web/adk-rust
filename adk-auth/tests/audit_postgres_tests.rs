//! `PostgresAuditSink::query` against a live PostgreSQL server.
//!
//! Run with `DATABASE_URL` set and `--run-ignored all`.
#![cfg(feature = "postgres-audit")]

use adk_auth::{
    AuditEvent, AuditEventType, AuditFilter, AuditOutcome, AuditSink, PostgresAuditSink,
};
use chrono::{Duration, TimeZone, Utc};

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL must point at a PostgreSQL server")
}

/// Resources of the events `filter` returns within `tenant`, newest first.
async fn resources(sink: &PostgresAuditSink, tenant: &str, filter: AuditFilter) -> Vec<String> {
    let filter = AuditFilter { tenant_id: Some(tenant.to_string()), ..filter };
    sink.query(&filter).await.expect("query").into_iter().map(|event| event.resource).collect()
}

#[tokio::test]
#[ignore = "requires DATABASE_URL"]
async fn query_applies_every_filter_with_bound_values() {
    let sink = PostgresAuditSink::new(&database_url()).await.expect("connect");
    sink.migrate().await.expect("migrate");
    let tenant = format!("tenant-{}", Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();

    let events = [
        AuditEvent::tool_access("alice", "search", AuditOutcome::Allowed)
            .with_workspace("ws-1")
            .with_resource_id("r-1")
            .with_agent_id("urn:adk:agent:app/researcher"),
        AuditEvent::tool_access("bob", "search_web", AuditOutcome::Denied).with_workspace("ws-2"),
        // A value that would break the statement if it were interpolated.
        AuditEvent::agent_access("o'brien", "planner", AuditOutcome::Allowed)
            .with_resource_id("r-2'; DROP TABLE audit_events; --"),
    ];
    for (offset, event) in events.into_iter().enumerate() {
        let mut event = event.with_tenant(&tenant);
        event.timestamp = start + Duration::minutes(offset as i64);
        sink.log(event).await.expect("log");
    }

    let all = resources(&sink, &tenant, AuditFilter::default()).await;
    assert_eq!(all, ["planner", "search_web", "search"]);

    let cases = [
        (AuditFilter { user: Some("o'brien".into()), ..Default::default() }, vec!["planner"]),
        (
            AuditFilter { workspace_id: Some("ws-2".into()), ..Default::default() },
            vec!["search_web"],
        ),
        (
            AuditFilter { event_type: Some(AuditEventType::AgentAccess), ..Default::default() },
            vec!["planner"],
        ),
        (
            AuditFilter { outcome: Some(AuditOutcome::Denied), ..Default::default() },
            vec!["search_web"],
        ),
        (
            AuditFilter { resource: Some("search".into()), ..Default::default() },
            vec!["search_web", "search"],
        ),
        (
            AuditFilter {
                resource_id: Some("r-2'; DROP TABLE audit_events; --".into()),
                ..Default::default()
            },
            vec!["planner"],
        ),
        (
            AuditFilter {
                agent_id: Some("urn:adk:agent:app/researcher".into()),
                ..Default::default()
            },
            vec!["search"],
        ),
        (
            AuditFilter {
                after: Some(start),
                before: Some(start + Duration::minutes(2)),
                ..Default::default()
            },
            vec!["search_web"],
        ),
        (AuditFilter { limit: Some(1), offset: Some(1), ..Default::default() }, vec!["search_web"]),
    ];
    for (filter, expected) in cases {
        let description = format!("{filter:?}");
        assert_eq!(resources(&sink, &tenant, filter).await, expected, "{description}");
    }
}
