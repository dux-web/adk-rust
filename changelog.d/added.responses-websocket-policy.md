OpenAI Responses can opt into a session-owned WebSocket with shared HTTP request
adaptation, header observation, response parsing and usage tracking. Handshake-only
preconnection, idle and maximum lifetimes, credential revisions and transport
observations give callers lifecycle control without another retry owner.

The first provider event has a fixed 15-second default deadline. An HTTP fallback
before that event may generate and bill twice if the server already accepted the
original request. Accepted requests are not automatically regenerated after socket
delivery failures. Explicit terminal failures keep their documented error identity
and leave a completed socket reusable. Caller beta headers, platform certificate
verification, proxy bypass rules and legacy endpoint forms are preserved.
