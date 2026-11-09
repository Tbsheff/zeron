# Map live actions

Checked on 2026-11-09. The 32-name count covers `RunEvent`, `DelEvent`, and
`OutboxEvent`. The native action and fact rows below extend the source map;
they do not change that count. This map does not cover every question, remote,
or transfer action.

- **Keep scope:** Engine captures contain 26 of these 32 accepted action names.
- **Check core:** Two scripted Driver captures contain all 32 accepted names and
  rejects for 17 names. Both paths pass exact state, ordered effect, and transaction
  checks: 105 rows in total.
- **Reject claims:** Driver tests do not prove live callers. Neither count proves
  [AC-I2](ACCEPTANCE.md#g-implementation-verification), which also needs generated
  engine cases for every guard in both states and every required mutant.

## Map the 26 live names

Each row names a constructor in live engine code. Captures use a fake engine or
harness; they do not prove real provider or network behavior. A constructor proves
that a path exists in source, not that each branch has run in a capture.

| Apply action | Enter through | Read source |
|---|---|---|
| Apply `PersonSend` | Send a queued person message | [`send_event`](../../crates/engine/src/orchestrator/wire.rs#L749) |
| Apply `Steer` | Send a steer to the live attempt | [`send_event`](../../crates/engine/src/orchestrator/wire.rs#L749) |
| Apply `RestartActive` | Send now during a live attempt | [`send_event`](../../crates/engine/src/orchestrator/wire.rs#L749) |
| Apply `ServerEnqueue` | Queue a late native result for the parent | [`deliver_native`](../../crates/engine/src/orchestrator/wire/questions.rs#L92) |
| Apply `ProviderTerminal` | Receive a root `Done`, reconcile a missing terminal, or fail a start | [`done_locked`](../../crates/engine/src/orchestrator/wire.rs#L905), [`terminal_unstarted`](../../crates/engine/src/orchestrator/wire.rs#L1377), [`end_stuck`](../../crates/engine/src/orchestrator/wire.rs#L445) |
| Apply `IngestEnd` | Consume the current root terminal | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `RetryAttempt` | Consume a retryable current root terminal within budget | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `IngestStale` | Remove an old root terminal | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `StartFromQueue` | Start an idle, unheld thread with a non-wake queue head | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `ResetPasses` | Fire the armed usage-limit timer | [`fire_locked`](../../crates/engine/src/orchestrator/wire.rs#L1481) |
| Apply `UsageLimitRecovery` | Start a resume run after that timer fires | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `OpenRoot` | Create local thread state | [`thread`](../../crates/engine/src/orchestrator/wire.rs#L556) |
| Apply `Stop` | Interrupt a chat or cancel a task | [`request_interrupt`](../../crates/engine/src/orchestrator/wire.rs#L847), [`task_cancel`](../../crates/engine/src/orchestrator/wire/tasks.rs#L665) |
| Apply `InterruptChat` | Accept an otherwise unrequested engine interrupt before its terminal | [`engine_interrupt`](../../crates/engine/src/orchestrator/wire.rs#L953) |
| Apply `NarrowAccess` | Lower a send, task, or remote access ceiling | [`decide_send`](../../crates/engine/src/orchestrator/wire.rs#L705), [`tasks`](../../crates/engine/src/orchestrator/wire/tasks.rs#L252), [`remote`](../../crates/engine/src/orchestrator/remote.rs#L637) |
| Apply `Delegate` | Place a new child task | [`place`](../../crates/engine/src/orchestrator/wire/tasks.rs#L374) |
| Apply `Followup` | Start the next task on an existing child chat | [`tasks`](../../crates/engine/src/orchestrator/wire/tasks.rs#L302) |
| Apply `ReadTaskResult` | Observe a successful matching task tool result in the current attempt | [`observed`](../../crates/engine/src/orchestrator/wire/tasks.rs#L784) |
| Apply `WaitReturn` | Observe the wait result, or write the reply for a harness without MCP result events | [`await_wait`](../../crates/engine/src/orchestrator/wire/tasks.rs#L464), [`apply_wait`](../../crates/engine/src/orchestrator/wire/tasks.rs#L533) |
| Apply `WaitTimeout` | Reach the wait deadline | [`await_wait`](../../crates/engine/src/orchestrator/wire/tasks.rs#L464) |
| Apply `WaitAbandon` | Drop the wait handler, or consume a completed root terminal while waiting | [`abandon_wait`](../../crates/engine/src/orchestrator/wire/tasks.rs#L569), [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `StartFromQueueWake` | Start an idle thread from a valid wake queue head | [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187) |
| Apply `DoNotice` | Deliver a task-settle message | [`deliver`](../../crates/engine/src/orchestrator/wire/tasks.rs#L878) |
| Apply `DoOffer` | Deliver a cohort offer message | [`deliver`](../../crates/engine/src/orchestrator/wire/tasks.rs#L878) |
| Apply `DoStop` | Deliver a child-stop message | [`deliver`](../../crates/engine/src/orchestrator/wire/tasks.rs#L878) |
| Apply `CrashEngine` | Open the wire and release old waits | [`crash_waits`](../../crates/engine/src/orchestrator/wire/tasks.rs#L937) |

## Keep six core actions separate

These actions remain public reducer inputs. Tests, proof code, enum definitions,
dispatch arms, and trace name lists are not live producers.

| Retain action | Check core state and effects | Use the live path |
|---|---|---|
| Retain `ChildStart` | Require a live attempt with no terminal or open child; set `attempt.kid_open`. Emit no action-specific effect. | No live caller. Native subagents use `NativeStart`, which creates a keyed native task and emits `NativeChanged`; it does not set `kid_open`. See [`run`](../../crates/orch/src/run.rs#L145), [`native adapter`](../../crates/engine/src/orchestrator/native.rs#L19), and [`native reducer`](../../crates/orch/src/native.rs#L58). |
| Retain `ChildEnd` | Require `kid_open`; clear it and add a completed `Node::Child` event for the current run and attempt. Emit no action-specific effect. | No live caller. Native subagents use `NativeEnd`, which changes a native task and may emit delivery effects. A delegated child chat ends its own root run and sends `Notice`; it is not a `Node::Child` in the parent's run. See [`run`](../../crates/orch/src/run.rs#L153), [`native reducer`](../../crates/orch/src/native.rs#L83), and [`end_run`](../../crates/orch/src/delegation.rs#L439). |
| Retain `IngestChild` | Require a current `Node::Child` event in the inbox; remove only that event. Keep the run live. Emit no action-specific effect. | `auto_step` can select it, but no live source creates its input: the sole `Node::Child` constructor is `child_end`. This is a consumer without a live producer. See [`run`](../../crates/orch/src/run.rs#L321) and [`auto_step`](../../crates/engine/src/orchestrator/wire.rs#L187). |
| Retain `ReadResult` | Require the child's task run to be 1, then use the shared read guard and acknowledgment update. | `ReadTaskResult` supplies the child task run explicitly. With `task_run=1`, both core events have the same guard, state, and ordered effects; they call the same `read_result` function. With a later task run, `ReadResult` rejects `StaleRun`. This equality does not equate event names or Driver receipt keys. See [`dispatch`](../../crates/orch/src/delegation.rs#L42), [`read_result`](../../crates/orch/src/delegation.rs#L202), and [`observed`](../../crates/engine/src/orchestrator/wire/tasks.rs#L784). |
| Retain `LateWaitAck` | Require `late_wait`; clear that marker. Emit no action-specific effect. | No live caller. `apply_wait` ignores a handler unless its captured attempt is still live, has no terminal, and still waits on the same child. Ignoring the handler leaves state unchanged; it does **not** perform `LateWaitAck`. A non-completed run end can set `late_wait`; `CrashEngine` clears it. See [`late_wait_ack`](../../crates/orch/src/delegation.rs#L290), [`end_run`](../../crates/orch/src/delegation.rs#L439), [`apply_wait`](../../crates/engine/src/orchestrator/wire/tasks.rs#L533), and [`crash`](../../crates/orch/src/outbox.rs#L96). |
| Retain `RequestInterrupt` | For a known thread, set the stop hold, clear usage-limit state, and set `int_req` if live. Emit `InterruptRun` only if live. | No live caller of this enum variant. The wire method named `request_interrupt` applies `Stop`: it also stops cohorts, disposes unhandled child deliveries, removes wakes, reroutes questions, and sends child stops. `Stop` emits `InterruptRun` for a parked thread when its run ordinal is positive and it owns a running background native task; the task stays running until a real terminal or stream exit. `InterruptChat` instead requires a live thread without `int_req`; it sets only `int_req` and emits `InterruptRun`. Neither event is an exact alias. See [`request_interrupt`](../../crates/orch/src/run.rs#L103), [`mark_interrupt`](../../crates/orch/src/state.rs#L397), [`thread_stop`](../../crates/orch/src/delegation.rs#L82), and [`interrupt_chat`](../../crates/orch/src/delegation.rs#L146). |

The state and effect comparisons above concern the core event relation. Every
successful `step` also runs native reconciliation and run retirement in
[`step.rs`](../../crates/orch/src/step.rs). Comparing only an
`InterruptRun` effect would miss the stop hold, timer, cohort, and child changes.

`ReadTaskResult.run` names the parent's live run. `task_run` names the child's
task. `task.cohort` names the parent's cohort that owns its delivery. Follow-up
tasks can make these numbers differ; do not use one in place of another.

## Map nested native activation

`NativeStartNested` is a separate sixth `NativeEvent` name. The earlier
26-live-plus-six-retained inventory covers only the 32 run, delegation, and
outbox names. It does not include this new native action or prove its guards.

| Apply action | Enter through | Check state and effects |
|---|---|---|
| Apply `NativeStartNested` | Activate a full native path with more than one key in [`activate`](../../crates/engine/src/orchestrator/native.rs#L188). The adapter supplies the immediate ancestor key. | Accept an existing task key without changes. For a new key, require a different ancestor key, an existing running ancestor, and an existing parent thread. Require either a background ancestor or the same live parent attempt with no terminal as that ancestor. Copy the ancestor's attempt; set background to the input flag OR the ancestor flag; store the ancestor; create running, pending delivery with generation zero; emit `NativeChanged`. Read [`StartNested`](../../crates/orch/src/native.rs#L93). |

`NativeStart` takes the current parent attempt and stores no ancestor. The two
actions have different guards and task state. They are not aliases. Both use
the common native reconciliation and run retirement after the action. Neither
action creates a root terminal. Full path identity also keeps equal leaf tool
IDs under different ancestors separate.

## Keep observation facts separate

| Observe fact | Read the source | Limit the claim |
|---|---|---|
| Observe `runtime_binding` | Read [`RuntimeBinding` and `record`](../../crates/engine/src/orchestrator/runtime_binding.rs#L14) and [`binding_snapshot`](../../crates/engine/src/sessions.rs#L3488). Record the runtime ID, logical run and attempt, origin input, access, operation, source ID, and source revision. | Treat install, issue, rebind, retire, remove, and config as observations of the runtime handle. The issue fact records the request access before the harness call. They are not reducer actions or evidence that a provider process is alive. Join by captured IDs and revisions; do not turn the fact into `NativeStartNested` or a terminal event. |
| Observe `native_binding` | Read the full path binding in [`native adapter`](../../crates/engine/src/orchestrator/native.rs#L11). | Join the captured full path to its native task key. The fact does not create or end a native task and does not increase the accepted-action count. |

Count native action guards and runtime facts in separate inventories. A captured
binding cannot replace an accepted `NativeStartNested` receipt or a raw provider
observation. Fresh captures must prove each of those source seams in its own
scope before an AC-I2 claim can include it.

## Check root and child event routes

The live route preserves the required distinction in
[S2 and S13](ACCEPTANCE.md#d-safety-properties-every-reachable-state): only a root
terminal ends its run.

```text
Root Done                  -> ProviderTerminal(Root) -> RetryAttempt or IngestEnd
Native tagged Done         -> NativeEnd              -> native task update
Delegated chat root Done   -> child's IngestEnd       -> Notice -> DoNotice
Generic ChildEnd core input -> inbox Child event      -> IngestChild
                                                       (no live producer)
```

[`SessionsEngine::publish`](../../crates/engine/src/sessions.rs#L2650) sends only a
top-level `AgentEvent::Done` to `provider_done`. A tagged `AgentEvent::Subagent`
goes through the [native adapter](../../crates/engine/src/orchestrator/native.rs#L45).
`NativeEnd` does not write a root terminal or end the parent run. A delegated chat
has its own thread and root run; its notice settles its task in the parent's state.
These paths support the same safety rule, but their full states and effects differ.

## Keep the open work precise

No missing public feature follows from these six absent event names. The live code
has routes for native status, delegated task completion, task reads, wait expiry,
late-handler rejection, and user stop. The generic `kid_open` / `Node::Child` feed
has no live adapter; adding calls just to raise an action count would create a new
runtime path without a current need.

Full AC-I2 remains open. The 26-name engine inventory does not prove every guard
in both states, every required mutant, or the unused generic child-node route.
The two Driver tests prove the retained core relations on finite scripted paths;
they do not close the engine test requirement. Keep the compatibility events
unless a separate API and stored-event review approves their removal.

Source check: search `crates/**` for all six names and `Node::Child`; separate live
constructors from `tests`, `#[cfg(test)]`, `#[cfg(kani)]`, dispatch, and name tables.
Capture evidence: `tla/v2/runtime/run-delegation-phase2-inventory.json` and
`run-delegation-driver-inventory.json` in the external verification workspace.
