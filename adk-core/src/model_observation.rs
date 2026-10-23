//! Read-only, call-scoped observation of generation transport attempts.
//!
//! The scope must include both generation and consumption of its response stream.
//! Callbacks run synchronously and must not block or perform I/O. Prepared bodies
//! may contain private content: project them immediately rather than logging or
//! retaining them. Request headers are never emitted; callers must treat request
//! body fields as sensitive even when a provider places authorization in JSON.
use serde_json::Value;
use std::{
    collections::BTreeMap,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests;

/// A callback scoped to one logical model invocation, never stored on a client.
pub type Observer = Arc<dyn Fn(Event) + Send + Sync>;

/// A transport attempt identity, unique within the enclosing observation scope.
#[derive(Clone, Debug)]
pub struct Event {
    /// Monotonically increasing, one-based ordinal within this scope.
    pub attempt: u64,
    /// The observed boundary, not an inferred provider stage.
    pub kind: Kind,
}

/// Boundaries observed at the transport owner.
#[derive(Clone, Debug)]
pub enum Kind {
    /// Execution has reached the generation transport.
    Started {
        /// Transport selected for this attempt.
        transport: Transport,
        /// Whether a connection was reused, when known at attempt start.
        reused: Option<bool>,
    },
    /// The actual serialized JSON request after provider customization.
    Prepared {
        /// Adapted request body; project sensitive fields without retaining them.
        body: Value,
    },
    /// A WebSocket connection has been selected for this generation.
    Connection {
        /// Whether this generation uses an existing WebSocket connection.
        reused: bool,
    },
    /// Actual HTTP response or WebSocket upgrade headers have arrived.
    /// A reused socket does not emit fabricated headers for a new generation.
    Headers {
        /// HTTP response status, including a WebSocket upgrade status.
        status: u16,
        /// Monotonic time from attempt start to header arrival.
        elapsed: Duration,
        /// Connection reuse state when the transport can determine it.
        reused: Option<bool>,
    },
    /// The request failed, the enclosing response consumption finished, or it was dropped.
    Finished {
        /// Monotonic time from attempt start to its observed end.
        elapsed: Duration,
        /// Whether the attempt completed, failed, or was dropped.
        outcome: Outcome,
        /// Content-free classification from the error owner, when available.
        /// Successes and cancellations do not carry a failure.
        failure: Option<Failure>,
    },
}

/// The transport used by this attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A generation request carried by HTTP, including streaming responses.
    Http,
    /// A generation request carried by a WebSocket connection.
    WebSocket,
}

/// The observed result, independent of retry eligibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The enclosing response consumption completed successfully.
    Succeeded,
    /// The raw request or enclosing response consumption returned an error.
    Failed,
    /// The observation scope was dropped before completion.
    Cancelled,
}

/// A content-free projection of a typed failure at an observed boundary.
///
/// This deliberately excludes messages, provider codes, arbitrary metadata,
/// request IDs, URLs, headers, and bodies. A later successful retry does not
/// replace the classification of an earlier failed attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    /// ADK's static machine-readable code, never provider-controlled text.
    pub code: &'static str,
    /// The subsystem that produced the error.
    pub component: crate::ErrorComponent,
    /// The typed error category.
    pub category: crate::ErrorCategory,
    /// Actual upstream HTTP status, when present on the typed error.
    pub upstream_status: Option<u16>,
}

impl From<&crate::AdkError> for Failure {
    fn from(error: &crate::AdkError) -> Self {
        Self {
            code: error.code,
            component: error.component,
            category: error.category,
            upstream_status: error.details.upstream_status_code,
        }
    }
}

struct State {
    observer: Observer,
    pending: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    next: u64,
    attempts: BTreeMap<u64, Instant>,
}

tokio::task_local! {
    static CURRENT: Arc<State>;
}

impl State {
    fn emit(&self, attempt: u64, kind: Kind) {
        // An observation failure cannot change model execution or trigger a replay.
        let _ = catch_unwind(AssertUnwindSafe(|| (self.observer)(Event { attempt, kind })));
    }

    fn finish(&self, id: u64, outcome: Outcome, failure: Option<Failure>) {
        let start = self.pending.lock().unwrap().attempts.remove(&id);
        if let Some(start) = start {
            self.emit(id, Kind::Finished { elapsed: start.elapsed(), outcome, failure });
        }
    }

    fn finish_all(&self, outcome: Outcome) {
        let attempts = std::mem::take(&mut self.pending.lock().unwrap().attempts);
        for (id, start) in attempts {
            self.emit(id, Kind::Finished { elapsed: start.elapsed(), outcome, failure: None });
        }
    }
}

struct Guard(Arc<State>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.finish_all(Outcome::Cancelled);
    }
}

/// Observes an entire generation, including stream consumption, in the current task.
///
/// Nested scopes are isolated. Unscoped calls emit nothing. Spawned tasks do not
/// inherit Tokio task-local state; providers must keep generation transport within
/// the scoped future or explicitly propagate its observation context.
///
/// ```
/// use adk_core::model_observation::{scope, Observer};
/// use std::sync::Arc;
/// # async fn example() -> Result<(), ()> {
/// let observer: Observer = Arc::new(|event| {
///     // Project counters/timings here; never log entire prepared bodies.
///     let _ = event.attempt;
/// });
/// scope(observer, async {
///     // Generate and consume the response stream within this future.
///     Ok::<_, ()>(())
/// }).await
/// # }
/// ```
pub async fn scope<T, E>(
    observer: Observer,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let state = Arc::new(State { observer, pending: Mutex::new(Pending::default()) });
    let guard = Guard(state.clone());
    let result = CURRENT.scope(state, future).await;
    // Only the protocol/response owner knows whether its own attempt completed.
    // Unfinished attempts remain cancellations even if a later retry succeeded.
    drop(guard);
    result
}

/// A lightweight handle used by generation transports. Cloning does not create an attempt.
#[derive(Clone, Default)]
pub struct Attempt(Option<(Arc<State>, u64, Instant)>);

impl Attempt {
    /// Starts an attempt only when the caller installed an observation scope.
    pub fn start(transport: Transport, reused: Option<bool>) -> Self {
        let Ok(state) = CURRENT.try_with(Arc::clone) else { return Self::default() };
        let start = Instant::now();
        let id = {
            let mut pending = state.pending.lock().unwrap();
            pending.next += 1;
            let id = pending.next;
            pending.attempts.insert(id, start);
            id
        };
        state.emit(id, Kind::Started { transport, reused });
        Self(Some((state, id, start)))
    }

    /// Observes buffered JSON without changing it; unavailable/non-JSON bodies emit nothing.
    pub fn prepared(&self, bytes: Option<&[u8]>) {
        if let Some((state, id, _)) = &self.0
            && let Some(body) = bytes.and_then(|bytes| serde_json::from_slice(bytes).ok())
        {
            state.emit(*id, Kind::Prepared { body });
        }
    }

    /// Observes an already prepared JSON frame without serializing it again.
    pub fn prepared_json(&self, body: &Value) {
        if let Some((state, id, _)) = &self.0 {
            state.emit(*id, Kind::Prepared { body: body.clone() });
        }
    }

    /// Reports actual connection selection, separately from handshake headers.
    pub fn connection(&self, reused: bool) {
        if let Some((state, id, _)) = &self.0 {
            state.emit(*id, Kind::Connection { reused });
        }
    }

    /// Records arrival of actual response headers before status/body processing.
    pub fn headers(&self, status: u16, reused: Option<bool>) {
        if let Some((state, id, start)) = &self.0 {
            state.emit(*id, Kind::Headers { status, elapsed: start.elapsed(), reused });
        }
    }

    /// Ends this attempt at its own response boundary. Repeated finishes are ignored.
    pub fn finish(&self, outcome: Outcome) {
        if let Some((state, id, _)) = &self.0 {
            state.finish(*id, outcome, None);
        }
    }

    /// Ends this attempt with only the error's fixed, structured classification.
    pub fn fail(&self, error: &crate::AdkError) {
        self.finish_failure(Failure::from(error));
    }

    /// Records a typed raw transport failure before an SDK may retry or replace it.
    pub fn finish_failure(&self, failure: Failure) {
        if let Some((state, id, _)) = &self.0 {
            state.finish(*id, Outcome::Failed, Some(failure));
        }
    }
}

impl Attempt {
    /// Keeps this attempt alive until its owner finishes it or drops the guard.
    ///
    /// Move the guard with the response/decoder, not into a client or session cache.
    /// A guard dropped before an explicit finish reports cancellation.
    pub fn guard(&self) -> AttemptGuard {
        AttemptGuard(self.clone())
    }

    /// Returns the latest active attempt in this task's observation scope.
    ///
    /// Providers use this handle when a validated request completes before the
    /// generation, such as a background submission. Unscoped calls return a no-op
    /// handle. Concurrent invocations must use separate observation scopes.
    pub fn current() -> Self {
        CURRENT
            .try_with(|state| {
                state
                    .pending
                    .lock()
                    .unwrap()
                    .attempts
                    .last_key_value()
                    .map(|(&id, &start)| Self(Some((state.clone(), id, start))))
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    fn active(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|(state, id, _)| state.pending.lock().unwrap().attempts.contains_key(id))
    }
}

/// Non-cloneable lifetime guard for one raw transport attempt.
pub struct AttemptGuard(Attempt);

impl AttemptGuard {
    /// Records the response owner's terminal outcome, at most once.
    pub fn finish(&self, outcome: Outcome) {
        self.0.finish(outcome);
    }

    /// Records the error owner's content-free classification, at most once.
    pub fn fail(&self, error: &crate::AdkError) {
        self.0.fail(error);
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        self.finish(Outcome::Cancelled);
    }
}

struct Operation {
    state: Option<Arc<State>>,
    after: u64,
}
impl Operation {
    fn finish(&mut self, outcome: Outcome, failure: Option<Failure>) {
        if let Some(state) = self.state.take() {
            let attempts =
                state.pending.lock().unwrap().attempts.split_off(&self.after.saturating_add(1));
            for (id, start) in attempts {
                state.emit(id, Kind::Finished { elapsed: start.elapsed(), outcome, failure });
            }
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.finish(Outcome::Cancelled, None);
    }
}

/// Observes an individual request/parse operation before its caller may retry it.
///
/// An error finishes only attempts started by this operation, before any retry
/// backoff. Success leaves response consumption to [`observe_stream`] or an
/// explicit [`AttemptGuard`]. Cancellation finishes this operation's attempts.
/// Use one scope per logical invocation; do not share it between concurrent calls.
pub async fn operation<T, E>(future: impl Future<Output = Result<T, E>>) -> Result<T, E> {
    operation_inner(future, |_| None).await
}

/// Observes an operation with a content-free classification supplied by its error owner.
///
/// Like [`operation`], this finishes failures before retry backoff and preserves
/// cancellation semantics. The classifier must use typed errors rather than
/// parsing display text or returning provider-controlled fields.
pub async fn operation_with_failure<T, E>(
    future: impl Future<Output = Result<T, E>>,
    classify: impl FnOnce(&E) -> Failure,
) -> Result<T, E> {
    operation_inner(future, |error| Some(classify(error))).await
}

async fn operation_inner<T, E>(
    future: impl Future<Output = Result<T, E>>,
    classify: impl FnOnce(&E) -> Option<Failure>,
) -> Result<T, E> {
    let state = CURRENT.try_with(Arc::clone).ok();
    let after = state.as_ref().map_or(0, |state| state.pending.lock().unwrap().next);
    let mut guard = Operation { state, after };
    let result = future.await;
    if let Err(error) = &result {
        guard.finish(Outcome::Failed, classify(error));
    } else {
        guard.state = None;
    }
    result
}

/// Finishes a model stream's attempt at its semantic terminal, error, or drop.
///
/// HTTP body EOF alone is not success. The provider parser must emit a complete
/// response; error responses and EOF without a terminal finish as failures.
/// This wrapper does not alter results, retry requests, or retain their content.
pub fn observe_stream(stream: crate::LlmResponseStream) -> crate::LlmResponseStream {
    Box::pin(ObservedStream { stream, guard: Attempt::current().guard() })
}

struct ObservedStream {
    stream: crate::LlmResponseStream,
    guard: AttemptGuard,
}
impl futures::Stream for ObservedStream {
    type Item = Result<crate::LlmResponse, crate::AdkError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let result = self.stream.as_mut().poll_next(cx);
        // Some providers start the raw request lazily on the first stream poll.
        // Failed retries have already finished at their own operation boundary.
        if !self.guard.0.active() {
            self.guard = Attempt::current().guard();
        }
        match &result {
            std::task::Poll::Ready(Some(Err(error))) => self.guard.fail(error),
            std::task::Poll::Ready(Some(Ok(response))) => {
                if response.error_code.is_some() || response.error_message.is_some() {
                    self.guard.finish(Outcome::Failed);
                } else if response.turn_complete
                    || (!response.partial && response.finish_reason.is_some())
                {
                    self.guard.finish(Outcome::Succeeded);
                }
            }
            std::task::Poll::Ready(None) => self.guard.finish(Outcome::Failed),
            std::task::Poll::Pending => {}
        }
        result
    }
}
