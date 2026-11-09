# Zeron Orchestrator v2: acceptance criteria (v2, approved 2026-11-08: proofs for S3, S9, S10; conformance + Kani)

Scope: port T3 Code orchestration v2 into Zeron, with the fixes the first model found (`../tla/README.md`). IDs freeze when Tyler approves; later changes add IDs, never renumber. Bounds marked *provisional* wait on the state-space measurement.

"Every permutation of state" means:
1. **Bounded:** the checker visits every reachable state within stated bounds (exhaustive BFS, not sampling).
2. **Unbounded:** selected safety properties are proved for any number of threads, runs, and devices (TLAPS).
3. **Implementation:** the Rust code is checked for conformance with the spec (section E). Conformance testing is strong evidence, not a proof; optional Kani proofs cover the pure core on bounded inputs.

## A. Functional criteria (what "better" means)

Each F has a **witness**: the success state written as a negated invariant that the checker must violate. A spec that over-restricts fails its witness.

- **F1** Delegation deeper than one level, up to MaxDepth. Witness: a grandchild run completes and its result reaches the root.
- **F2** Results are pushed to the parent; no polling. Witness: the parent's wake run starts with the child's result and the parent made no read call.
- **F3** Several child results that settle while the parent is busy arrive in one wake message.
- **F4** Wait mode: the parent can block on a child and get its result inside the same run.
- **F5** Fork and merge-back: a fork's delta (fork point → source point) is delivered to the parent as context.
- **F6** Agent switch with a structured handoff: a new harness continues a thread with a budgeted context packet.
- **F7** Child questions surface in the parent (or to the person) and the answer returns to the child.
- **F8** Stop cascades: stopping a thread stops its whole subtree.
- **F9** Usage-limit recovery: a run that fails on a usage limit resumes after the reset without the person.
- **F10** Native subagents (Claude Task, Codex agents) appear in the same task model as Zeron child chats.
- **F11** Children may run on another device or in the cloud.

## B. Port requirements from model 1 (R1-R8)

- **R1** When a run ends, deliver every result it was still holding (wait sweep).
- **R2** Count an acknowledgement only from the live run that read the result; cancel a wait when its run ends.
- **R3** Enforce MaxDepth and MaxFanout in the engine, not only in MCP.
- **R4** When a thread's access narrows, clamp every descendant.
- **R5** Keep the stop cascade (and every effect) in a durable outbox; replay idempotently.
- **R6** Idempotent steps: settle keyed on (child, child run); wake dispatch gated on generation with a fixed message id; planning skips handled results.
- **R7** Interrupt is a request; the run ends only on provider confirmation; no delegation after Stop.
- **R8** Questions route past a blocked parent.

## C. Model completeness

- **AC-M1 Modules.** `Delegation`, `RunLifecycle` (queue, dispatch, steer, restart, retry, follow-ups, interrupt, usage-limit resume), `ContextTransfer` (fork, switch, merge-back, budget), `Placement` (host device, relay, offline, cloud), `NativeSubagents`, and `Composition` (all together).
- **AC-M2 Exhaustive at bounds.** Per-module bounds, set from the measurement (*provisional*: threads 4, depth 2-3, runs 3, devices 2 + cloud, crash and duplicate budgets 1). `Composition` is checked exhaustively at smaller bounds and by random simulation at larger ones.
- **AC-M3 Bound saturation.** Each module is also checked at N+1 in its key dimension; no new violation or new action coverage may appear.
- **AC-M4 Unbounded proofs.** TLAPS inductive-invariant proofs for S3 (exactly once), S9 (one host), S10 (idempotent replay). Checked by `tla-ts prove` + tlapm in CI. *Cost: tlapm must be installed; proofs of crash/duplicate protocols are the most labor-heavy part.*
- **AC-M5 No dead parts.** Every action is enabled in some reachable state (coverage report).
- **AC-M6 No vacuous properties.** Each property has a mutant spec (one guard removed) that the checker must fail. Mutants run in CI.
- **AC-M7 Deadlock.** None, except documented terminal states.

## D. Safety properties (every reachable state)

- **S1** At most one active run per thread.
- **S2** Only a run's root node ends the run; a child, tool, or native subagent ending never ends the parent run.
- **S3** Each task result is handled exactly once: delivered at most once, never both delivered and disposed.
- **S4** No wake message is dispatched into a live parent run.
- **S5** Depth ≤ MaxDepth, fan-out ≤ MaxFanout.
- **S6** A child's access is never wider than its parent's, including after narrowing.
- **S7** An acknowledgement counts only from the live run that read the result.
- **S8** Context order: a run never sees context from after its source point. A handoff or merge-back carries exactly the messages in its range, or a stated subset with a coverage header; never a silent drop or a duplicate.
- **S9** One host: each chat executes on exactly one device; only the host drains its commands. (Zeron has no host move today; if host handover is added, it must not run a command twice or lose one.)
- **S10** Replaying any command or effect after a crash gives the same state as running it once.
- **S11** After Stop, no new delegation starts and every descendant ends stopped or terminal.
- **S12** Each pending child question has exactly one visible answerer at a time; an answer is applied once.
- **S13** A native subagent ending never ends the parent run; orphans close when the parent run ends.

## E. Liveness (fairness; devices eventually online)

- **L1** Every settled result is eventually delivered or disposed.
- **L2** Stop eventually quiesces the subtree.
- **L3** A queue that is not held eventually drains.
- **L4** Every child question eventually reaches an answerer.
- **L5** Every context transfer eventually resolves: applied, or failed with a reason.
- **L6** A parent with settling children still eventually runs its wake.

## F. Plan verification

- **AC-P1 Design doc.** `DESIGN.md` maps each T3 concept to a Zeron type, module, and file, and traces each F, R, S, L to spec actions and planned code.
- **AC-P2 Refinement.** `ZeronImpl` models Zeron's real mechanisms (per-chat command ledger, host drain, held queue, relay, run journal, outbox). The checker verifies `ZeronImpl ⇒ Composition` with `PROPERTY Abstract!Spec` under a written refinement mapping (tla-ts supports `INSTANCE` and `PROPERTY`).
- **AC-P3 Review.** Tyler approves `DESIGN.md` and this list before code starts.

## G. Implementation verification

- **AC-I0 Reducer design.** The orchestrator core is a pure function `step(state, event) -> (state, effects)`. Each branch maps 1:1 to a `ZeronImpl` action. All I/O goes through the durable outbox.
- **AC-I1 Trace validation.** The engine logs spec-level events with per-device sequence numbers and relay message ids (no clocks). A checker merges device logs by those ids and replays them against `ZeronImpl`; every step must be a valid action. 100% of integration-test traces and one dogfood week validate.
- **AC-I2 Model-based tests.** Generated from the state graph: every action, every guard both true and false, and every mutant from AC-M6 caught by at least one test. Run against the engine with a fake harness (T3-style provider replay). All pass.
- **AC-I3 Fault injection.** Engine tests inject each spec `Environment` action: crash at every effect boundary, duplicate and reordered relay messages, device offline and back, usage limit.
- **AC-I4 Optional proofs.** Kani or proptest checks on `step()` for S1, S3, S5, S6 on bounded inputs.
- **AC-I5 CI gate.** Spec checks, proofs, mutants, trace validation, and model-based tests run on every push to the orchestrator branch; any failure blocks.

## H. Assumptions (stated, not verified)

- Harnesses report their own events correctly.
- The relay eventually delivers; loss happens only while a device is offline.
- Clocks are not trusted for order; sequence numbers are.
- Bounds are a choice; AC-M3, AC-M4, and section G limit what that choice can hide.
