# Spec: Adopting Monty 0.0.19 Capabilities in the CodeAct Runtime

| | |
|---|---|
| **Status** | Draft / RFC — requirements + design + tasks, pending approval |
| **Depends on** | #525 (`adk-codeact-monty` joins the workspace on crates.io monty 0.0.19) |
| **Affected crates** | `adk-agent` (`codeact` feature), `adk-codeact-monty`; optional touchpoints in `adk-devtools`, `adk-runner` |
| **Related** | #380 (Code Agents), `docs/design/coding-agent.md`, `docs/official_docs/agents/code-agent.md` |

---

## 1. Summary

Monty 0.0.19 exposes a substantially larger surface than the one-shot `MontyRun`
we consume today: a **stateful, serializable REPL**, **structured tracebacks**,
a **mount-based virtual filesystem**, **concurrent future resolution**, **lazy
name lookup**, **per-stream print capture**, and **tracker-installed
cancellation**. Seven of those are unreferenced in `adk-codeact-monty`.

The blocker is not the runtime — it is our own abstraction. `CodeRuntime` models
**one script execution**, so a persistent interpreter cannot be expressed even
though Monty now supports one. This spec adds a session-scoped seam to
`CodeRuntime` (defaulted, so existing runtimes are unaffected) and then adopts the
capabilities that seam unlocks.

## 2. What Monty 0.0.19 actually provides

Verified against the vendored sources at
`~/.cargo/registry/src/*/monty{,-types,-fs}-0.0.19`:

| Capability | API | Verified detail |
|---|---|---|
| **Stateful REPL** | `MontyRepl::new/feed_start/feed`, `ReplProgress`, `ReplStartError` | "Preserves heap and global variable state between snippets… avoiding the cost and semantic risks of replaying prior code." `feed_start` suspends on external/OS calls and futures exactly like `MontyRun::start`. |
| **Session serialization** | `MontyRepl::dump/load` | `#[derive(Serialize, Deserialize)]` on the whole struct + `postcard::to_allocvec(self)` — so heap, globals, slot map **and `snippet_sources`** all persist. Explicitly "between process runs". |
| **State survives failure** | `ReplStartError { repl, error }` | "On a Python-level runtime exception the REPL is **not** destroyed" — a failed snippet keeps the namespace. |
| **Structured tracebacks** | `MontyException::traceback() -> &[StackFrame]` | `StackFrame { filename, start: CodeLoc, end: CodeLoc, frame_name, source-line preview }`, `CodeLoc { line, column }`. Cross-snippet frames are resolvable because the REPL retains snippet sources. |
| **Virtual filesystem** | `monty_fs::{MountTable, Mount, MountMode, MountCallOutcome, OverlayState}` | `Mount { virtual_path, host_path, mode }` with `MountMode::{ReadOnly, ReadWrite}`; `MountTable::handle_os_call` routes guest OS calls. `OverlayState` provides copy-on-write staging. |
| **Concurrent futures** | `RunProgress::ResolveFutures` / `ReplResolveFutures` | Snapshot carries "the pending `call_ids` that this snapshot is waiting on" — i.e. several outstanding calls at once. |
| **Lazy name binding** | `RunProgress::NameLookup` | `{ name, namespace slot, global-vs-local }` — the host resolves a name on first use and the value is cached in the slot. |
| **Per-stream print** | `PrintWriter::collect_streams(&mut Vec<(PrintStream, String)>)`, `PrintWriterCallback` | Preserves stdout/stderr identity **and interleaving order**; callback form allows live streaming. |
| **Cancellation** | `MontyRepl::tracker_mut()` | "REPL hosts use this to install ephemeral execution controls, such as async cancellation flags, before calling `feed_start()`." |

Currently used: `MontyRun`, `RunProgress::{Complete,FunctionCall,OsCall,NameLookup,ResolveFutures}`,
`MontyObject`, `DictPairs`, `ResourceLimits`, `MountTable`, `MountMode`,
`MountCallOutcome`, `DEFAULT_MAX_PRINT_COLLECT_BYTES`.

Unused: `MontyRepl`, `ReplProgress`, `detect_repl_continuation_mode`,
`OverlayState`, `StackFrame`, `CodeLoc`, `AssertMessageAnnotations`,
`PrintWriterCallback`, `MontyFileHandle`.

## 3. Where today's architecture blocks adoption

`adk-agent::codeact::CodeRuntime`:

```rust
fn start(&self, script: &str, script_name: &str) -> Result<RunStep, RuntimeError>;
fn resume(&self, snapshot: &[u8], with: ResumeWith) -> Result<RunStep, RuntimeError>;
```

- `start` takes a **script**, not a session. There is no "same interpreter, next
  snippet" concept, so `MontyRepl` has nowhere to live.
- `resume(snapshot, …)` already threads opaque bytes, but only for a **paused
  run inside one script** — a genuinely good durable-continuation design we keep.
- `RunStep` has no channel to hand *updated session state* back to the caller.
- `adk-codeact-monty` documents this honestly: "**No shared state** … entirely
  stateless", and builds "a fresh `MountTable`" per run.

So state persists *within* a script across tool-call pauses, but **not across
steps**. Canonical CodeAct carries a namespace across steps; the original design
doc listed "fresh vs persistent interpreter state" as an open question. Monty now
answers it, and this is the decision this spec makes.

## 4. Goals / Non-goals

**Goals**

- **G1** Let a `CodeRuntime` optionally be **session-scoped**, without breaking
  the existing one-shot contract or any current implementor.
- **G2** Give the model **structured, CPython-style tracebacks** for failed scripts.
- **G3** Expose a **workspace as a mounted virtual filesystem** with explicit
  read-only/read-write intent, composable with `adk-devtools::Workspace`.
- **G4** Dispatch **concurrent tool calls** from a script through ADK's existing
  concurrency policy rather than a second mechanism.
- **G5** Support **lazy tool binding**, so a large toolset need not be rendered
  into every prompt.
- **G6** Preserve stdout/stderr **identity and ordering**, and allow streaming.
- **G7** Make a long-running script **cancellable** via ADK's existing token.

**Non-goals**

- **N1** Publishing `adk-codeact-monty`. It must stay `publish = false`: the build
  only resolves because the **workspace `Cargo.lock` pins `get-size2` 0.10.1**, and
  published libraries ship no lockfile, so a downstream consumer would resolve
  `0.10.2+` and fail `ruff_python_ast 0.0.3`'s derive. Recording this reason is a
  task; changing it is not.
- **N2** Re-adding `max_allocations` — `ResourceLimits` no longer counts them and a
  silently unenforced cap misrepresents the sandbox.
- **N3** Changing the durable mid-script continuation model, which already works.
- **N4** Making sessions the default in this change (see R1 acceptance criteria).

## 5. Requirements

**R1 — Session-scoped execution (opt-in).**
As an agent author, I can configure a CodeAct agent so successive steps share one
Python namespace.
*Acceptance:* a script defining `x = 1` in step 1 can read `x` in step 2 when
sessions are enabled; with sessions disabled (**the default**), step 2 raises
`NameError`. A runtime that does not implement sessions keeps working unchanged
and reports `supports_sessions() == false`.

**R2 — Session state survives a failed step.**
*Acceptance:* if step 2 raises, variables from step 1 are still visible in step 3.

**R3 — Session state survives process restart.**
*Acceptance:* session bytes round-trip through `SessionService` state; after a
simulated restart, a prior variable is still readable. A configured size budget is
enforced, and exceeding it degrades to a fresh session with a warning rather than
failing the turn.

**R4 — Structured tracebacks.**
*Acceptance:* a failing script yields an error containing ordered frames with
`filename`, `line`, `column`, optional `frame_name`, and the source line; the
rendered text a model sees names the failing line. Runtimes without traceback
support still return today's flat message.

**R5 — Mounted workspace.**
*Acceptance:* a script can read a file under a `ReadOnly` mount and is denied
writing to it; it can write under a `ReadWrite` mount. Paths outside every mount
are denied. Mount configuration is explicit — no implicit host-filesystem access.

**R6 — Concurrent tool calls from a script.**
*Acceptance:* a script awaiting two independent tool calls dispatches them under
the agent's configured `ToolExecutionStrategy`, and results bind to the correct
call ids regardless of completion order.

**R7 — Lazy tool binding.**
*Acceptance:* with lazy binding enabled, a tool not referenced by a script is
never resolved, and the rendered preamble need not enumerate every tool; a
referenced-but-unknown name produces the existing unknown-tool message.

**R8 — Ordered stdout/stderr.**
*Acceptance:* a script interleaving `print()` and `print(file=sys.stderr)` yields
output preserving both stream identity and relative order.

**R9 — Cancellation.**
*Acceptance:* cancelling the invocation stops a long-running script promptly and
surfaces a cancellation outcome, not a timeout.

## 6. Design

### D1 — The session seam (R1, R2, R3)

Add to `adk-agent::codeact`:

```rust
/// Opaque, runtime-owned interpreter state that survives between scripts.
#[derive(Clone, Debug)]
pub struct CodeSessionState(Vec<u8>);

pub trait CodeRuntime: Send + Sync {
    // … existing start() / resume() / capabilities() / render_tools() unchanged …

    /// Whether this runtime can carry a namespace across scripts.
    fn supports_sessions(&self) -> bool { false }

    /// Execute `script` against prior session state, if any.
    ///
    /// Defaulted to ignore `state` and delegate to `start`, so every existing
    /// implementor remains correct and stateless.
    fn start_in_session(
        &self,
        state: Option<&CodeSessionState>,
        script: &str,
        script_name: &str,
    ) -> Result<RunStep, RuntimeError> {
        let _ = state;
        self.start(script, script_name)
    }
}
```

`RunStep` gains an out-channel, set on **completion and on raise** (R2):

```rust
impl RunStep {
    pub fn with_session(mut self, state: CodeSessionState) -> Self { … }
    pub fn session(&self) -> Option<&CodeSessionState> { … }
}
```

**Monty mapping.** `MontyRuntime::start_in_session`:
`MontyRepl::load(state)` (or `MontyRepl::new`) → `feed_start(script)` →
match `ReplProgress`:

| `ReplProgress` | `RunStep` |
|---|---|
| `Complete` → `into_complete() -> (MontyRepl, MontyObject)` | `complete(value).with_session(repl.dump())` |
| `FunctionCall` / `OsCall` / `ResolveFutures` / `NameLookup` | existing pause path — `ReplProgress::dump()` already contains the REPL |
| `Err(ReplStartError { repl, error })` | `raised(render(error)).with_session(repl.dump())` — **R2** |

The mid-script pause snapshot and the cross-script session snapshot are therefore
*the same kind of thing*, which is why no second durability mechanism is needed.

**Persistence (R3).** `CodeActAgent` stores session bytes in session state under a
new `SESSION_STATE_KEY`, beside the existing `PENDING_STATE_KEY`. Because a heap
dump is unbounded in principle and `SessionService` may be SQLite-backed, the
agent enforces `max_session_state_bytes` (default to be chosen from measurement,
see Q1): over budget → drop the session, log a warning, next step starts fresh.
Sessions are **opt-in** (`CodeActAgentBuilder::persistent_session(true)`) so the
default cost profile is unchanged (**N4**).

### D2 — Structured tracebacks (R4)

Runtime-agnostic types in `adk-agent::codeact`:

```rust
pub struct ScriptFrame {
    pub filename: String,
    pub line: u32,
    pub column: u32,
    pub frame_name: Option<String>,
    pub source_line: Option<String>,
}
pub struct ScriptError { pub message: String, pub frames: Vec<ScriptFrame> }
```

`RunStep::raised` keeps taking a message; add `raised_with(ScriptError)`.
`error_map` renders frames into the CPython-style block the model sees. The monty
side is a direct projection of `MontyException::traceback()`. Advertise via
`RuntimeCapabilities::structured_errors`, so a runtime without frames degrades to
the current message.

### D3 — Mounted workspace (R5)

Runtime-agnostic mount description on the builder, so the trait stays
Monty-free:

```rust
pub enum MountAccess { ReadOnly, ReadWrite }
pub struct WorkspaceMount { pub guest_path: String, pub host_path: PathBuf, pub access: MountAccess }
```

`MontyRuntimeBuilder::mount(WorkspaceMount)` builds the `MountTable` once per
runtime (not per run) and hands it to `OsAccess`. Default remains **no mounts**.

This is the natural join with the coding agent: an `adk_devtools::Workspace` root
becomes a `WorkspaceMount`, so `bash`/`read_file` and the Python sandbox address
the same tree with the same intent. `OverlayState` is a follow-up (Q3): stage
writes, then review or discard — attractive for an agent that should not mutate a
repo in place.

### D4 — Concurrent tool calls (R6)

`ResolveFutures` carries the set of pending `call_ids`. Map it to a `RunStep`
variant carrying **many** `PendingCall`s, and dispatch them through the agent's
existing `ToolExecutionStrategy` and `ToolConcurrencyManager` — the same policy
that governs JSON tool calls, not a parallel one. Resume binds results by
`call_id`, so completion order is irrelevant.

### D5 — Lazy tool binding (R7)

`NameLookup` gives `{ name, slot, global? }`. The agent resolves the name against
its toolset on first use and returns the binding; unknown names reuse
`unknown_tool_message`. `render_tools` then only needs a **summary** rather than
full signatures for every tool, which is the same lazy philosophy as
`adk-skill`. Gated by `RuntimeCapabilities::lazy_names` and a builder flag,
because it changes what the model is told upfront.

### D6 — Ordered stdout/stderr (R8)

Replace the single collected `String` with
`PrintWriter::collect_streams(&mut Vec<(PrintStream, String)>)`, and widen
`ScriptOutput` to an ordered `Vec<(OutputStream, String)>` while keeping the
existing flat accessor for compatibility. `PrintWriterCallback` enables live
streaming as a follow-up (Q4).

### D7 — Cancellation (R9)

Before `feed_start`, install a cancellation flag via `repl.tracker_mut()`, driven
by the invocation's `CancellationToken`. Surface a distinct cancellation outcome so
it is not reported as a timeout.

### Error handling

Three failure classes stay distinct: **Python-level raise** (R4 traceback, session
preserved), **resource limit** (time/memory from `ResourceLimits`), and
**host/dispatch error** (`RuntimeError`). Only the first is script-visible.

### Testing

- Unit, `adk-agent`: the defaulted seam — a stateless test runtime must be
  unaffected and report `supports_sessions() == false`.
- Unit, `adk-codeact-monty`: `x = 1` then `print(x)` across two `start_in_session`
  calls; failure-then-read (R2); dump→load→read across a fresh runtime (R3);
  traceback frame assertions (R4); mount allow/deny matrix (R5); two-call
  concurrent resume with reversed completion order (R6); lazy lookup of an
  unreferenced tool never resolving (R7); interleaved stream ordering (R8).
- Integration: a `CodeActAgent` over the monty runtime with sessions enabled,
  asserting namespace continuity through the real agent loop.
- A **negative control** for R3: corrupt/oversize state must degrade to a fresh
  session, not fail the turn.

## 7. Risks and open questions

- **Q1 — Session size.** A heap dump's realistic size is unmeasured. Needs a
  measurement pass (typical agent script, 10 steps) before fixing the default
  budget. If large, options are compression, or storing session bytes in an
  artifact rather than session state.
- **Q2 — Tracker serialization.** `MontyRepl::dump` requires
  `T: ResourceTracker + Serialize`. Confirm `LimitedTracker` satisfies it and that
  a restored tracker's counters start from a sane state (a restored session should
  not inherit an exhausted time budget).
- **Q3 — `OverlayState` semantics.** Deferred; needs its own read of the
  copy-on-write model before promising review-then-commit behavior.
- **Q4 — Streaming print.** `PrintWriterCallback` interacts with the borrow of the
  output buffer during a suspendable run; deferred behind D6.
- **Q5 — Monty is `0.0.x`.** Every release is potentially breaking, and the
  `get-size2` pin is load-bearing. Sessions increase our coupling to Monty's
  serialized representation: **a snapshot is not portable across Monty versions.**
  Session state must therefore be version-stamped and discarded on mismatch.

## 8. Tasks

**Phase 1 — the seam (R1, R2)**
- [ ] `CodeSessionState`, `supports_sessions`, `start_in_session` (defaulted), `RunStep::with_session/session` — `adk-agent` (D1)
- [ ] Version-stamp session bytes; discard on mismatch (Q5)
- [ ] `MontyRuntime::start_in_session` over `MontyRepl`, incl. `ReplStartError` state preservation (D1)
- [ ] Unit tests: stateless runtime unaffected; continuity; failure-then-read

**Phase 2 — persistence (R3)**
- [ ] `SESSION_STATE_KEY` + `persistent_session` builder flag + size budget with graceful degradation (D1)
- [ ] Measure realistic snapshot sizes; fix the default (Q1)
- [ ] Restart round-trip test + oversize/corrupt negative control

**Phase 3 — diagnostics (R4, R8)**
- [ ] `ScriptFrame`/`ScriptError`, `raised_with`, capability flag, renderer (D2)
- [ ] Monty projection from `MontyException::traceback()` (D2)
- [ ] Ordered `ScriptOutput` via `collect_streams` (D6)

**Phase 4 — sandbox + orchestration (R5, R6)**
- [ ] `WorkspaceMount`/`MountAccess` + builder wiring + allow/deny tests (D3)
- [ ] Join with `adk_devtools::Workspace` in the coding-agent path (D3)
- [ ] Multi-`PendingCall` step + dispatch via `ToolExecutionStrategy` (D4)

**Phase 5 — lazy binding (R7)**
- [ ] `NameLookup` resolution + `render_tools` summary mode + capability flag (D5)

**Phase 6 — cancellation (R9)**
- [ ] Tracker-installed cancellation wired to `CancellationToken` (D7)

**Housekeeping (independent, do first — it is one line plus a comment)**
- [ ] Record in `adk-codeact-monty/Cargo.toml` *why* `publish = false` must remain:
      the `get-size2` lockfile pin cannot travel with a published crate (N1)

## 9. Sequencing note

Phases 1–3 are additive and behind defaults, so they are safe on a 2.0.x line.
Phase 4's `ToolExecutionStrategy` reuse touches shared dispatch and wants its own
review. Nothing here needs to precede the 2.0.0 tag.
