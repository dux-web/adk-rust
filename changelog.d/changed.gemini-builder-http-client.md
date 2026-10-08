- **`GeminiBuilder::with_http_client` applies to AI Studio** (`adk-gemini`): the configured
  `reqwest::ClientBuilder` was previously ignored on the AI Studio path and now carries its
  proxy, timeout and default-header settings into every request. The redirect policy is
  always replaced with `Policy::none()`, and a client build failure returns
  `Error::PerformRequestNew` instead of panicking.
