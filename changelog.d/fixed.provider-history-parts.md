- **Preserve provider replay parts between invocations** (`adk-core`, `adk-agent`,
  `adk-runner`, `adk-model`): with a `provider_origin`, reconstruct opaque reasoning and
  signatures only for their captured provider, model and authorization owner. Without
  one, signed thinking, encrypted reasoning and thought signatures replay unchanged
  within an invocation, as before. Preserve visible text, tool history and original
  stored events without duplicating replay parts.
  Each `ProviderData` envelope contains a boxed single-part array, so inspection,
  tool-call bookkeeping and provider projection share the same representation.
  The existing single-element `parts` wire shape stays unchanged; empty or multi-part
  envelopes are rejected rather than partially inspected.
