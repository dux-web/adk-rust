- **Responses client WebSocket policy** (`adk-model`): reuse a bounded connection,
  preconnect without generating, and select HTTP before the first server event.
  Both transports share request adapters, header observers, parsing and usage
  tracking. Callers own capability and credential revisions; helper generations
  can select HTTP explicitly.
