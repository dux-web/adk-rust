- **Responses response-header observer** (`adk-model`):
  `OpenAIResponsesClient::with_response_observer` receives the raw headers of every
  Responses attempt, including failed ones, before status handling or body reads.
