- **Premature Responses EOF remains retryable** (`adk-model`): a stream that
  closes before its terminal event reports an unavailable error; unfinished
  calls in an explicit terminal response remain nonretryable wire errors.
