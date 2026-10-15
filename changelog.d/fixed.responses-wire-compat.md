- **Responses parsing validates consumed fields** (`adk-model`): lifecycle and
  response metadata accept provider extensions without weakening content, tool
  or terminal validation. Streaming and JSON responses share terminal rules;
  failed, cancelled, nonterminal and truncated responses do not complete successfully.
- **Premature Responses EOF remains retryable** (`adk-model`): a stream that
  closes before its terminal event reports an unavailable error; unfinished
  calls in an explicit terminal response remain nonretryable wire errors.
