- **Configurable request retries and stream recovery** (`adk-model`): opt-in raw HTTP
  retries preserve transport failures and monotonic server deadlines. Responses
  recovery retains incomplete reasons and completed provider-tool execution across
  HTTP and WebSocket interruptions, independently of fallback and model retries.
