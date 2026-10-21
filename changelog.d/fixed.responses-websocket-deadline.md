- **Responses WebSocket initial events have a bounded deadline** (`adk-model`):
  an unacknowledged request can fall back to HTTP after 15 seconds, independently of the 300-second stream idle bound. Any provider event ends fallback eligibility, including response.created and response.in_progress before text arrives.
  Delivery failures after a provider event preserve their original error category and diagnostics but disable automatic retry, preventing a second generation after acknowledgment or partial output. Explicit provider rejections retain their normal retry guidance.
