- **Content-free model attempt failures** (`adk-core`, `adk-model`):
  `model_observation::Kind::Finished` now includes an optional structured failure with
  the static ADK code, typed component and category, and upstream HTTP status. Raw
  HTTP retries and stream errors preserve their cause before a later attempt can
  recover, without copying provider messages, bodies, headers, or arbitrary metadata.
