- **OpenCode Go and Zen** (`adk-model`): the `opencode` feature adds `OpenCodeClient`, which
  routes documented models through Chat Completions, Responses, Anthropic Messages or Gemini
  `generateContent` and sends the application's `User-Agent` and `x-opencode-session` headers
  with every request. Unknown models take an explicit `OpenCodeApi`.
- **Default HTTP headers on model clients** (`adk-anthropic`, `adk-model`):
  `Anthropic::with_default_headers`, `AnthropicClient::with_default_headers`,
  `OpenAICompatible::with_default_headers` and `OpenAIResponsesClient::with_default_headers`
  add headers to every request. Authentication headers and disabled redirects are kept, and
  the Responses client keeps its request rewriting and adapter in either call order.
