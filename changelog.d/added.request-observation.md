- **Generation and request-attempt observations** (`adk-core`, `adk-model`): an opt-in
  scoped observer receives prepared JSON, attempt identities, response-header timing,
  and completion or cancellation outcomes. Reused sockets report reuse without
  repeating earlier handshake timing; observation leaves request and retry behavior unchanged.
