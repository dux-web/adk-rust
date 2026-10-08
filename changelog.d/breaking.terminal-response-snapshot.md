- **Terminal streaming events carry the complete response** (`adk-agent`, `adk-server`): the
  last event of a streamed model response holds the whole response, marked
  `provider_metadata.content_complete = true`, instead of only the final chunk. Raw
  `/api/run_sse` clients that append every event's text show the reply twice; replace the
  content rendered for an event `id` when `content_complete` is set, or pass each event
  through `adk_core::EventTextDeltas`. Tool-result events arrive in completion order rather
  than call order.
