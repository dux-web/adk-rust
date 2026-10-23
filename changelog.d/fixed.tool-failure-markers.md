- **Tool dispatch failures remain inspectable** (`adk-agent`): framework-authored
  failures carry `isError: true`, including calls rejected before execution and
  failed after-tool callbacks. Successful tool payloads remain unchanged.
