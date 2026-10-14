- **Anthropic web search request-size errors deserialize** (`adk-anthropic`):
  `web_search_tool_result_error` blocks with the documented `request_too_large` code now
  round-trip instead of failing JSON deserialization.
