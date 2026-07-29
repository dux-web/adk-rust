//! `SandboxRunner::run` must actually run the agent.
//!
//! It used to provision the workspace, start a session, bind tools, and then execute
//! `Ok::<(), AdkError>(())` — a comment read "For now, we simulate the agent loop step as a
//! placeholder." Provisioning succeeded, so the call returned `Ok` and reported a completed
//! sandbox run in which no agent had ever executed.
#![cfg(feature = "sandbox-runner")]

use adk_core::{Agent, Content, EventStream, InvocationContext, Result as AdkResult};
use adk_runner::Runner;
use adk_runner::sandbox_runner::SandboxRunner;
use adk_sandbox::SandboxError;
use adk_sandbox::workspace::{
    Capability, DirEntry, ExecOutput, Manifest, SandboxClient, SandboxConfig, SandboxSession,
    SessionHandle, SnapshotId,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Records whether the agent was invoked, and what tools it could see.
struct RecordingAgent {
    runs: Arc<AtomicUsize>,
    tools_seen: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent for RecordingAgent {
    fn name(&self) -> &str {
        "recording_agent"
    }

    fn description(&self) -> &str {
        "records that it ran"
    }

    fn sub_agents(&self) -> &[Arc<dyn Agent>] {
        &[]
    }

    async fn run(&self, ctx: Arc<dyn InvocationContext>) -> AdkResult<EventStream> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let mut count = 0usize;
        for runtime in &ctx.run_config().runtime_toolsets {
            count += runtime.toolset().tools(ctx.clone()).await?.len();
        }
        self.tools_seen.store(count, Ordering::SeqCst);
        Ok(Box::pin(futures::stream::empty()))
    }
}

struct FakeSession;

#[async_trait]
impl SandboxSession for FakeSession {
    async fn exec_command(
        &self,
        _command: &str,
        _working_dir: Option<&str>,
    ) -> Result<ExecOutput, SandboxError> {
        Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            duration: Duration::from_millis(1),
            timed_out: false,
        })
    }

    async fn read_file(&self, _path: &str) -> Result<Vec<u8>, SandboxError> {
        Ok(Vec::new())
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> Result<(), SandboxError> {
        Ok(())
    }

    async fn list_dir(&self, _path: &str) -> Result<Vec<DirEntry>, SandboxError> {
        Ok(Vec::new())
    }

    async fn apply_patch(&self, _patch: &str) -> Result<(), SandboxError> {
        Ok(())
    }
}

/// Counts lifecycle calls so cleanup can be asserted.
struct FakeClient {
    stops: Arc<AtomicUsize>,
}

#[async_trait]
impl SandboxClient for FakeClient {
    async fn provision(&self, _manifest: &Manifest) -> Result<SessionHandle, SandboxError> {
        Ok(SessionHandle("fake-handle".to_string()))
    }

    async fn start(
        &self,
        _handle: &SessionHandle,
    ) -> Result<Box<dyn SandboxSession>, SandboxError> {
        Ok(Box::new(FakeSession))
    }

    async fn stop(&self, _handle: &SessionHandle) -> Result<(), SandboxError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn snapshot(&self, _handle: &SessionHandle) -> Result<SnapshotId, SandboxError> {
        Ok(SnapshotId("fake-snapshot".to_string()))
    }

    async fn resume(&self, _snapshot_id: &SnapshotId) -> Result<SessionHandle, SandboxError> {
        Ok(SessionHandle("fake-handle".to_string()))
    }
}

#[tokio::test]
async fn run_invokes_the_inner_agent() {
    let runs = Arc::new(AtomicUsize::new(0));
    let tools_seen = Arc::new(AtomicUsize::new(0));
    let stops = Arc::new(AtomicUsize::new(0));

    let agent = Arc::new(RecordingAgent { runs: runs.clone(), tools_seen: tools_seen.clone() });
    let runner = Runner::builder()
        .app_name("sandbox-runner-test")
        .agent(agent)
        .session_service(Arc::new(adk_session::InMemorySessionService::new()))
        .build()
        .expect("runner builds");

    let config = SandboxConfig {
        client: Arc::new(FakeClient { stops: stops.clone() }),
        manifest: Manifest::new(Vec::new()),
        capabilities: [Capability::Shell, Capability::Filesystem].into_iter().collect(),
        command_timeout: Duration::from_secs(5),
        session_timeout: Duration::from_secs(30),
        snapshot_on_stop: false,
    };

    let sandbox = SandboxRunner::new(runner);
    let content = Content::new("user").with_text("do the thing");
    sandbox.run(&config, "user-1", "session-1", content).await.expect("the sandbox run succeeds");

    assert_eq!(runs.load(Ordering::SeqCst), 1, "the agent must actually be invoked");
    assert!(
        tools_seen.load(Ordering::SeqCst) >= 2,
        "the agent must see the tools bound to the live session, saw {}",
        tools_seen.load(Ordering::SeqCst)
    );
    assert_eq!(stops.load(Ordering::SeqCst), 1, "the session must be stopped exactly once");
}
