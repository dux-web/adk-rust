- **Background submissions finish their HTTP observation** (`adk-model`): validated
  non-streaming `queued` and `in_progress` responses record success before yielding,
  including when the caller drops the stream immediately. The background generation
  remains pending; ordinary unfinished streams still fail.
