- **`McpToolset::cancel_pending_tasks`** (`adk-tool`): cancels remote MCP tasks whose
  tool-call future was dropped, and in-flight ones, and releases each task once the
  server reports it terminal or no longer knows it. `McpServerManager` runs it within the
  grace period when it stops a server.
