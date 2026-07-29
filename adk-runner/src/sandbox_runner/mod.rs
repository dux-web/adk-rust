//! Sandbox runner lifecycle management.
//!
//! This module provides [`SandboxRunner`], a wrapper around the standard [`Runner`](crate::Runner)
//! that manages the full sandbox lifecycle: provision → start → bind tools → run → stop → snapshot.
//!
//! # Overview
//!
//! The `SandboxRunner` extracts a [`SandboxConfig`](adk_sandbox::workspace::SandboxConfig) from
//! the agent, provisions a workspace, binds shell and filesystem tools based on enabled
//! capabilities, delegates execution to the inner runner, and guarantees cleanup (stop) even
//! on failure.
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_runner::sandbox_runner::SandboxRunner;
//! use adk_runner::Runner;
//! use adk_sandbox::workspace::SandboxConfig;
//!
//! let runner = Runner::new(config)?;
//! let sandbox_runner = SandboxRunner::new(runner);
//! let content = adk_core::Content::new("user").with_text("list the files");
//! let result = sandbox_runner
//!     .run(&sandbox_config, "user_1", "session_1", content)
//!     .await?;
//! ```

pub mod binding;
pub mod tools;

use crate::Runner;
use adk_sandbox::SandboxError;
use adk_sandbox::workspace::{SandboxConfig, SnapshotId};
use std::sync::Arc;

use futures::StreamExt;
use tracing::{info, warn};

/// Exposes the tools bound to a live sandbox session as a [`Toolset`].
///
/// The tools hold the session handle, so they are valid only for the run that created them and
/// are injected per-invocation rather than attached to the agent.
struct SandboxToolset {
    tools: Vec<Arc<dyn adk_core::Tool>>,
}

#[async_trait::async_trait]
impl adk_core::Toolset for SandboxToolset {
    fn name(&self) -> &str {
        "sandbox"
    }

    async fn tools(
        &self,
        _ctx: Arc<dyn adk_core::ReadonlyContext>,
    ) -> adk_core::Result<Vec<Arc<dyn adk_core::Tool>>> {
        Ok(self.tools.clone())
    }
}

/// Runner wrapper that manages the sandbox lifecycle around agent execution.
///
/// Provisions the workspace, binds tools, delegates to the inner Runner,
/// and cleans up (stop + optional snapshot) on completion or failure.
pub struct SandboxRunner {
    inner: Runner,
}

impl SandboxRunner {
    /// Creates a new `SandboxRunner` wrapping the given [`Runner`].
    pub fn new(inner: Runner) -> Self {
        Self { inner }
    }

    /// Returns a reference to the inner [`Runner`].
    pub fn inner(&self) -> &Runner {
        &self.inner
    }

    /// Runs the agent with full sandbox lifecycle management.
    ///
    /// Manages the complete sandbox lifecycle:
    /// 1. Provisions workspace from the config's manifest
    /// 2. Starts the sandbox session
    /// 3. Binds tools based on enabled capabilities
    /// 4. Runs the agent loop via the inner Runner
    /// 5. Stops the session (always, even on failure)
    /// 6. Optionally snapshots the workspace
    ///
    /// # Stop Guarantee
    ///
    /// The `stop` method is **always** called on the sandbox client, regardless
    /// of whether the agent loop succeeds or fails. This ensures resources are
    /// cleaned up even in error scenarios.
    ///
    /// # Errors
    ///
    /// Returns an error if provisioning or starting the session fails (without
    /// entering the agent loop), or if the agent loop itself fails (after
    /// cleanup has been performed).
    pub async fn run(
        &self,
        config: &SandboxConfig,
        user_id: &str,
        session_id: &str,
        user_content: adk_core::Content,
    ) -> Result<SandboxRunResult, adk_core::AdkError> {
        // 1. Provision workspace from manifest
        info!("provisioning sandbox workspace");
        let handle =
            config.client.provision(&config.manifest).await.map_err(adk_core::AdkError::from)?;

        // 2. Start session
        info!(session_handle = %handle.0, "starting sandbox session");
        let session = match config.client.start(&handle).await {
            Ok(s) => s,
            Err(e) => {
                // If start fails, attempt to stop/cleanup the provisioned session
                let _ = config.client.stop(&handle).await;
                return Err(adk_core::AdkError::from(e));
            }
        };

        // 3. Bind tools based on capabilities
        let session_arc = Arc::from(session);
        let bound_tools =
            binding::bind_tools(session_arc, &config.capabilities, config.command_timeout);
        info!(
            capabilities = ?config.capabilities,
            tool_count = bound_tools.len(),
            "bound sandbox tools"
        );

        // 4. Run the agent loop with the sandbox tools injected, under the session timeout.
        //
        // The tools exist only while this session is live, so they are supplied per-invocation
        // through `runtime_toolsets` rather than baked into the agent.
        let mut run_config = self.inner.run_config().clone();
        run_config
            .runtime_toolsets
            .push(adk_core::RuntimeToolset::new(Arc::new(SandboxToolset { tools: bound_tools })));

        let agent_loop_future = async {
            // The Runner requires the session to exist. Create it when absent so a caller can
            // hand in a fresh session ID, matching how the A2A handler resolves sessions.
            let session_service = self.inner.session_service();
            if session_service
                .get(adk_session::GetRequest {
                    app_name: self.inner.app_name().to_string(),
                    user_id: user_id.to_string(),
                    session_id: session_id.to_string(),
                    num_recent_events: None,
                    after: None,
                })
                .await
                .is_err()
            {
                session_service
                    .create(adk_session::CreateRequest {
                        app_name: self.inner.app_name().to_string(),
                        user_id: user_id.to_string(),
                        session_id: Some(session_id.to_string()),
                        state: std::collections::HashMap::new(),
                    })
                    .await?;
            }

            let user_id = adk_core::UserId::new(user_id)?;
            let session_id = adk_core::SessionId::new(session_id)?;
            let mut events = self
                .inner
                .run_with_config(user_id, session_id, user_content, Some(run_config))
                .await?;
            // Drain the stream so the agent runs to completion before the session is stopped;
            // returning early would tear the sandbox down underneath the agent.
            let mut count = 0usize;
            while let Some(event) = events.next().await {
                event?;
                count += 1;
            }
            Ok::<usize, adk_core::AdkError>(count)
        };

        let agent_loop_result =
            tokio::time::timeout(config.session_timeout, agent_loop_future).await;

        // Convert timeout to SandboxError::SessionTimeout
        let agent_loop_result = match agent_loop_result {
            Ok(result) => result,
            Err(_elapsed) => {
                warn!(
                    session_handle = %handle.0,
                    timeout = ?config.session_timeout,
                    "sandbox session timed out"
                );
                Err::<usize, adk_core::AdkError>(adk_core::AdkError::from(
                    SandboxError::SessionTimeout { timeout: config.session_timeout },
                ))
            }
        };

        // 5. Stop session — ALWAYS called, regardless of agent loop outcome
        info!(session_handle = %handle.0, "stopping sandbox session");
        if let Err(e) = config.client.stop(&handle).await {
            warn!(
                session_handle = %handle.0,
                error = %e,
                "failed to stop sandbox session during cleanup"
            );
        }

        // 6. Handle agent loop result — propagate error after cleanup
        agent_loop_result?;

        // 7. Optionally snapshot
        let snapshot_id = if config.snapshot_on_stop {
            info!(session_handle = %handle.0, "snapshotting sandbox workspace");
            match config.client.snapshot(&handle).await {
                Ok(id) => {
                    info!(snapshot_id = %id.0, "sandbox snapshot created");
                    Some(id)
                }
                Err(e) => {
                    warn!(
                        session_handle = %handle.0,
                        error = %e,
                        "sandbox snapshot failed, continuing without snapshot"
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(SandboxRunResult { snapshot_id })
    }
}

/// Result of a sandbox-managed agent run.
#[derive(Debug)]
pub struct SandboxRunResult {
    /// The snapshot ID if snapshot-on-stop was enabled.
    pub snapshot_id: Option<SnapshotId>,
}
