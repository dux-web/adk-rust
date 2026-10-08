- **Direct MCP tool calls keep the task lifecycle and the connection's input policy**
  (`adk-tool`): `McpToolset::call_tool_value` polls server-materialized tasks, answers
  MRTR and in-task input through the connection's handler, and restores resource
  subscriptions after a reconnect, as model-facing calls do. A task returned while task
  support is disabled is cancelled, and the error names
  `McpToolset::with_task_support(McpTaskConfig::enabled())`. MRTR input forwards sampling
  and roots only when the client declared them, toolsets built on ADK's own handler apply
  its input policy, and each input round is capped at 64 requests. A pending elicitation
  no longer holds the connection lock, and an error result without a text block reports
  its structured content.
