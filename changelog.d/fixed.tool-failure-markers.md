- **Explicit tool execution status** (`adk-core`, `adk-agent`): framework-generated
  failures are identified in function-response annotations, including callbacks,
  dispatch rejection and action-ledger failures. Pending, cancelled and uncertain
  calls remain distinct; tool and callback response payloads stay unchanged.
  JSON-only after-tool callbacks retain the response's execution annotations.
  Later callbacks, plugins and result screening preserve an existing unknown
  outcome, including when a postprocessing stage fails or is cancelled.
