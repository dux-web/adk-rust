- **Token usage counts cache and thought tokens** (`adk-model`): Anthropic
  `prompt_token_count` now includes cache-read and cache-creation input tokens, and Gemini
  `candidates_token_count` now includes thought tokens; `provider_usage` carries the
  provider's raw usage. `adk-eval`'s `CostTracker` prices `prompt_token_count` at the full
  input rate, so cached Anthropic input is counted at that rate, and Claude through Bedrock
  still reports `prompt_token_count` without cache tokens, so the two paths differ for the
  same request.
