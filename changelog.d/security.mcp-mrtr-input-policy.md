- **Direct MCP tool calls follow ADK's MRTR input policy** (`adk-tool`):
  `McpToolset::call_tool_value` went through rmcp's `call_tool`, which answered a
  server's MRTR sampling requests with the configured sampling handler. Every toolset on
  `AdkClientHandler`, including one passed to `McpToolset::new`, now applies ADK's input
  policy, which rejects MRTR sampling and roots. On any other client handler, MRTR
  sampling and roots requests reach the handler only when it declared those capabilities.
