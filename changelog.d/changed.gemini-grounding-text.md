- **Gemini grounding is no longer appended to the answer** (`adk-model`): the
  "Searched" and "Sources" text that followed Google Search grounded responses is gone. The
  queries, sources and supports remain in `provider_metadata` (`webSearchQueries`,
  `groundingChunks`, `groundingSupports`) for applications that display them.
