- **Responses WebSocket authorization follows request adapters** (`adk-model`):
  adapters can remove the default Authorization header and inject another header without the constructor key being restored during the handshake.
- **HTTP generation adapters edit the complete configured header set** (`adk-model`):
  Chat Completions and Responses no longer restore constructor or default authorization headers after an adapter removes them.
