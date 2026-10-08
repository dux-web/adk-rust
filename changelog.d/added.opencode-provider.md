- **OpenCode Go and Zen** (`adk-model`): the `opencode` feature adds a model adapter that routes
  documented models through Chat Completions, Responses, Messages or Gemini `generateContent`,
  preserving application and conversation headers. The existing clients accept additional
  default HTTP headers, and the Gemini client honours its configured HTTP client builder.
