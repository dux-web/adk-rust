//! Property-based tests for adk-agent.
//!
//! These tests verify universal properties of agent execution using proptest
//! with 100+ randomly generated inputs per property.

use adk_agent::{CustomAgentBuilder, ParallelAgent, SequentialAgent};
use adk_core::{
    Agent, Content, Event, EventStream, InvocationContext, Part, ReadonlyContext, RunConfig,
    ToolExecutionStrategy,
};
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

struct MockState;

impl adk_core::State for MockState {
    fn get(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
    fn set(&mut self, _key: String, _value: serde_json::Value) {}
    fn all(&self) -> HashMap<String, serde_json::Value> {
        HashMap::new()
    }
}

struct MockSession;

impl adk_core::Session for MockSession {
    fn id(&self) -> &str {
        "test-session"
    }
    fn app_name(&self) -> &str {
        "test-app"
    }
    fn user_id(&self) -> &str {
        "test-user"
    }
    fn state(&self) -> &dyn adk_core::State {
        &MockState
    }
    fn conversation_history(&self) -> Vec<Content> {
        Vec::new()
    }
}

struct TestContext {
    content: Content,
    config: RunConfig,
}

impl TestContext {
    fn new(message: &str) -> Self {
        Self {
            content: Content {
                role: "user".to_string(),
                parts: vec![Part::Text { text: message.to_string() }],
            },
            config: RunConfig::default(),
        }
    }
}

#[async_trait]
impl ReadonlyContext for TestContext {
    fn invocation_id(&self) -> &str {
        "test-invocation"
    }
    fn agent_name(&self) -> &str {
        "test-agent"
    }
    fn user_id(&self) -> &str {
        "test-user"
    }
    fn app_name(&self) -> &str {
        "test-app"
    }
    fn session_id(&self) -> &str {
        "test-session"
    }
    fn branch(&self) -> &str {
        ""
    }
    fn user_content(&self) -> &Content {
        &self.content
    }
}

#[async_trait]
impl adk_core::CallbackContext for TestContext {
    fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
        None
    }
}

#[async_trait]
impl InvocationContext for TestContext {
    fn agent(&self) -> Arc<dyn Agent> {
        unimplemented!()
    }
    fn memory(&self) -> Option<Arc<dyn adk_core::Memory>> {
        None
    }
    fn run_config(&self) -> &RunConfig {
        &self.config
    }
    fn end_invocation(&self) {}
    fn ended(&self) -> bool {
        false
    }
    fn session(&self) -> &dyn adk_core::Session {
        &MockSession
    }
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Generate valid agent names: alphanumeric with underscores, 1-20 chars.
fn arb_agent_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,19}".prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a vector of unique agent names (1 to max_count).
fn arb_agent_names(max_count: usize) -> impl Strategy<Value = Vec<String>> {
    proptest::collection::hash_set(arb_agent_name(), 1..=max_count)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>())
}

/// Generate a ToolExecutionStrategy variant.
fn arb_tool_execution_strategy() -> impl Strategy<Value = ToolExecutionStrategy> {
    prop_oneof![
        Just(ToolExecutionStrategy::Sequential),
        Just(ToolExecutionStrategy::Parallel),
        Just(ToolExecutionStrategy::Auto),
    ]
}

// ---------------------------------------------------------------------------
// Property 1: Agent Event Stream Well-Formedness — Valid Author Fields
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: one-point-zero-readiness, Property 1: Agent Event Stream Well-Formedness**
    /// *For any* valid agent configuration with sub-agents, the event stream produced
    /// by agent execution SHALL contain events where each event has a valid author
    /// field matching either the agent name or a sub-agent name.
    /// **Validates: Requirements 8.2**
    #[test]
    fn prop_event_stream_has_valid_author_fields(
        agent_names in arb_agent_names(5)
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Build custom agents that emit events with their own name as author
            let sub_agents: Vec<Arc<dyn Agent>> = agent_names
                .iter()
                .map(|name| {
                    let agent_name = name.clone();
                    let agent = CustomAgentBuilder::new(name.as_str())
                        .description("test sub-agent")
                        .handler(move |_ctx| {
                            let author = agent_name.clone();
                            async move {
                                let mut event = Event::new("test-invocation");
                                event.author = author;
                                event.llm_response.content = Some(Content {
                                    role: "assistant".to_string(),
                                    parts: vec![Part::Text {
                                        text: "response".to_string(),
                                    }],
                                });
                                Ok(Box::pin(stream::iter(vec![Ok(event)])) as EventStream)
                            }
                        })
                        .build()
                        .unwrap();
                    Arc::new(agent) as Arc<dyn Agent>
                })
                .collect();

            // Use a SequentialAgent to orchestrate sub-agents
            let sequential = SequentialAgent::new("orchestrator", sub_agents);

            let ctx = Arc::new(TestContext::new("test input"));
            let mut event_stream = sequential.run(ctx).await.unwrap();

            // Collect all events and verify author fields
            let valid_authors: std::collections::HashSet<&str> = agent_names
                .iter()
                .map(|s| s.as_str())
                .collect();

            let mut event_count = 0;
            while let Some(result) = event_stream.next().await {
                let event = result.unwrap();
                event_count += 1;
                assert!(
                    valid_authors.contains(event.author.as_str()),
                    "Event author '{}' is not in the set of valid agent names: {:?}",
                    event.author,
                    valid_authors
                );
            }

            // We should have received one event per sub-agent
            assert_eq!(event_count, agent_names.len());
        });
    }

    /// **Feature: one-point-zero-readiness, Property 1: Agent Event Stream Well-Formedness (Parallel)**
    /// *For any* valid parallel agent configuration, the event stream SHALL contain
    /// events where each author matches a sub-agent name.
    /// **Validates: Requirements 8.2**
    #[test]
    fn prop_parallel_agent_event_authors_are_valid(
        agent_names in arb_agent_names(5)
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let sub_agents: Vec<Arc<dyn Agent>> = agent_names
                .iter()
                .map(|name| {
                    let agent_name = name.clone();
                    let agent = CustomAgentBuilder::new(name.as_str())
                        .description("test sub-agent")
                        .handler(move |_ctx| {
                            let author = agent_name.clone();
                            async move {
                                let mut event = Event::new("test-invocation");
                                event.author = author;
                                Ok(Box::pin(stream::iter(vec![Ok(event)])) as EventStream)
                            }
                        })
                        .build()
                        .unwrap();
                    Arc::new(agent) as Arc<dyn Agent>
                })
                .collect();

            let parallel = ParallelAgent::new("parallel_orchestrator", sub_agents);

            let ctx = Arc::new(TestContext::new("test input"));
            let mut event_stream = parallel.run(ctx).await.unwrap();

            let valid_authors: std::collections::HashSet<&str> = agent_names
                .iter()
                .map(|s| s.as_str())
                .collect();

            let mut event_count = 0;
            while let Some(result) = event_stream.next().await {
                let event = result.unwrap();
                event_count += 1;
                assert!(
                    valid_authors.contains(event.author.as_str()),
                    "Parallel event author '{}' not in valid set: {:?}",
                    event.author,
                    valid_authors
                );
            }

            assert_eq!(event_count, agent_names.len());
        });
    }
}

// ---------------------------------------------------------------------------
// Property: Tool Execution Strategy Produces Correct Results Regardless of Count
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: one-point-zero-readiness, Property 1: Tool Execution Strategy Correctness**
    /// *For any* tool execution strategy and any tool count (1-10), the strategy
    /// SHALL produce the correct number of results matching the tool count.
    /// **Validates: Requirements 8.2**
    #[test]
    fn prop_tool_execution_strategy_produces_correct_results(
        strategy in arb_tool_execution_strategy(),
        tool_count in 1usize..=10,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Create `tool_count` sub-agents, each representing a "tool execution"
            // that produces one event. The strategy determines execution order but
            // the total result count must always equal tool_count.
            let sub_agents: Vec<Arc<dyn Agent>> = (0..tool_count)
                .map(|i| {
                    let name = format!("tool_{i}");
                    let agent_name = name.clone();
                    let agent = CustomAgentBuilder::new(name)
                        .description("simulated tool agent")
                        .handler(move |_ctx| {
                            let author = agent_name.clone();
                            async move {
                                let mut event = Event::new("test-invocation");
                                event.author = author;
                                event.llm_response.content = Some(Content {
                                    role: "assistant".to_string(),
                                    parts: vec![Part::Text {
                                        text: "tool result".to_string(),
                                    }],
                                });
                                Ok(Box::pin(stream::iter(vec![Ok(event)])) as EventStream)
                            }
                        })
                        .build()
                        .unwrap();
                    Arc::new(agent) as Arc<dyn Agent>
                })
                .collect();

            // Use the appropriate workflow agent based on strategy
            let orchestrator: Arc<dyn Agent> = match strategy {
                ToolExecutionStrategy::Sequential => {
                    Arc::new(SequentialAgent::new("seq_orchestrator", sub_agents))
                }
                ToolExecutionStrategy::Parallel | ToolExecutionStrategy::Auto => {
                    Arc::new(ParallelAgent::new("par_orchestrator", sub_agents))
                }
            };

            let ctx = Arc::new(TestContext::new("execute tools"));
            let mut event_stream = orchestrator.run(ctx).await.unwrap();

            let mut results = Vec::new();
            while let Some(result) = event_stream.next().await {
                let event = result.unwrap();
                results.push(event);
            }

            // Regardless of strategy, we must get exactly tool_count results
            assert_eq!(
                results.len(),
                tool_count,
                "Strategy {:?} with {} tools produced {} results instead of {}",
                strategy,
                tool_count,
                results.len(),
                tool_count
            );

            // Each result should have a valid author matching a tool agent name
            for (i, event) in results.iter().enumerate() {
                let expected_prefix = "tool_";
                assert!(
                    event.author.starts_with(expected_prefix),
                    "Result {} has unexpected author '{}' (expected prefix '{}')",
                    i,
                    event.author,
                    expected_prefix
                );
            }

            // All tool indices should be represented (no duplicates, no missing)
            let mut seen_indices: Vec<usize> = results
                .iter()
                .filter_map(|e| e.author.strip_prefix("tool_").and_then(|s| s.parse().ok()))
                .collect();
            seen_indices.sort();
            let expected_indices: Vec<usize> = (0..tool_count).collect();
            assert_eq!(
                seen_indices, expected_indices,
                "Strategy {:?}: not all tool indices represented. Got {:?}, expected {:?}",
                strategy, seen_indices, expected_indices
            );
        });
    }
}
