- **Agent event fidelity** (`adk-agent`, `adk-runner`, `adk-cli`, `adk-graph`): preserve
  citations and complete terminal content, continue provider-native paused turns, and emit
  individual tool results while sibling approvals are pending. An approval failure ends the
  turn with an error once running tools finish, approvals are requested one at a time, and
  tool-start events no longer end responses. Built-in streaming consumers render complete
  snapshots once, and later agents in the same invocation see each streamed response once.
