- **Model-facing MCP tools forward MRTR input to a custom client handler** (`adk-tool`):
  on `McpToolset::new` with a handler other than `AdkClientHandler`, an MRTR input
  request from a tool call returned an error. It now reaches the handler, whose default
  `create_elicitation` declines.
