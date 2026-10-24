- **Preserve Responses WebSocket protocol errors** (`adk-model`): response
  recovery retains request-scoped error frames, including the original status,
  provider code and cause, instead of replacing them with an unfinished-stream error.
