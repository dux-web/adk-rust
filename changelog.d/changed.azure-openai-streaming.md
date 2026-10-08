- **Azure OpenAI streams on the shared Chat Completions transport** (`adk-model`):
  `AzureOpenAIClient` now streams when asked instead of always making a non-streaming
  call, and accepts `with_reasoning_effort`. Error codes stay `model.azure_openai.*`,
  including the status-specific ones.
