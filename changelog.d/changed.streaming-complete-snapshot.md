- **Streams end with a complete snapshot** (`adk-model`): Anthropic streaming, and OpenAI
  Responses streaming with commentary, emit a final response carrying the full message with
  `provider_metadata.content_complete` set to `true`. Code that consumes `Llm` streams
  directly replaces the earlier deltas with that response instead of appending it.
