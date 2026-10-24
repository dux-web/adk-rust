- **Retain Responses WebSocket refusal causes** (`adk-model`): preconnection
  failures preserve their original status, retry advice and error classification
  for the following generation. Pre-event errors expose `websocket_refusal`, and
  close frames expose their numeric `websocket_close_code`, without recording
  provider close text or authorization material.
