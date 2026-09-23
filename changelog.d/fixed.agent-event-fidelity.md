- **Agent event fidelity** (`adk-agent`): preserve citations and complete terminal content,
  continue provider-native paused turns, and emit individual tool results while sibling
  approvals are pending. Propagate approval failures and keep tool-start events from ending
  responses. Built-in streaming consumers avoid appending complete snapshots twice;
  `adk-core::EventTextDeltas` exposes the same text/thinking adapter to applications.
