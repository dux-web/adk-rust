- **Responses transport metadata** (`adk-model`): SSE framing is preserved when the
  response has no content type, and failed requests keep the HTTP status, provider
  code and `Retry-After` hint.
