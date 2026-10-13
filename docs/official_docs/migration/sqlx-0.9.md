# Sharing SQLx pools with ADK-Rust 3.0

ADK-Rust 3.0 database backends use SQLx 0.9. SQLx 0.8 pools and errors are distinct
Rust types; update your application's dependency when passing pools into ADK
constructors or using their SQLx error types.

```toml
[dependencies]
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "sqlite"] }
adk-graph = { version = "3.0.0", features = ["sqlite"] }
```

```rust,no_run
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let pool = sqlx::SqlitePool::connect("sqlite::memory:").await?;
let checkpointer = adk_graph::checkpoint::SqliteCheckpointer::from_pool(pool.clone()).await?;
# Ok(())
# }
```

For PostgreSQL, select `postgres`, `runtime-tokio` and
`tls-rustls-aws-lc-rs`. SQLx 0.9 removed combined flags such as
`runtime-tokio-rustls`. ADK's PostgreSQL features enable aws-lc-rs TLS themselves;
a standalone backend no longer depends on another ADK crate enabling TLS.
Memory uses pgvector 0.4.2. SQLite telemetry remains compatible with the same
native SQLite library used by SQLx.

This update does not change persisted database schemas. Existing migrations and
checkpoint data continue through the existing backend migration paths.

The public session and memory migration runners accept registry table names
containing ASCII letters, digits and underscores, starting with a letter or
underscore. Schema-qualified identifiers are not supported. Descriptions may
contain quotes and are bound as data. Migration SQL bodies are trusted
executable SQL: do not construct them from untrusted application input.
