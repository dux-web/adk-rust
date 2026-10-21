- **Retain composed Anthropic tool constraints** (`adk-model`): root `anyOf`, `oneOf`
  and `allOf` in a custom tool's `input_schema` are nested under `not`/`not` with
  the visible root fields intact, instead of being sent where the Messages API
  rejects them. Declarations marked `"strict": true` keep their schema unchanged,
  since Anthropic does not document `not` for strict tools. The schema adapter
  itself no longer rewrites composition.
