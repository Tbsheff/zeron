# Zeron Orchestrator v2: design

Status: draft for Tyler's review (AC-P3). Satisfies `ACCEPTANCE.md` (v2, 2026-11-08).
Sources: T3 clone `/tmp/t3code-research/repo` @ 8a84e942 (MIT, © 2026 T3 Tools Inc.; keep the notice on any file that ports T3 code). Model 1: `../tla/Orchestration.tla` and `../tla/README.md`. Zeron: `~/code/zeron-wt/plugin-platform` @ `d1f3d825` (feat/mcp-redesign). Line cites are for that commit.

## 1. Summary

The orchestrator is a pure reducer plus a small engine driver.

- **Core.** A new crate, `crates/orch`, holds `step(&State, &Event) -> (State, Vec<Effect>)` (AC-I0). It has no I/O, no tokio and no clock. Each `match` arm is one TLA+ action and carries the same name.
- **Driver.** A new engine module, `crates/engine/src/orchestrator/`, turns Zeron inputs into events. For each event it commits, in one SQLite transaction: the receipt, the new state, the effects into the outbox, and one trace record. Then an effect worker runs the outbox.
- **Partitions.** Each chat's state lives on that chat's host device (`Chat.device_id`, `entities.rs:262`). No device holds global state. Hosts talk only through messages in session docs, and those docs ride the existing durable chat2 sync.
- **Placement.** Zeron has no host move today. This design adds none.

```
 person / UI            agent (harness)                 MCP server (zeron mcp)
     │ QueueCommand          │ AgentEvent stream             │ RPC: Delegate, Tasks, Transfer
     ▼                       ▼                               ▼
 ┌──────────── host engine of chat T (one per chat; S9) ─────────────────────────────┐
 │ doc_host.drain_commands ─┐   sessions.rs Done/Subagent ─┐   rpc.rs + agent_gate ─┐ │
 │                          ▼                              ▼                        ▼ │
 │            orchestrator::driver  ── Event ──►  orch::step (pure)                  │
 │                 │  one SQLite txn: receipt + state + outbox + trace(seq)          │
 │                 ▼                                                                 │
 │          docs.sqlite3: orch_state │ orch_outbox │ orch_receipts │ orch_timers     │
 │                 │                                                                 │
 │       effect worker: StartRun/Steer/Interrupt → SessionsEngine → Harness::run     │
 │                      AppendPart → T's session doc (task chips, wake, handoff)     │
 │                      WriteDown → child doc `commands` (Orch payload)  ─┐          │
 │                      PublishUp → T's own doc `orch_out` list          ─┤          │
 └────────────────────────────────────────────────────────────────────────┼──────────┘
                                                                          ▼
                        chat2 ChatRoom DO per chat (durable rows, batch_id dedupe)
                                                                          ▼
                    host engine of child C / parent P (other device or Cloud)
```

**Persistence**

| Data | Where | Scope |
|---|---|---|
| Reducer state for chats this device hosts: runs, attempts, queue order, tasks, cohorts, waits, questions, transfers | `docs.sqlite3`, new tables (migration in `crates/sync/src/store.rs`) | per device |
| Outbox, receipts (by message id), timers, trace sequence | `docs.sqlite3` `orch_outbox`, `orch_receipts`, `orch_timers`, `orch_meta` | per device |
| Trace log (AC-I1) | `{store}/orch-trace/{device}.jsonl` | per device |
| Downward messages (stop, answer, clamp, follow-up) | child session doc `commands`, new payload `Orch`, no TTL | synced |
| Upward messages (notice, question, question-closed) | sender's own session doc, new Loro list `orch_out` | synced |
| Views: task chips, wake messages, handoff headers | session doc `messages` (new `MessagePart`s) | synced |
| Lineage, config/access, run status | registry chat and session rows (+ `depth`, `taskOf`) | synced |

Why `docs.sqlite3`: it already holds `processed_commands` and `chat_outbox`. One file lets one transaction cover the receipt, the state and the outbox. Update-trial rollback restores SQLite stores but not synced docs (ARCHITECTURE §1). So recovery must not trust ledger marks: it rescans entries that have no receipt and re-applies them. Deterministic ids make that idempotent.

## 2. Concept map

| T3 entity | Zeron type | File / module | Change |
|---|---|---|---|
| AppThread (lineage parent, fork, subagent, side) | `Chat` + `parent_chat_id` + `child_kind` | `proto/src/entities.rs:259,318,332` | Add `depth: u8` and `task_of`. Persist `ChildKind` (today it is derived). |
| Run (ordinal, queueHeld, contextHandoffId, delegatedTaskId) | new `orch::Run {chat, ord, kind, status, transfer, task}` | `orch/src/run.rs` | New. `SessionStatus` stays the UI's view. |
| RunAttempt (initial, steering_restart, retry, provider_recovery) | new `orch::Attempt {ord, n, reason}` | `orch/src/run.rs` | New. Today `.resume` in `run_journal.rs` counts attempts. |
| ExecutionNode: root_turn / tool_call / approval / user_input | the run; `MessagePart::Tool`, `Input` | `doc/src/parts.rs:258,320` | Only the root ends a run (S2). |
| ExecutionNode: subagent | `orch::Task {origin: Native}` + `Tool.subagent_ref` | `orch/src/native.rs` | New (§6). |
| Subagent / delegated task (origin, completionWake, completionDelivery) | `orch::Task {id: (child, child_ord), origin, wake: Always│SettledOnly, delivery}` | `orch/src/delegation.rs` | New. Task id = (child chat, child run ordinal), so a follow-up is a new task. A child has at most one open task: a follow-up is accepted only after the previous task is handled (Delegation `FollowUp`, flag `FollowUps`). |
| Delivery cohort (`parentRun.delegatedCompletion`) | `orch::Cohort {parent, ord, disp, st, del, gen}` | `orch/src/delegation.rs` | New. |
| ProviderSession / Thread / Turn; ProviderRef (strong / weak / none) | harness runtime + `Done.session_id`; `orch::ProviderRef` | `harness/src/lib.rs:262`, `agent.rs:1178` | New ref on Run. Strong = session id came from this harness's own Done. |
| RuntimeRequest (user_input; live / message / not_resumable) | `MessagePart::Input` + `RespondInput` | `parts.rs:320`, `commands.rs:39` | Questions get `qid` and `epoch` (S12). |
| ContextTransfer + SourcePoint | `orch::Transfer {kind, src, base, point, target, status}`; a point = (chat, message id, Loro frontier) | `orch/src/transfer.rs` | New. Replaces `fork_history_prompt` (`sessions.rs:3096`). |
| ContextHandoff (strategy, delivery) | `MessagePart::Handoff {transfer, strategy, coverage}` | `parts.rs` | New part. `Switch` (`:360`) stays the seam marker. |
| Fork / merge-back | `MessagePart::Fork` + `base_message_id`; new `MergeBack` | `parts.rs:343`, `rpc.rs:3140` | The fork handler records its base point. |
| Commands, domain events, EventSink, projections, UsageLimitRecoveryWorker | `orch::Event` (about 30 variants), trace records, derived doc parts and registry rows, `TimerFired` | `orch/src/event.rs`, `orchestrator/{store,trace,project,timers}.rs` | One transaction per event. Projections are rewritable from state. |
| Effect outbox, command receipts | `orch_outbox(effect_id PK, …)`, `orch_receipts(msg_id PK, chat, seq)` | `orchestrator/store.rs` | Deterministic ids, `ON CONFLICT DO NOTHING`. Receipts are the source of truth for "processed". |
| `handoffBudget.ts`, `selectHistory`, `subagentResultForRun` | `orch::transfer::budget`, `result_text` | `orch/src/transfer/budget.rs`, `orchestrator/project.rs` | Port, with the MIT notice. 16 KB inline per wake. |
| MCP `delegate_task`, `task_status`, `task_cancel`, `t3_thread_transfer` | `create_chats` (+mode), `tasks {action}`, `transfer {action}` | `crates/mcp/src/tools/{defs,tasks,transfer}.rs` | §7. |

## 3. State machines

These tables are the contract. The TLA+ modules and `orch::step` both use these action names. Names from model 1 are kept verbatim. Rows marked **Δ** differ from model 1 and must be re-checked in slice 0; they are not yet verified.

**Locality rule.** Every guard reads only the partition of the host that runs the step (§4). A decision that needs another host's state is split into two steps: the sender *publishes*, and the receiver *decides at ingest*. This is what makes trace merge valid.

Notation: `t` is a chat, `c` a child, `r` a run ordinal, `g` a generation. "EndRun" is the shared tail of RootComplete, RootFail and ConfirmInterrupt.

### 3.1 Run (module `RunLifecycle`)

| # | Action | From | Guard | Next | Effects |
|---|---|---|---|---|---|
| RL1 | PersonSend(t) | any | queue not full (MaxQueue bounds person and server sends only; task, wake, resume and question messages are never refused) | queue += person msg; held := none (any reason) | — |
| RL2 | StartFromQueue(t) | idle | ¬Live ∧ ¬held ∧ head not gated | stale wake (cSt≠queued ∨ gen≠g): drop, stay idle. Else live, rord+1, attempt 1, stopMark := F, waitOn := ∅. A wake accepts here (T6). | StartRun{t, r, msgId, transfer?} |
| RL3 | Steer(t, m) **Δ** | live | ¬stopMark ∧ `supports_steering` ∧ row not `hold_for_turn_end` | live (same r) | SteerRun |
| RL4 | RestartActive(t, m) **Δ** | live | "send now" | live, attempt+1 (steering_restart) | InterruptAttempt; StartAttempt |
| RL5 | RetryAttempt(t) **Δ** | live | attempt failed, retryable class, n < MaxAttempts | live, attempt+1 (retry) | StartAttempt |
| RL6 | RequestInterrupt(t) **Δ** | live | interrupt_chat or ThreadStop | stopMark := T | InterruptRun (R7: a request, not an end) |
| RL7 | ConfirmInterrupt(t) | live ∧ stopMark | `Done{Interrupted}` for r | interrupted; EndRun | EndRun |
| RL8 | RootComplete(t) | live | waitOn = ∅ ∧ ¬Blocked(t) ∧ `Done{Completed}` | completed; EndRun | EndRun |
| RL9 | RootFail(t, cls) | live | `Done{Errored, class}` | failed; held := fail, or limit if cls = usage_limit (a stop hold is never downgraded). cls = usage_limit ∧ held ≠ stop ⇒ arm the resume for this run (ulArmed := r; per run, not once per thread). If Stop already reached the run, keep the stop hold and arm nothing (R24). A later run start disarms it. EndRun | EndRun; ArmTimer{t, "ul", reset_at} unless stopped |
| RL10 | EndRun(t, out) | — | — | waitOn := ∅ (R2); stopMark := F; qPend[t] := F; Q8 catch-up; N3 orphans. **Δ Sweep: if cDisp[t][r] = open, Plan(t, r, settled ∧ pending, liveNow=F) for every outcome, including interrupted.** Model 1 sweeps only on completed and failed; Zeron's `interrupt_chat` disposes nothing, so without this its results strand (R1). | Notice{t, r} up if rtask; Offer if Plan reserves a generation |
| RL11 | UsageLimitRecovery(t) | idle | ulArmed = rord ∧ last outcome usage_limit ∧ TimerFired(t, "ul") ∧ held ∈ {none, limit} (not "¬held": T3 holds the queue on a usage limit, so ¬held makes F9 unreachable) | start the resume run at once (start_immediately, server-created, fixed id; it does not enter the queue); rord+1; held: limit → none (D6: a held person message runs after it); ulArmed := 0 | StartRun{t, r, resume} |

- **Failure class.** F9 needs `FailureClass {UsageLimit{reset_at?}, Provider, Transport, Permission, Validation, Unknown}` on `Done`. Today `Done.error` is a plain string (`agent.rs:1178`), and Claude's rate-limit event becomes an `Error` string (`claude/normalize.rs:1227`). Each harness adds a classifier. F9 rests on "harnesses report their own events correctly". With no `reset_at`, the timer tries at 15 min, then 60 min.
- **Clocks.** The timer driver owns time. `step()` sees only `TimerFired`, and the trace logs only that event.

### 3.2 Task: delegation and delivery (module `Delegation`)

States: tSt ∈ {running, settled}. tDel ∈ {pending, claimed, delivered, acknowledged, disposed}. Cohort cSt ∈ {none, offered, queued}, cDisp ∈ {open, stopped}. All rows run on host(parent).

| # | Action | Guard | Next | Effects |
|---|---|---|---|---|
| T1 | Delegate(t, c, mode, a) | Live(t) ∧ ¬stopMark ∧ waitOn = ∅ ∧ ¬Blocked(t) ∧ depth+1 ≤ MaxDepth ∧ \|UnsettledKids(t)\| < MaxFanout **Δ** ∧ a ≤ access[t] ∧ clientRequestId unseen. mode = wait ⇒ one request per call. | task (c, 1) running; tWake := settled_only if wait, else always; tCoh := rord; waitOn := c if wait; queue[c] := ⟨task⟩ | CreateChild{c, device, config, ceiling, prompt}. The caps are an engine guard (R3). |
| T2 | DoNotice(Notice(c, r)) | no receipt `notice:c:r` ∧ tSt ≠ settled | tSt := settled; Plan(p, tCoh, {c} if pending, CohortLive) | Offer if reserved |
| T2' | DoNotice duplicate | receipt present or settled | no-op (R6) | — |
| T3 | Plan(p, r, S, live) | cDisp = open | S minus (settled_only ∧ live) → claimed; join the offered/queued wake, or g+1 and cSt := offered. Handled tasks are skipped. | Offer{p, r, g} |
| T3' | Plan, cohort stopped | cDisp = stopped | S → disposed | — |
| T4 | DoOffer(Offer(p, r, g)) | cSt = offered ∧ cGen = g | queue[p] += wake(r, g), msgId `wake:p:r:g`; cSt := queued | — |
| T4' | DoOffer stale | generation or state differs | no-op (R6) | — |
| T6 | StartFromQueue(p) on wake(r, g) | not stale | claimed → delivered (pushes+1); settled pending siblings → g+1, offered | StartRun with a `TaskResults` part: ≤ 16 KB inline, rest as "read with `tasks status`" |
| T7 | ReadResult(p, c) | Live(p) ∧ ¬Blocked(p) ∧ settled ∧ tDel ∈ {pending, claimed} ∧ ack observed (below) | acknowledged; remove from wake; cancel an emptied wake | — |
| T8 | WaitReturn(t) | waitOn = c ∧ Live(t) ∧ (settled ∨ question seen) | settled: Ack(c). Question: qSeen, tWake := always. waitOn := ∅ | CompleteWait{call, result} |
| T9 | WaitTimeout(t) | waitOn = c ∧ unsettled ∧ no question | tWake := always; waitOn := ∅ | CompleteWait{call, timed_out} |
| T10 | WaitAbandon(t) | waitOn ≠ ∅; tool call cancelled or MCP gone | waitOn := ∅ | — |
| T11 | LateWaitAck(t) | a handler outlives its run | no-op (R2) | — |

**When an ack counts (S7).** The ack must come from p's live run r, as seen in p's own harness stream:

- Harnesses that emit `ToolCall::Mcp{server, tool, input}` and its `ToolResult{id, is_error}` are Claude, Codex, Cursor, Pi and Muse. For these, the ack counts when the stream shows a successful `ToolResult` for the matching `zeron` call (same tool, same canonical input hash) during live run r.
  - The result *text* is not used. Claude and Codex forward command output only (`agent.rs:1107`).
  - In ZeronImpl, WaitReturn becomes `WaitReply` then `WaitObserved`. A crash between them maps to WaitAbandon, and the RL10 sweep then delivers the result by wake.
- ACP and OpenCode do not emit `ToolCall::Mcp` today. For them, the ack counts when the MCP reply is written, which is T3's rule. FP5 can then lose a reply after it was counted.
  - This is a stated assumption per harness, `reports_mcp_results`, listed in §12. Slice 4 adds `ToolCall::Mcp` to both normalizers where the wire carries it.

**Fan-out Δ.** Model 1 counts every child ever created, `Kids(t)`. That ends long sessions early, so v2 counts unsettled tasks. The spec guard changes with it (open question 2).

### 3.3 Question (module `Delegation`, flag `Questions`)

Each question has `qid` (the `Input` request id) and `epoch`. Only (qid, epoch) can answer. One agent answerer holds an epoch. The person may always answer from the child's UI, and the first applied answer wins.

| # | Action | Host | Guard | Next | Effects |
|---|---|---|---|---|---|
| Q1 | RaiseQuestion(c) | c | Live(c) ∧ rtask ∧ waitOn[c] = ∅ ∧ ¬stopMark | qPend := T | QuestionUp{c, qid, epoch} via `orch_out` |
| Q2 | IngestQuestion(p, c) **Δ** | p | receipt `q:c:qid:e` absent | wait on c active (waitOn[p] = c): mark seen-by-wait, no wake (R8a). p Blocked: republish up to its parent (R8b). Else qWake[p] += c. | QuestionUp (republished) if escalated |
| Q3 | StartQuestion(t) | t | ¬Live ∧ ¬held ∧ qWake[t] ≠ ∅ | qSeen; live run (rord+1) | StartRun with the question part |
| Q4 | WaitReturn (question branch) | p | see T8 | qSeen; tWake := always | CompleteWait |
| Q5 | AgentAnswer(x, c) | x | qSeen ∧ Live(x) ∧ ¬Blocked(x) ∧ x holds the epoch | — | AnswerDown{c, qid, epoch}, hop by hop |
| Q6 | ApplyAnswer(c) **Δ** | c | qPend ∧ (qid, epoch) current | qPend := F | RespondInput on c; QuestionClosed up |
| Q6' | ApplyAnswer stale | c | qid closed or epoch old | no-op (S12: applied once) | — |
| Q7 | PersonAnswer(c) | c | qPend | qPend := F | QuestionClosed up (retracts the answerer's view) |
| Q8 | EndRun catch-up | p | any run end (completed, failed or interrupted; R22) where the spawning run's questions were seen only by its wait | qWake[p] += those | — |
| Q9 | Reroute(p) **Δ** | p | p becomes Blocked (Q1 on p) or its question run ends unanswered | epoch+1 for each unanswered question in qWake[p] or held by p | QuestionUp (new epoch) to p's parent; Retract |
| Q10 | IngestClosed(x, c) | x | — | drop c from qWake; clear the view | — |

When escalation passes the root, the question goes to the person (a desktop notification, `desktop_notify.rs`).

### 3.4 Context transfer (module `ContextTransfer`)

xSt ∈ {pending, resolved_native, resolved_portable, failed, consumed, superseded}. All rows run on host(target).

| # | Action | Guard | Next | Effects |
|---|---|---|---|---|
| X1 | CreateTransfer(k, src, base, point, tgt) | point is a message id that exists in src; base ≤ point | pending; range (base, point] pinned | — |
| X2 | ResolveNative(x) | same harness ∧ ProviderRef strong ∧ native_resume or native_fork | resolved_native | — |
| X3 | ResolvePortable(x, strat) | otherwise | resolved_portable; sel ⊆ range; sel ≠ range ⇒ coverage header | — |
| X4 | FailTransfer(x, why) | source missing, budget 0, or summarizer failed | failed(why) | error part in the target (L5) |
| X5 | ConsumeTransfer(x) | target StartFromQueue ∧ resolved_* | consumed | StartRun carries the packet + a `Handoff` part |
| X6 | SupersedeTransfer(x) | a newer transfer for the same target, still pending | superseded | — |

S8: every packet item has base < id ≤ point, with no item twice. Resolution happens lazily at run start but always reads the pinned range.

### 3.5 Stop cascade (modules `Delegation` + `Placement`)

| # | Action | Guard | Next | Effects |
|---|---|---|---|---|
| S-1 | Stop(t) | created | ThreadStop(t) | — |
| S-2 | ThreadStop(t) | — | stopMark := Live(t); held := T; undelivered tasks → disposed; cohorts r ≤ rord → stopped; drop wakes; drop children's questions from qWake[t] | RequestInterrupt if live; Stop{c, e} down per child (durable, R5) |
| S-3 | DoStop(Stop(c, e)) | receipt `stop:c:e` absent | ThreadStop(c) (recursive) | as S-2 |
| S-4 | Delegate after stop | stopMark[t] | refused (R7, S11) | MCP `rejected` |
| S-5 | PersonSend(t) | — | held := F (resumes t only) | — |

Native subagents end with the parent's interrupt (N3).

### 3.6 Native subagent (module `NativeSubagents`)

| # | Action | Guard | Next | Effects |
|---|---|---|---|---|
| N1 | NativeStart(t, k) | Live(t) ∧ a spawn `ToolCall` k exists ∧ first `AgentEvent::Subagent{parent_tool_use_id: k}` | task (native, k) running | open doc `{t}--sub--{k}` (`sessions.rs:3436`) |
| N2 | NativeEnd(t, k) | running ∧ (spawn `ToolResult` or nested Done) | settled; tDel := delivered (inline in t's own run) | chip Done or Failed |
| N3 | NativeOrphan(t, k) | EndRun(t) ∧ k running | closed (orphaned) | chip Failed "parent run ended" |

S2/S13: N2 and N3 never change rst[t].

## 4. Placement and multi-device (module `Placement`)

**Who runs the reducer.** Each chat's host device runs the reducer for that chat. The host is `Chat.device_id`. A tree may span a laptop, a VPS and Cloud session devices.

| State | Owner |
|---|---|
| run, queue, held, stopMark, waitOn of t | host(t) |
| tasks whose parent is t, cohorts of t's runs | host(t) |
| qPend of c | host(c) |
| qWake and epochs at each hop | that hop's host |
| transfers whose target is t | host(t) |

**Gate.** `WorkspaceHost::is_host` (`workspace_host.rs:834`) returns true for a chat with no row and on a registry read error. The orchestrator ingests only on a positive check: the row exists and its device equals this device (S9).

**Channels.** Every message goes into a doc the receiver can read. A Cloud runner may open `/chat2/{id}` only for chats it hosts and for children of chats it hosts (`cloud-device.md:141,182`). So a parent can write into a child's room, but a Cloud child cannot write into its parent's room.

| Message | Direction | Into | Read by |
|---|---|---|---|
| CreateChild | down | registry row (Cloud: edge `createChildSession`) + child doc first `Run` | child host |
| Stop, AnswerDown, Clamp, FollowUp | down | child doc `commands`, payload `Orch`, `expires_at = i64::MAX` | child host (drain) |
| Notice, QuestionUp, QuestionClosed | up | sender's own doc `orch_out` (append-only) | parent host, which joins its children's rooms |

Escalation and answers move one hop at a time, so each host reads only its children's rooms.

**Ids and dedupe.**

- **Ids.** Every message id is deterministic: `create:c`, `notice:c:r`, `stop:c:e`, `q:c:qid:e`, `ans:c:qid:e`, `clamp:c:e`, `offer:p:r:g`.
- **Sender.** The effect worker checks the target doc for the id before it appends.
- **Receiver.** It commits `orch_receipts(msg_id)` in the same transaction as the step, then marks the ledger entry processed. This is receipt-then-mark, not the ledger's mark-then-execute (R6, S10).
- **Recovery.** On startup, and after an update rollback, the host rescans `commands` and its children's `orch_out` for ids that have no receipt.
- **Duplicates.** ChatRoom `batch_id UNIQUE` and Loro's idempotent import absorb duplicate rows.

**Offline.**

- **Child host offline or asleep.** Downward entries wait in the ChatRoom. Cloud wakes on the existing command nudge.
- **Parent host offline.** Notices wait in the child's `orch_out`, and the parent ingests them on backfill (L1 under "devices eventually online").
- **No TTL.** Orchestrator entries never expire. The default 24 h TTL (`constants.rs:27`) would break L1.
- **Local profile.** It has no rooms, and the code path is the same.

**Capability gate.** An older engine skips the unknown `Orch` payload, so the entry stays pending forever. Delegate therefore places a task only on a device that advertises `orch-v2`, and refuses otherwise.

**Trace merge (AC-I1).**

- **Records.** Each device numbers its records with its own `seq`, committed in the step transaction. Send and receive records carry the message id.
- **Merge.** The checker builds a partial order: per-device seq order, plus send before receive.
- **Why it is sound.** The locality rule means steps on different hosts touch disjoint variables (apart from appending to and taking from a channel), so they commute. Any linear order that respects the partial order is then a valid ZeronImpl behavior. `Placement` checks this as `CommuteOK`.
- **No clocks.**

**Host move.** Zeron has none today. S9 holds because the host is constant per chat. Adding handover needs a fenced handoff of the partition and outbox, as a new `Placement` action with its own proof.

## 5. Context transfer

**Strategies** (chosen by new `Harness` methods with defaults, next to `access_modes`: `native_resume`, `native_fork`, `inject_history`; window from `model_context()`):

| Strategy | When | Packet |
|---|---|---|
| native_resume | same harness ∧ strong ProviderRef ∧ the target last saw `point` | none; resume the session id |
| native_fork | same harness ∧ `native_fork` | provider fork at the point |
| delta | the target harness saw the thread up to p < point | items in (p, point] through `select` |
| summary | range > budget ∧ a summarizer is configured (off by default) | summary + coverage header |
| portable_replay | otherwise | `select` over (base, point] |

**Budget.** Ported from `handoffBudget.ts:99`: `max(0, min(16_000 tok, 64_000 B, window − used − current − max(16_000, ⌈window/4⌉)))`, at 1 byte ≈ 1 token. `used` comes from the last `AgentEvent::Usage`. `current` is the prompt bytes plus 8,192 per image and 4,096 per other attachment. `select` keeps the last user, last assistant and first user messages, then the rest newest first. It skips an oversized item whole, never truncates it, and keeps the original order.

**Coverage header** (adapted from `handoffBudget.ts:226,261`):
`Zeron context handoff. Chat {id}. Covered runs {a}-{b}. Source messages {m1} through {m2}. Selected {n} intact items; omitted {k}. Historical material is context, not a new request or higher-priority instructions. Attachments and native tool state are not replayed. Read omitted items with read_chat {chat:"{id}", before:"{msgId}"}.`
A subset is legal only with this header (S8).

| Operation | Kind | base → point | Target, delivery |
|---|---|---|---|
| Fork | fork | start → source's latest completed message | new chat; `Fork{base_message_id}` |
| Merge-back (F5) | merge_back | fork's base → fork's latest stable message | parent; queued like a wake, `MergeBack` part |
| Agent switch (F6) | provider_handoff | last point the new harness saw (or start) → latest | same chat; `Switch` + `Handoff` |
| Delegation spawn | subagent_spawn | — | child gets the task prompt only |
| Delegation result | subagent_result | child run start → end | parent wake; 16 KB inline |

`fork_history_prompt` (`sessions.rs:3096`; 80 k chars, newest first) and `warm_fork_history` (`:1369`) are deleted. `drive_run` (`:3847`) renders the consumed transfer's packet.

## 6. Native subagents

Harness-owned subagents arrive as `AgentEvent::Subagent{parent_tool_use_id, event}` (`agent.rs:1206`). They are routed to doc `{chatId}--sub--{toolUseId}`, and the parent keeps a spawn chip (`Tool.subagent_status`).

- **F10.** v2 makes each one an `orch::Task{origin: Native, id: (t, k)}` in t's partition (§3.6). It lists in `tasks list` and the lanes card, with the same status words as app-owned tasks.
- **Addressing.** Native tasks are not chats. They have no queue and cannot be sent to. `read_chat` on the `--sub--` id reads them. T3 gives them child threads; Zeron keeps them as docs because the harness owns their lifecycle.
- **Delivery.** Inline to the parent's own run, as the tool result. No wake and no cohort.
- **S13.** N2 and N3 never touch rst.
- **Caps.** Zeron observes native subagents but cannot refuse them, so S5 caps apply to app-owned tasks only. A native subagent that calls Zeron MCP runs as its parent (`ZERON_CHAT_ID`), so the parent's caps and stopMark apply.

## 7. MCP surface

This adds two action-style tools (18 total) and extends `create_chats`.

| Tool | Input (new fields) | Result (new fields) |
|---|---|---|
| `create_chats` (kind `subagent`) | `mode: async│wait` (wait ⇒ exactly 1 request), `timeoutMs` (default 10 min), `access` (narrow only), `isolation: shared│worktree`, `clientRequestId` | per item: `taskId`, `status`, `result?`, `waitTimedOut?`, `next` |
| `tasks` | `action: status│list│cancel`, `taskId?`, `reason?`, `clientRequestId?` | `{taskId, chatId, origin, status, delivery, result?, hasPendingQuestion, next}`; cancel → `cancel_requested│<terminal>` |
| `transfer` | `action: fork│merge_back│status`, `chat`, `sourcePoint?: latest_stable│{messageId}`, `target?`, `title?` | `{transferId, targetChatId, status, coverage?}` |
| `send_messages` to own child | unchanged | adds `taskId` (a follow-up task) |
| `wait_for_turn` on a task chat | unchanged | task status and `next`; never acknowledges |

- **`next` strings.**
  - async: `"Results arrive in your next turn as a task-results message. End your turn or keep working; do not poll."`
  - wait timeout: `"Still running. The result is pushed to you when it settles; do not poll."`
  - stopped: `"This chat was stopped; delegation is refused until the person sends again."`
- **Idempotency.** `clientRequestId` is persisted in `orch_receipts` as `mcp:{originChat}:{op}:{id}`. Today it is kept per connection. A repeated key returns the first task (S10 across reconnects).
- **Caps.** Checked in `step` (T1), so every caller gets them: MCP, plugins (`plugins/actions.rs`) and the UI. The one-level rule (`tools.rs:786-813`) is removed under the flag.
- **Access.** `cap_access` (`agent_gate.rs:73`) still caps at creation. T1 repeats `a ≤ access[t]`. NarrowAccess (a person's config change) sends `Clamp{ceiling, e}` down, hop by hop (R4).

## 8. Migration

- **Flag.** `orchestrator_v2` engine setting (`ZERON_ORCH_V2=1`), off by default. While it is off, today's paths run unchanged: the one-level rule, polling and `fork_history_prompt`.
- **Legacy children.** Existing chats that have a `parent_chat_id` but no task row stay readable. They get no push. Existing chats enter the reducer at their next command.
- **Capability.** A device advertises `orch-v2` only while the flag is on, and tasks are placed only on such devices. The edge's `createChildSession` drops `too_deep` for v2 runners and checks depth from the registry; this ships before the flag defaults on. Old UI builds must skip unknown `MessagePart` kinds; slice 4 adds a decode test.
- **Rollback.** Turn the flag off. Pending `Orch` entries stay inert. Tasks in flight show `failed: orchestrator disabled`.

## 9. Verification plan

### 9.1 TLA+ modules and bounds

Model 1 reaches 1.8M–8.6M distinct states at 3 threads. A 4-thread, depth-2 run did not finish in 25 min (`out/measure4.txt`). So bounds are set per module, and `Composition` relies on simulation.

| Module | Content | Exhaustive bound | N+1 (AC-M3) |
|---|---|---|---|
| `Delegation` | model 1 + Δ rows (T1, RL10, Q2, Q6, Q9) | threads 3, depth 2, runs 8, gen 4, budgets 1 | threads 4, P1 S1 C1 and others 0, SYMMETRY |
| `RunLifecycle` | RL1–RL11 | 1 thread, queue 3, runs 4, attempts 2 | queue 4 |
| `ContextTransfer` | X1–X6, budget as item count | 2 threads, 4 msgs each, 2 harnesses | 5 msgs |
| `Placement` | hosts, channels, receipts, offline, crash, `CommuteOK` | 3 chats, 2 devices + cloud, dup/crash/offline 1 | 3 devices |
| `NativeSubagents` | N1–N3 under RL | 1 thread, 2 natives, 2 runs | 3 natives |
| `Composition` | all, sharing rst and queue | 2 threads, 2 devices, budgets 1 | simulation: 4 threads, depth 3, 3 devices, 10⁶ traces |
| `ZeronImpl` | ledger, `orch_out`, receipts, per-host outbox, held queue, journal, WaitReply/WaitObserved, enforced access | 2 threads, 2 devices | 3 threads (safety) |

- **Proofs (AC-M4, tlapm).**
  - S3: inductive; a delivered task has pushes = 1 and is never disposed.
  - S10: restated as `ReplayIsStutter` (§12).
  - S9: host is constant per chat.
- **Mutants (AC-M6), one guard removed each.** S1: ¬Live in StartFromQueue. S2/S13: NativeEnd sets rst. S3: settle receipt. S4: gen check in DoOffer. S5: DepthCap. S6: Clamp never sent (caught by `EnforcedAccessOK` in ZeronImpl). S7: live-run check on ack. S8: resolve at point+1. S9: permissive is_host. S10: receipt check. S11: ¬stopMark in Delegate. S12: epoch check. L1: RL10 sweep. L2: in-memory outbox. L3: held never cleared. L4: Q2 escalation. L5: FailTransfer. L6: wakes only after questions.
- **Enforced access (S6, R4).** The abstract mapping makes access effective at once (§9.2), so ZeronImpl needs its own checks.
  - `EnforcedAccessOK` (safety): every StartRun, StartAttempt and gate decision on d uses access ≤ min(config[d], last ceiling d received).
  - `CeilingConverges` (liveness): d's ceiling eventually equals min over its ancestors.

### 9.2 Refinement ZeronImpl ⇒ Composition (AC-P2)

| Composition variable | ZeronImpl expression |
|---|---|
| created[c], parent, depth | task row in host(parent) ∧ registry row |
| access[d] | min{config[x] : x ∈ {d} ∪ Anc(d)} |
| queue[t] | CreateChild pending ⇒ ⟨task⟩; else host(t) ordered queue (Loro rows ⊕ internal wake/resume/question msgs) |
| held, rst, rord, rtask, stopMark, waitOn | host(t) run row; waitOn ≠ ∅ from Delegate until WaitObserved |
| tSt[c] | settled ⇔ host(parent) has receipt `notice:c:r` |
| tWake, tDel, tCoh, pushes | host(parent) task row; pushes is a ghost |
| cDisp, cSt, cDel, cGen | host(t) cohort rows |
| qPend | host(c) |
| qSeen, qWake | the deciding host's rows only. An unreceipted `q:*` entry is in flight, not in qWake. |
| outbox | ⋃ host outboxes ∪ unreceipted `notice:*` / `q:*` in `orch_out` ∪ unreceipted `stop:*` / `ans:*` in ledgers, deduped by id |
| xSt, range | host(target) transfer row |
| doomed, zWait | ghosts |

- **Two-step questions.** Composition models questions in two steps as well: `RaiseQuestion` publishes and `IngestQuestion` decides. The `Delegation` module carries the same split, so the mapping is direct.
- **Stutters.** Doc writes, Clamp delivery, projection writes and WaitReply map to stutters. A duplicate receive also maps to a stutter.
- **Scope.** Safety refinement (`Init ∧ [][Next]_vars`) is checked exhaustively. Fairness is not refined. L1–L6 and `CeilingConverges` are checked on ZeronImpl directly, under its own fairness.

### 9.3 Implementation checks

- **Trace schema (AC-I1).**
  - One JSON line per step: `{"v":1,"dev":"<id>","seq":n,"act":"DoNotice","args":{"c":"…","r":3},"chat":"…","msg":{"id":"notice:…","dir":"recv"},"cmd":"<ledger id>?","post":"<blake3 of partition>"}`. No time fields.
  - `tools/orch-trace` merges the logs (§4), replays them against ZeronImpl in `tla-ts` trace mode, and checks `post` through the mapping.
  - It runs on 100% of integration traces and on one dogfood week.
- **Model-based tests (AC-I2).**
  - The `tla-ts` state graph feeds a path-cover generator. Its JSON scenarios cover every action, each guard true and false (a false guard is asserted as a no-op or a refusal), and every mutant.
  - The runner drives 1–3 in-process engines over an in-memory ChatRoom fake, with a `ReplayHarness` built on `harness/src/mock.rs`.
  - After each step it compares state through §9.2.
- **Fault injection (AC-I3).**

  | Point | Fault |
  |---|---|
  | FP1 | step committed, effect not run |
  | FP2 | doc entry written, outbox not marked done |
  | FP3 | entry received, no receipt yet |
  | FP4 | receipt committed, ledger not marked |
  | FP5 | WaitReply sent, not yet observed |
  | FP6 | Done read, journal not written |

  Also: duplicated and reordered ChatRoom rows; a device offline and back; Cloud asleep; usage-limit `Done` with and without `reset_at`.
- **Kani (AC-I4).** In `crates/orch/src/proofs.rs`: assume `inv(s)` on a bounded `State` (≤ 3 chats, ≤ 3 runs, ≤ 2 gens), take any `Event`, and assert `inv(step(s, e).0)` for S1, S3, S5 and S6. Proptest runs the same invariants over random sequences of length ≤ 40.
- **CI gate (AC-I5).** Job `orchestrator-verify`. On every push to the orchestrator branch it runs: spec checks at bounds, coverage, mutants, tlapm, `cargo test -p zeron-orch`, MBT, trace validation and Kani. Any failure blocks. N+1 and the Composition simulation run nightly (open question 5, §12).

## 10. Traceability

Short names: Del = Delegation, RL = RunLifecycle, CT = ContextTransfer, Pl = Placement, NS = NativeSubagents, Comp = Composition, ZI = ZeronImpl. Rust paths are in `orch/src/` (core) or `engine/src/orchestrator/` (driver).

| ID | Rows | Spec | Rust | Test |
|---|---|---|---|---|
| F1 | T1–T6 | Del witness `GrandchildResultAtRoot` | delegation.rs | MBT depth-2 |
| F2 | T3, T4, T6 | Del `WakeWithoutRead` | delegation.rs, project.rs | e2e: no read call |
| F3 | T3 join | Del `TwoInOneWake` | delegation.rs | MBT |
| F4 | T1 wait, T8 | Del `WaitGetsResult` | delegation.rs, mcp | MCP integration |
| F5 | X1–X5 merge_back | CT `MergeDeltaDelivered` | transfer.rs | fork + merge-back e2e |
| F6 | X2/X3 handoff | CT `SwitchWithPacket` | transfer/budget.rs | ReplayHarness switch |
| F7 | Q1–Q7 | Del `AnswerReturns` | question.rs | MBT |
| F8 | S-1–S-3 | Del, Pl | stop.rs | 3-level stop e2e |
| F9 | RL9, RL11 | RL `UsageLimitResumed` | run.rs, timers.rs | fake usage-limit Done |
| F10 | N1–N3 | NS `NativeInTaskList` | native.rs | Claude replay fixture |
| F11 | §4 channels | Pl `RemoteChildResult` | channel.rs | 2 engines + cloud fake |
| R1 | RL10 Δ | Del `NoStrandedResult` | run.rs | FP5, MBT |
| R2 | T7, T8, T11, RL10 | Del `NoPhantomAck` | delegation.rs | FP5 |
| R3 | T1 | Del `DepthBound` | delegation.rs | Kani S5 |
| R4 | NarrowAccess, Clamp | Del `AccessMonotone`; ZI `EnforcedAccessOK`, `CeilingConverges` | delegation.rs, channel.rs | MBT, Clamp mutant |
| R5 | S-2, S-3 | Del/Pl `NoOrphanRunning` | store.rs | FP1, FP2 |
| R6 | T2', T4', T3 | Del, Pl | delegation.rs, store.rs | duplicate rows |
| R7 | RL6, RL7, S-4 | RL `OnlyRootEndsRun` | run.rs | MBT |
| R8 | Q2, Q9 | Del `QuestionEventuallySurfaced` | question.rs | nested-question MBT |
| S1 | RL2 | RL `AtMostOneActiveRun` | run.rs | Kani |
| S2 | RL7–RL9 | RL `OnlyRootEndsRun` | run.rs | MBT |
| S3 | T2–T8 | Del `NoDuplicateDelivery`; TLAPS | delegation.rs | Kani, duplicate rows |
| S4 | T4, T6 | Del `NoWakeWhileLive` | delegation.rs | MBT |
| S5 | T1 | Del `DepthBound` | delegation.rs | Kani |
| S6 | T1, Clamp | Del `AccessMonotone`; ZI `EnforcedAccessOK` | delegation.rs, driver.rs | Kani, Clamp mutant |
| S7 | T7 ack rule | Del `NoPhantomAck` | delegation.rs | FP5 |
| S8 | X1, X3 | CT `ContextOrder` | transfer.rs | budget unit tests |
| S9 | §4 gate | Pl `OneHost`; TLAPS | driver.rs | permissive-host mutant |
| S10 | receipts, ids | Pl `ReplayIsStutter`; TLAPS | store.rs | FP1–FP4 |
| S11 | S-2, S-4 | Del `NoOrphanRunning` | stop.rs | MBT |
| S12 | Q5–Q7, Q9 | Del `OneAnswerer` | question.rs | MBT |
| S13 | N2, N3 | NS `NativeNeverEndsRun` | native.rs | replay fixture |
| L1–L6 | T3/RL10, S-3, RL2/RL11, Q2/Q9, X2–X4, T6 | `EveryResultEventuallyHandled`, `StopEventuallyQuiesces`, `QueueEventuallyDrains`, `QuestionEventuallySurfaced`, `TransferResolves`, `ParentEventuallyWoken`, on modules + ZI fairness | — | spec only; trace validation keeps the code inside ZI |
| AC-M1–M7 | — | 6 modules + ZI; §9.1 bounds, N+1, proofs, coverage, mutants, `-deadlock` with listed terminal states | — | CI + nightly |
| AC-P1–P3 | this doc, §9.2 | ZI ⇒ Comp | — | review, CI |
| AC-I0 | all | ZI actions | orch/step.rs | test: arm names = action names |
| AC-I1 | §4 merge | ZI | trace.rs | `tools/orch-trace` |
| AC-I2 | all | state graph | — | `crates/orch/tests/mbt_*` |
| AC-I3 | FP1–FP6 | ZI Environment | — | `engine/tests/orch_faults.rs` |
| AC-I4 | — | — | proofs.rs | Kani + proptest |
| AC-I5 | — | — | — | `orchestrator-verify` |

## 11. Build plan

Each slice merges behind the flag and passes its own checks.

| # | Slice | After | Verifies |
|---|---|---|---|
| 0 | Split `Orchestration.tla` into 6 modules; add the Δ rows; draft ZeronImpl; measure bounds; CI skeleton | — | AC-M1, M5–M7; Δ re-check |
| 1 | `crates/orch`: types, `step` for RL + Del (no questions); Kani, proptest; MBT on the core alone | 0 | AC-I0, AC-I4; S1, S3, S5, S6 |
| 2 | Store migration, outbox, receipts, trace, driver (one device); `FailureClass` in harnesses | 1 | S10, FP1–FP4, trace schema |
| 3 | Wire into `doc_host` and `sessions`: run lifecycle, interrupt as request, usage-limit timers | 2 | F9, R7, S2, L3 |
| 4 | Delegation on one device: tasks, cohorts, `TaskResults`, wait mode + ack rule, caps, clamp, stop; MCP `create_chats` mode + `tasks` | 3 | F1–F4, F8, R1–R6, S4, S7, S11 |
| 5 | Questions: epochs, ingest decisions, escalation | 4 | F7, R8, S12, L4 |
| 6 | Context transfer: budget port, strategies, `transfer` tool; delete `fork_history_prompt` | 3 | F5, F6, S8, L5 |
| 7 | Native subagents in the task model | 4 | F10, S13 |
| 8 | Multi-device: `Orch` payload, `orch_out`, child-room joins, `orch-v2`, edge depth rule | 4, 5 | F11, S9, Placement faults |
| 9 | Refinement in CI, trace validation on all tests, dogfood week, flag on by default | 8 | AC-P2, AC-I1, AC-I5 |

## 12. Criteria that cannot be met as written

| ID | Problem | Proposed handling |
|---|---|---|
| S5 | Native subagents are observed, not admitted. | Scope S5 to app-owned tasks. |
| S6 | A live provider process on another host keeps its old mode until it receives `Clamp` and reconfigures or restarts. | S6 holds on abstract state. ZeronImpl proves `EnforcedAccessOK` and `CeilingConverges`. The real-time lag is one relay delivery plus one restart. |
| S7 / S3 | ACP and OpenCode do not report MCP tool calls. Their acks count at reply time, so FP5 can lose a counted reply. | Assume it per harness (`reports_mcp_results`). Slice 4 closes the gap where the wire allows. |
| S10 | "Same state as running once" compares two executions; it is not a state invariant. | Restate as `ReplayIsStutter`: applying an effect or command whose receipt exists is a stuttering step. Prove that. |
| S12 | The person can always answer from the child's UI, so "exactly one visible answerer" is never true. | Exactly one agent answerer per epoch; the person always may; the first applied answer wins. |
| AC-M3 | Delegation's N+1 is 4 threads, the run that did not finish in 25 min. | Cut budgets at N+1 and use SYMMETRY, nightly, capped at 2 h. If it does not finish, report simulation coverage instead. |
| AC-P2 | `PROPERTY Abstract!Spec` includes fairness, and refining weak fairness through stutter-heavy mappings is costly. | Check safety refinement; check liveness on ZeronImpl directly. |
| AC-I5 | N+1 and Composition simulation take hours, too long for every push. | Every push: bounds, mutants, proofs, MBT, traces (about 20 min). Nightly: N+1 and simulation. |

## 13. Open questions

1. **MCP shape.** Extend `create_chats` with `mode`, or add a T3-style `delegate_task`? *Recommendation: extend `create_chats`, and add `tasks` and `transfer`, for 18 tools. Agents already know `create_chats`, and wait mode allows one request per call.*
2. **Caps.** What do the caps count, and what are the defaults? *Recommendation: MaxDepth 2 (root, child, grandchild). MaxFanout 8 counts unsettled tasks, not every child ever created (a Δ, re-checked in slice 0).*
3. **Narrowing a running descendant.** Apply it now or at the next run? *Recommendation: reconfigure in place where the harness supports it; otherwise restart the active attempt (RL4). Waiting leaves wide access on a live agent.*
4. **Acceptance amendments.** Accept the §12 rewordings (S5, S6, S7, S10, S12, AC-I5) as new IDs, such as S5a, with no renumbering? *Recommendation: yes. Also add IDs for the two Δ rules (RL10 sweep on every end, fan-out counts unsettled tasks).*
5. **CI compute.** Run the gate on a dedicated 16-core runner, with N+1 and simulation nightly? *Recommendation: yes. This is the AC-I5 change in §12.*

## Decisions (Tyler, 2026-11-08)

- **D1 MCP shape:** follow T3 Code. Add `delegate_task {task, target, title, role, isolation shared|worktree, mode async|wait, timeoutMs, clientRequestId}`, `task_status`, `task_cancel`, and a transfer tool (`fork | merge_back | status`), next to the redesigned tools. Result shapes and `next` hints follow T3's `orchestratorMcp.ts`.
- **D2 Caps:** MaxDepth 2 (root, child, grandchild). MaxFanout 8, counting unsettled tasks.
- **D3 Narrowing:** apply at once. Reconfigure in place where the harness supports it; otherwise restart the active attempt now.
- **D4 CI:** fast checks locally and in GitHub Actions on the fork on each push; long runs (N+1 bounds, Composition simulation) nightly on Tyler's Mac. No paid runner.
- **D5 Criteria wording:** the reworded criteria and the two Δ rules get new IDs (S5a, S6a, S7a, S10a, S12a, R9 sweep-on-every-run-end, R10 fan-out counts unsettled tasks). No renumbering.

## Requirement ID registry (avoid clashes between model agents)

- R9 sweep results on every run end (D5). R10 fan-out counts unsettled tasks (D5).
- From ContextTransfer/NativeSubagents (`../tla/v2/README-ct.md`, which numbered them R9-R14 locally):
  - R11 (ct R9) settle an uncertain delivery in recovery, not only at the next turn start.
  - R12 (ct R10) give a background native result that arrives after the root turn a destination.
  - R13 (ct R11) do not close still-running background subagents at root-turn end (corrects DESIGN §6 N3 and S13).
  - R14 (ct R12) persist per-native-thread delivery state; replace the thread on uncertain delivery.
  - R15 (ct R13) remove already-delivered message ids from every packet.
  - R16 (ct R14) a switch packet ends before the run's own input.
- Criteria rewordings from ContextTransfer: L5a (conditional on target run start), S13a (background subagents outlive the root turn), S8a (anchors and per-packet vs cross-packet duplicates stated), F10a (task row matches subagent record).
- Tooling: tla-ts 0.0.8 has no simulation mode; use TLC `-simulate` (jar at `~/enzo-work/wellsky-tla/tools/tla2tools.jar`) for Composition and N+1 fallbacks.
- From Placement (`../tla/v2/README-pl.md` PL-1…PL-4):
  - R17 (PL-1) dispatch checks message id: a retry of a host-claimed attempt must not run the provider twice (`dispatch_inner` sessions.rs:836 has no id check). Suspected live Zeron bug; not reproduced.
  - R18 (PL-2) before dispatch, check the synced doc, not only SQLite (update rollback).
  - R19 (PL-3) `quiesce_for_sleep` must not sleep a Cloud machine with unacknowledged chat2 rows. Suspected live Zeron bug; not reproduced.
  - R20 (PL-4) host handover needs the four conditions in README-pl.md (Zeron has no handover today).
- Proofs: tlapm 1.6.0-pre in `~/.local/tlapm` (PATH `$HOME/.local/tlapm/bin`). S9: 2,530/2,559 obligations, 23/33 action cases; IndInv model-checked on 159,822 states. CI needs newer tla-ts for `prove`.
- From Delegation/RunLifecycle (`../tla/v2/README.md`, local numbers in parentheses):
  - R9 confirmed (sweep on every run end; a plain interrupt strands results without it).
  - R21 (del R12) a provider-failure hold must not strand result wakes (timing-dependent T3 bug).
  - R22 (del R15) a plain interrupt must not strand a child's question (catch-up on every run end).
  - R23 (del R13, candidate) Delegate vs a pending ancestor Stop: conflicts with host-local reads. Open decision D7.
  - R24 (del R14) Stop disarms usage-limit resume (T3 `holdStoppedThread`); unchecked race: Stop reaches a live run that then ends on a usage limit.
  - D6 open: is a person's message held by a usage limit released when a later run completes (T3 releases only unattended server messages, Orchestrator.ts:2527)?
  - Fix: RL11 guard "not held" makes F9 unreachable (T3 holds the queue on a usage limit); resume must be allowed from a usage-limit hold.
  - Criteria: S11a needed (async stop cascade); S12a checked; S5a replaces S5; S10 checked via ghost variables.
- **D6 (Tyler):** a person's message held by a usage limit sends automatically once the limit resets and a run can start. (Differs from T3, which releases only unattended server messages.)
- **D7 (Tyler):** Delegate checks only local state. A grandchild started while an ancestor's Stop is in flight is stopped by the arriving cascade. R23 is dropped; S11a states the eventual form: after a Stop is applied on a host, that host starts no new delegation, and every descendant is eventually stopped or terminal.
- **AC-P3 approved (Tyler, 2026-11-08):** DESIGN.md approved; build starts with crates/orch step() core while refinement gaps and S9 proof close in parallel.
- **Verification round 2 (2026-11-09; `../tla/v2`).** Spec wins over the tables; table rows changed to match it: RL1 (MaxQueue bounds person and server sends only), RL9 (arm the resume per run; stop hold kept, nothing armed), RL11 (guard `held ∈ {none, limit}`, start the resume run at once, D6 release), §2 Task row (one open task per child; follow-up = new task (c, r)), Q8 (catch-up on every run end).
  - D6 applied: `UsageLimitRecovery` releases a `limit` hold (never `stop`/`fail`); `D6_ReleaseAtReset` + mutant `D6_releaseOnReset` (also fails `R11_LimitHoldReleased`). RL11 fix checked (`W_RL_F9`). R24 checked: `W_RL_R24` reaches "Stop, then usage-limit end" and `R14_StopDisarmsResume` holds there.
  - D7 applied: `StopCheckAnc` fixed FALSE; `S11a_StopReachesDescendants` passes at depth 2 (`MC_Del_Live2`, 422,230 states); mutant `M_D_S11a_cascade` caught.
  - D2 applied: `DesignMaxDepth == 2`, `DesignMaxFanout == 8`, invariant `D2_DesignCaps` in every Delegation safety config except the deliberate 4-deep N+1 sim. Depth counts from root 0.
  - S9 proved: 3,280/3,280 obligations, 33/33 cases (`OutWithCmd` added to IndInv). S10 step form `ReplayStepIsStutter` proved: 803/803 (`PlacementS10Proof.tla`).
- R25 follow-up tasks need settle receipts keyed on (child, child run): keyed on the child only, a late duplicate of the previous task's notice settles the follow-up while its run is live (`M_D_FU_runKey`, `FU_SettledAfterTaskRun`). A follow-up is accepted only after the child's previous task is handled.
- R26 QSplit question effects (QuestionUp, AnswerDown, release, QuestionClosed) are durable outbox effects with the same retry as notices; without delivery fairness a question stalls in flight (`MC_Del_QSLive` failed L4 before the fairness fix).
- R27 Q9 at the root: a child question the root's run saw and left unanswered is released past the root, so it waits for the person (desktop notification) and no agent is woken again. L4 is restated as L4a: pending ⇒ eventually answered, visible to an agent, or surfaced to the person (`AtPerson`).
- R28 S6 needs clamps tracked in flight: DESIGN §9.1's mutant "Clamp never sent" passes `EnforcedAccessOK` and the refinement (51,952 states); `ZI_ClampInFlight` catches it. A received Clamp applies to the live run (D3).
- R29 a read ack counts only at the observed step (ReadResult split into reply and observed, like WaitReply/WaitObserved); counting at reply time loses a counted read on a crash (`ZI_Read_M_AckAtReply`).
- R30 a device crash clears the volatile wait-reply flag (`CrashKeepsReply`, BFS depth 17).
- R31 the receipt check runs on every ingest path, including the second copy of a duplicated row (`DupNoReceipt`, depth 10).
- R32 host(q) owns the question epoch: holders carry it; host(q) drops answers with an old epoch.
- R33 a drain checks the replica (doc) state before it applies a row (`ZI_PL_M_NoReplicaGate`, depth 8; with R18).
- R34 a native fork (X2) needs the source and target on the same host; ContextTransfer has no host check (`ZI_CT_Gap_NativeForkCrossHost`).
- R35 RootComplete's guard (no open wait, not blocked) must hold when the provider emits `completed`; a completed event that the reducer rejects stays in the inbox forever (Composition).
- R36 agent tool steps (wait reply, read ack) need the current attempt still running; otherwise an ack lands after the run's terminal event.
- R37 a new attempt (retry, provider recovery, steering restart) drops the old attempt's open wait.
- R38 each stop-cascade hop disarms the child's usage-limit resume (R14 per hop).
- R39 a D3 clamp restart counts against the attempt/retry budget.
- R40 a person send needs an existing chat (in the model a never-created child ran and delegated to itself).
- R41 open decision: on a provider crash, NativeSubagents ends the run but RunLifecycle starts a new attempt of the same run; also define what a retry or restart does to running natives.
- R42 X5/CT: define the transfer and turn state on a failed, interrupted or usage-limit run end (CT has a turn end only on Complete).
- Spec digests (sha256, first 8 hex) for the Rust MBT fixtures are in the round-2 report.
