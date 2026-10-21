- **Preserve provider replay parts between invocations** (`adk-core`, `adk-agent`,
  `adk-runner`, `adk-model`): reconstruct opaque reasoning and signatures only for
  their captured provider, model and authorization owner. Preserve visible text,
  tool history and original stored events without duplicating replay parts.
