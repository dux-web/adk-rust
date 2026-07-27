use crate::{InvocationContext, Result, event::Event};
use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// A pinned, boxed stream of [`Event`] results emitted by an agent during execution.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event>> + Send>>;

/// The fundamental trait for all ADK agents.
///
/// Every agent — whether a simple LLM wrapper, a multi-step workflow, or a
/// composite orchestrator — implements this trait. The runtime invokes
/// [`run`](Self::run) with an [`InvocationContext`] and consumes the returned
/// [`EventStream`].
#[async_trait]
pub trait Agent: Send + Sync {
    /// Returns the unique name of this agent.
    fn name(&self) -> &str;
    /// Returns a human-readable description of this agent's purpose.
    fn description(&self) -> &str;
    /// Returns the child agents managed by this agent.
    fn sub_agents(&self) -> &[Arc<dyn Agent>];

    /// The system instruction this agent will send, when it is fixed for every turn.
    ///
    /// A runner uses this as the material for provider-side context caching. It must be
    /// the instruction the agent actually sends, not a stand-in: a cache built from
    /// different text than the request it is attached to is either ignored by the
    /// provider or applies the wrong instruction.
    ///
    /// Returns `None` — the default — when the agent has no fixed instruction, for
    /// example because it is computed per request. A runner must then skip caching
    /// rather than caching something else.
    fn cacheable_instruction(&self) -> Option<&str> {
        None
    }

    /// Whether this agent participates in LLM-driven agent transfer and may be
    /// resumed directly across conversation turns.
    ///
    /// When a session persists across turns, the runner inspects history to
    /// decide which agent should handle the next user message. LLM-based and
    /// custom agents return the default `true`, so the runner can hand a new
    /// turn back to whichever agent responded last.
    ///
    /// Deterministic workflow agents (sequential, parallel, loop, conditional)
    /// override this to return `false`. Their sub-agents must not be resumed
    /// individually: doing so would skip the workflow's other sub-agents on
    /// subsequent turns. Returning `false` makes the runner resume the workflow
    /// root instead, so every sub-agent runs again on each turn.
    fn supports_agent_transfer(&self) -> bool {
        true
    }

    /// Executes the agent and returns a stream of events.
    async fn run(&self, ctx: Arc<dyn InvocationContext>) -> Result<EventStream>;
}

/// A validated context containing engineered instructions and resolved tool instances.
///
/// This structure serves as the "Atomic Unit of Capability" for an agent. It guarantees
/// that the agent's cognitive frame (the instructions telling it what it can do) is
/// perfectly aligned with its physical capabilities (the binary tool instances bound
/// to the session).
///
/// By using `ResolvedContext`, the framework eliminates "Phantom Tool" hallucinations,
/// where an agent tries to call a tool that was mentioned in its prompt but never
/// actually registered in the runtime.
#[derive(Clone)]
pub struct ResolvedContext {
    /// The engineered system instruction.
    pub system_instruction: String,
    /// The resolved, executable tools.
    pub active_tools: Vec<Arc<dyn crate::Tool>>,
}

impl std::fmt::Debug for ResolvedContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedContext")
            .field("system_instruction_len", &self.system_instruction.len())
            .field("active_tools_count", &self.active_tools.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Content, ReadonlyContext, RunConfig};
    use async_stream::stream;

    struct TestAgent {
        name: String,
    }

    use crate::{CallbackContext, Session, State};
    use std::collections::HashMap;

    struct MockState;
    impl State for MockState {
        fn get(&self, _key: &str) -> Option<serde_json::Value> {
            None
        }
        fn set(&mut self, _key: String, _value: serde_json::Value) {}
        fn all(&self) -> HashMap<String, serde_json::Value> {
            HashMap::new()
        }
    }

    struct MockSession;
    impl Session for MockSession {
        fn id(&self) -> &str {
            "session"
        }
        fn app_name(&self) -> &str {
            "app"
        }
        fn user_id(&self) -> &str {
            "user"
        }
        fn state(&self) -> &dyn State {
            &MockState
        }
        fn conversation_history(&self) -> Vec<Content> {
            Vec::new()
        }
    }

    #[allow(dead_code)]
    struct TestContext {
        content: Content,
        config: RunConfig,
        session: MockSession,
    }

    #[allow(dead_code)]
    impl TestContext {
        fn new() -> Self {
            Self {
                content: Content::new("user"),
                config: RunConfig::default(),
                session: MockSession,
            }
        }
    }

    #[async_trait]
    impl ReadonlyContext for TestContext {
        fn invocation_id(&self) -> &str {
            "test"
        }
        fn agent_name(&self) -> &str {
            "test"
        }
        fn user_id(&self) -> &str {
            "user"
        }
        fn app_name(&self) -> &str {
            "app"
        }
        fn session_id(&self) -> &str {
            "session"
        }
        fn branch(&self) -> &str {
            ""
        }
        fn user_content(&self) -> &Content {
            &self.content
        }
    }

    #[async_trait]
    impl CallbackContext for TestContext {
        fn artifacts(&self) -> Option<Arc<dyn crate::Artifacts>> {
            None
        }
    }

    #[async_trait]
    impl InvocationContext for TestContext {
        fn agent(&self) -> Arc<dyn Agent> {
            unimplemented!()
        }
        fn memory(&self) -> Option<Arc<dyn crate::Memory>> {
            None
        }
        fn session(&self) -> &dyn Session {
            &self.session
        }
        fn run_config(&self) -> &RunConfig {
            &self.config
        }
        fn end_invocation(&self) {}
        fn ended(&self) -> bool {
            false
        }
    }

    #[async_trait]
    impl Agent for TestAgent {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "test agent"
        }

        fn sub_agents(&self) -> &[Arc<dyn Agent>] {
            &[]
        }

        async fn run(&self, _ctx: Arc<dyn InvocationContext>) -> Result<EventStream> {
            let s = stream! {
                yield Ok(Event::new("test"));
            };
            Ok(Box::pin(s))
        }
    }

    #[test]
    fn test_agent_trait() {
        let agent = TestAgent { name: "test".to_string() };
        assert_eq!(agent.name(), "test");
        assert_eq!(agent.description(), "test agent");
    }
}
