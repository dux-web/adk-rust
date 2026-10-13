- **SQLx 0.9 database APIs** (`adk-session`, `adk-memory`, `adk-graph`,
  `adk-auth`, `adk-rag`): applications sharing pools or handling exposed SQLx
  errors must update their direct SQLx dependency to 0.9. PostgreSQL backends use
  Tokio and rustls with aws-lc-rs; replace removed combined runtime/TLS flags in
  application manifests. Memory's pgvector integration moves to 0.4.2.
- **Migration registry identifiers** (`adk-session`, `adk-memory`): public SQL
  migration runners require ASCII table names starting with a letter or underscore,
  followed by letters, digits or underscores. Descriptions are bound parameters
  and may contain apostrophes. Migration SQL bodies remain trusted executable
  input. Existing database schemas are unchanged.
