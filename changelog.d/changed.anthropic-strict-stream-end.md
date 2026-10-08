- **Anthropic streams must end with `message_stop`** (`adk-model`): a stream that closes
  without `message_stop`, or whose message has no stop reason, now fails instead of
  ending quietly. Anthropic-compatible proxies that drop the final event are affected.
