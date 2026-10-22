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

    fn finish(&self, id: u64, outcome: Outcome) {
        let start = self.pending.lock().unwrap().attempts.remove(&id);
        if let Some(start) = start {
            self.emit(id, Kind::Finished { elapsed: start.elapsed(), outcome });
        }
    }

    fn finish_all(&self, outcome: Outcome) {
        let attempts = std::mem::take(&mut self.pending.lock().unwrap().attempts);
        for (id, start) in attempts {
            self.emit(id, Kind::Finished { elapsed: start.elapsed(), outcome });
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
    guard.0.finish_all(if result.is_ok() { Outcome::Succeeded } else { Outcome::Failed });
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

    /// Ends a failed raw send before another attempt. Repeated finishes are ignored.
    /// Successful streams remain pending until the enclosing scope finishes.
    pub fn finish(&self, outcome: Outcome) {
        if let Some((state, id, _)) = &self.0 {
            state.finish(*id, outcome);
        }
    }
}
