- **Vertex requests are no longer replayed over REST** (`adk-gemini`, `adk-model`): a gRPC
  transport failure on a non-streaming Vertex `generateContent` call is returned instead of
  being re-sent over REST, because the first request may already have reached the model.
  `adk-model` maps those failures to `ErrorCategory::Unavailable`, so `RetryConfig` decides
  whether to retry.
