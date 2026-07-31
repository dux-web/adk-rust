# Spec: The CodeAct Python Runtime on `monty-pool`

| | |
|---|---|
| **Status** | Draft / RFC — revision 2, pending approval |
| **Depends on** | #525 (`adk-codeact-monty` on crates.io monty 0.0.19) |
| **Affected crates** | `adk-codeact-monty` (rewritten as a pool client), `adk-agent` (`codeact`); docs, CI |
| **Related** | #380, `docs/design/coding-agent.md`, `docs/official_docs/agents/code-agent.md` |
| **Reference** | `reference/monty` (gitignored clone of `pydantic/monty`) |

## Revision history

**r1** designed session support, mounts, tracebacks, print streams and cancellation
*in-process*, by adopting more of the `monty` crate's API (`MontyRepl` et al).

**r2 (this revision) pivots to `monty-pool`.** r1 was researched depth-first inside
an API that had already been chosen, without first asking whether it was the right
integration. It is not: Monty's own guidance is that `monty-pool` — not the
in-process crate — is *"the recommended way to run Monty from Rust"* for untrusted
code, and LLM-generated Python is untrusted by construction. Nearly every feature
r1 proposed to build already exists there, with isolation the in-process API
cannot provide. r1's requirements survive; its design mostly does not.

The one r1 artifact that stands is the **session seam** already merged in
`1afbb693` (`CodeSessionState`, `CodeRuntime::start_in_session`,
`RunStep::with_session`). It is runtime-agnostic and maps directly onto
`Checkout::dump`/`restore`, so it was not wasted work.

---

## 1. Summary

Rewrite `adk-codeact-monty` from an in-process interpreter embedder into a
**`monty-pool` client**: Python executes in `monty` worker subprocesses, one
dedicated `Checkout` per CodeAct session, reached over Monty's wire protocol.

This buys three things r1 could not:

1. **Crash isolation.** Monty's README is explicit: *"a Monty process can never be
   made fully crash-proof against memory errors (stack overflow aborts, allocator
   aborts)."* Today those aborts kill **the whole ADK agent process** — and no
   `ResourceLimits` value prevents them, because limits catch time and memory, not
   aborts. In the pool, a crash kills only the worker; the pool observes
   `PoolError::Crashed`, discards it, and spawns a replacement.
2. **A hard timeout that can catch a hang the sandbox cannot see** — a parent-side
   watchdog kills any worker exceeding `request_timeout`.
3. **Publishability.** Verified by `cargo tree`: the `get-size2 0.10.1` lockfile
   pin — the sole reason the crate is `publish = false` — comes only via
   `monty` → `ruff_python_ast 0.0.3`. `monty-pool` depends on `monty-types`,
   `monty-fs`, `monty-proto`, `tungstenite`, `rustls` and **not** on `monty` core.
   Dropping the in-process interpreter drops the pin, so the crate can ship.

That third point closes the largest real gap in the whole CodeAct feature: today a
crates.io user gets `CodeActAgent` and the `CodeRuntime` trait with **no runtime
able to execute anything**.

## 2. Evidence: what `monty-pool` already provides

`Checkout::feed(code, inputs, mounts, skip_type_check, on_print) -> TurnEvent`,
plus `resume` / `resume_name_lookup` / `resume_futures` / `dump` / `restore` /
`finish`.

| r1 requirement | Already provided |
|---|---|
| R1/R2 sessions | A `Checkout` is a REPL session; *"session state persists between feeds"*. `PoolError::Runtime` and `PoolError::Typing` *"leave the session usable"*, so a failed step keeps its namespace. |
| R3 restart survival | `Checkout::dump`/`restore` — *"including on a different worker or machine"*, strictly better than an in-process heap dump. |
| R5 mounts | `feed(mounts: Vec<MountSpec>)` — **per-feed**, finer-grained than r1's per-runtime `MountTable`. |
| R8 print streams | `on_print(stream, text)` — a streaming callback, better than collect-then-return. |
| R9 cancellation | `request_timeout` watchdog + `max_duration` enforced from outside the child. |
| *(missed in r1)* inputs | `feed(inputs: Vec<(String, MontyObject)>)` — host values as sandbox globals. |
| *(never considered)* | **Type checking every snippet** (`skip_type_check`, `PoolError::Typing`, via `monty-type-checking`/`ty`). |
| *(never considered)* | **Dependency installation** (`InstallDependencies`). |
| *(impossible in-process)* | Crash isolation, worker recycling (`max_checkouts_per_worker`), untrusted-child wire validation. |

Two transports: `PoolConfig::subprocess(path)` (poolable, prewarmed, replaced on
crash) and `PoolConfig::websocket(url)` (single-use, remote; isolation is the
remote host's responsibility).

## 3. Goals / Non-goals

**Goals**

- **G1** Execute CodeAct Python in crash-isolated workers, so adversarial or buggy
  generated code cannot abort the agent process.
- **G2** Make `adk-codeact-monty` **publishable**, so the CodeAct feature is usable
  from crates.io.
- **G3** Keep the `CodeRuntime` seam unchanged for `adk-agent` — the pivot must not
  leak into the agent.
- **G4** Adopt, rather than rebuild: sessions, per-feed mounts, inputs, streaming
  print, watchdog timeouts, and snippet type checking.
- **G5** Never block the async runtime on worker IPC.
- **G6** Degrade honestly when the worker binary is absent — a clear error or a
  skipped test, never a silent fallback to unisolated execution.

**Non-goals**

- **N1** Keeping the in-process interpreter in this crate. It is what forces the
  `get-size2` pin, so it is mutually exclusive with G2 (see D5).
- **N2** Bundling or vendoring the `monty` binary.
- **N3** Concurrent tool dispatch. Withdrawn in r1 and still withdrawn: the seam
  is single-continuation by design, and `TurnEvent`'s futures variant is answered
  one call at a time. (r1 §D4 argument retained below.)
- **N4** Changing the CodeAct prompt/protocol (`call_tool`, `final_result`).

## 4. Requirements

**R1 — Isolated execution.** A script that aborts the interpreter (stack overflow,
allocator abort) must not terminate the agent process.
*Acceptance:* a test feeding deliberately abort-inducing code observes
`PoolError::Crashed`, the agent survives, and a subsequent step succeeds on a
replacement worker.

**R2 — Sessions.** Successive steps in one CodeAct session share a namespace.
*Acceptance:* `x = 1` in step 1 is readable in step 2; a step that raises still
leaves `x` readable in step 3.

**R3 — Session durability.** `Checkout::dump` bytes round-trip through
`CodeSessionState` and `SessionService`; after a simulated restart a prior variable
is readable. Over a configured budget, degrade to a fresh session with a warning.

**R4 — Tool calls.** `TurnEvent`'s function-call variant maps to `RunStep::Call`
and resumes with the tool result, preserving today's `call_tool(...)` contract and
the existing 22 `drive_script` behaviours.

**R5 — Mounts.** A script reads under a read-only mount, is denied writing to it,
writes under a read-write mount, and is denied outside every mount.

**R6 — Inputs.** The driver can bind host values as named globals, so data need not
be interpolated into the script source (and therefore need not round-trip through
the model).

**R7 — Type checking.** A snippet with a static type error is reported to the model
as a script error with the session intact, before execution. Defeatable per feed via
`skip_type_check`.

**R8 — Streaming output.** `print()` reaches the driver as `(stream, text)` while
the script runs, preserving stream identity and order.

**R9 — Timeout.** A script exceeding the configured wall clock is killed by the
parent watchdog and surfaced distinctly from a script raise.

**R10 — Worker discovery.** The binary is located explicitly (builder path, then
`ADK_MONTY_BINARY`, then `PATH`), and its absence is a clear construction error.

**R11 — Publishable.** `adk-codeact-monty` builds from crates.io with no lockfile
pin, and `cargo package` verifies.

## 5. Design

### D1 — Runtime shape

```rust
pub struct MontyPoolRuntime {
    pool: Arc<Pool>,                 // elastic, prewarmed workers
    repl: ReplConfig,                // per-session limits + type checking
    mounts: Vec<WorkspaceMount>,     // applied per feed
    os: Arc<OsPolicy>,               // env/clock policy (unchanged in spirit)
}
```

`CodeRuntime` impl unchanged in signature (**G3**):

| `CodeRuntime` | Pool call |
|---|---|
| `start(script, name)` | `pool.checkout(&repl)` → `feed(script, inputs, mounts, …)` → drop the checkout on completion |
| `start_in_session(state, script, name)` | `checkout` → `Checkout::restore(state.bytes_for(RUNTIME_ID))` → `feed(…)` → `dump()` into the returned `RunStep` |
| `resume(snapshot, with)` | `restore(snapshot)` → `Checkout::resume(value, on_print)` |
| `supports_sessions()` | `true` |

`RUNTIME_ID` is `"monty-pool/0.0.19"`, so a snapshot from a different Monty
version is rejected by `bytes_for` and degrades to a fresh session (r1's Q5
mitigation, already implemented).

**One snapshot format.** `Checkout::dump` serializes an idle *or suspended*
session, so the mid-call continuation and the cross-step session are the same
bytes. The tagged-envelope problem that blocked r1's Phase 1 does not arise.

### D2 — Event and error mapping

`TurnEvent` → `RunStep`: completion → `Complete`; function call → `Call`; OS call
→ resolved in-place against the host policy and resumed; name lookup → `Undefined`
(tools are only callable, never bare names); futures → answered one at a time with
the existing corrective message (**N3**).

`PoolError` splits along the seam's existing script-vs-host line:

| `PoolError` | Maps to | Why |
|---|---|---|
| `Runtime` | `RunStep::Raised` | A script error; session stays usable — the model can fix it. |
| `Typing` | `RunStep::Raised` | Same: the model wrote code that does not type-check (**R7**). |
| `Timeout` | `RunStep::Raised`, distinctly worded | The model should see it and write cheaper code (**R9**). |
| `Crashed` | `RunStep::Raised` + `warn!` | The script killed a worker; the pool already replaced it. Not a host failure — the agent is healthy (**R1**). |
| `Protocol`, pool exhaustion, spawn failure | `RuntimeError` | Genuine host breakage. |

Crash-as-`Raised` is deliberate: the agent survives, so the run should continue
with the model informed, not abort.

### D3 — The async boundary (**G5**)

`monty-pool` is fully synchronous (no tokio) and now performs subprocess IPC, so
the seam's stated assumption — *"advancing the interpreter is synchronous and
fast"* — no longer holds. Every pool call is wrapped in
`tokio::task::spawn_blocking`. Because `CodeRuntime::start`/`resume` are sync but
called from async agent code, the adapter owns a small blocking bridge; the trait
does not change.

### D4 — Worker discovery (**R10**, **G6**)

Resolution order: explicit builder path → `ADK_MONTY_BINARY` → `monty` on `PATH`.
Absence is a construction-time error naming the fix. No silent fallback to
in-process execution — that would quietly drop the isolation this design exists to
provide.

**The install command must carry `--locked`, and this was verified, not assumed.**
`cargo install monty-runtime --version 0.0.19` **fails to compile**:

```
error[E0277]: the trait bound `CompactString: GetSize` is not satisfied
  --> ruff_python_ast-0.0.3/src/name.rs:15
  = note: there are multiple different versions of crate `compact_str` in the
          dependency graph
```

It is the same `get-size2` break that pins our own lockfile (§1): a fresh
resolution picks `get-size2 0.10.3` → `compact_str 0.10`, while
`ruff_python_ast 0.0.3` derives `GetSize` on `compact_str 0.9` fields. Binary
crates publish their `Cargo.lock`, so `--locked` uses Monty's own known-good
resolution:

```bash
cargo install monty-runtime --version 0.0.19 --locked
```

Consequences for the design:

- The setup error must print the command **with `--locked`**. Printing it without
  would send every user into a confusing upstream compile failure — the single
  highest-value string in this feature.
- Pin the version alongside `monty-pool`, since the wire protocol is version-tied (Q4).
- CI installs the same way.

Runtime tests **skip with a printed reason** when the binary is absent, the same
pattern used for the Windows sandbox portability tests, so a contributor without
it still gets a green, honest run.

### D5 — In-process path: removed, not feature-gated

Feature-gating in-process support in the same crate would look attractive but is a
footgun: cargo publishes the manifest with every feature, so a downstream user
enabling it would hit the unpinnable `get-size2` resolution and fail to build. The
in-process interpreter therefore **leaves this crate**. If an embedded, non-isolated
runtime is wanted later (no external binary, lower latency, trusted code only), it
belongs in a separate unpublished crate — a decision deferred, not made here.

### D6 — Error rendering

`monty-types` still supplies `MontyException` and `StackFrame`/`CodeLoc` over the
wire, so r1's structured-traceback design (r1 §D2 — `ScriptFrame`/`ScriptError`,
capability flag, CPython-style rendering) carries over unchanged and is the one r1
design section adopted wholesale.

### Testing

- Isolation (**R1**): abort-inducing script → `Crashed` → agent alive → next step
  succeeds on a replacement worker. This is the test that justifies the pivot.
- Sessions (**R2**, **R3**): continuity; failure-then-read; dump→restore across a
  *different* worker; oversize/corrupt state degrades rather than fails.
- Behaviour parity (**R4**): the existing 22 `drive_script` tests must pass against
  the pool runtime with only construction changed — the regression net for the
  rewrite.
- Mounts (**R5**) allow/deny matrix; inputs (**R6**); type-error-with-live-session
  (**R7**); interleaved stream order (**R8**); timeout distinct from raise (**R9**);
  missing-binary error text (**R10**); `cargo package` (**R11**).

## 6. Risks and open questions

- **Q1 — Worker binary as a deployment dependency.** The strongest objection to
  this pivot, and measurably rougher than r2 first assumed: the plain
  `cargo install monty-runtime` **does not compile** (see D4) and needs `--locked`.
  It is a Rust binary rather than a Python install, but it is still an artifact to
  build, ship, and locate, and the install has a sharp edge we must paper over in
  our own error text. Weigh against: without it there is no crash isolation *and*
  no publishable runtime.
- **Q2 — Latency.** Per-step IPC plus checkout, versus an in-process call. Prewarmed
  workers (`min_processes`) amortize spawn cost; unmeasured, and worth measuring
  before defaults are fixed.
- **Q3 — Session size** (carried from r1). `Checkout::dump` size is unmeasured;
  measure before fixing `max_session_state_bytes`.
- **Q4 — Monty is `0.0.x`.** Both the wire protocol and the snapshot format may
  change per release. `RUNTIME_ID` gates snapshots; the protocol is pinned by the
  dependency version. A `monty` binary older or newer than the linked
  `monty-proto` is a real misconfiguration — needs a version handshake check.
- **Q5 — Type checking cost.** On by default is the safer choice for generated
  code, but it runs per feed; measure before deciding the default.
- **Q6 — `InstallDependencies`.** Powerful (pip packages in the sandbox) and
  dangerous (network egress, supply chain). Out of scope here; do not enable without
  its own security review.

## 7. Tasks

**Phase 0 — decide and prepare**
- [x] Session seam in `adk-agent` (merged, `1afbb693`)
- [x] Clone `pydantic/monty` into gitignored `reference/`
- [ ] Accept or reject the pivot (this document)
- [ ] CI: install `monty-runtime`; make runtime tests skip-with-reason when absent

**Phase 1 — pool client**
- [ ] `MontyPoolRuntime` + builder; worker discovery (D1, D4)
- [ ] `TurnEvent` → `RunStep`; `PoolError` split (D2)
- [ ] `spawn_blocking` bridge (D3)
- [ ] Port the 22 `drive_script` tests; drop the in-process path (D5)

**Phase 2 — sessions**
- [ ] `start_in_session` over `restore`/`dump`; `RUNTIME_ID` gating (D1)
- [ ] Persist via `SessionService` + size budget; restart and cross-worker tests

**Phase 3 — isolation and limits**
- [ ] Crash test (**R1**); watchdog timeout mapping (**R9**); worker recycling config

**Phase 4 — adopt the rest**
- [ ] Per-feed mounts (**R5**); inputs (**R6**); type checking (**R7**); streaming
      print (**R8**); structured tracebacks (r1 §D2)

**Phase 5 — ship**
- [ ] Drop `publish = false` and the `get-size2` pin; `cargo package` verifies (**R11**)
- [ ] Docs: `code-agent.md` rewrite, worker install, isolation guarantees, and the
      note that script-level `async`/`gather` is serialized at the call boundary (**N3**)

## 8. Sequencing

Nothing here precedes the 2.0.0 tag. Phase 5 is the point of the exercise — a
published, crash-isolated Python runtime — so Phases 1–4 should not be split
across releases if avoidable.

**Method note.** Twice in r1 I researched depth-first inside an assumed API instead
of first asking whether it was the right one: it missed `feed`'s `inputs` (an unused
*parameter* of a *used* call, invisible to a survey of unreferenced symbols) and
then the entire recommended integration. Reading a vendor's whole workspace as a
reference crate, before designing against one of its modules, is cheaper than
either correction.
