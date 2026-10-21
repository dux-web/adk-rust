- **Responses WebSocket initial events have a bounded deadline** (`adk-model`):
  an unacknowledged request can fall back to HTTP after 15 seconds, independently of the 300-second stream idle bound. Any provider event ends fallback eligibility, including response.created and response.in_progress before text arrives.
