# Zeron Orchestrator v2: TLA+ modules `RunLifecycle` and `Delegation`

Checker: **tla-ts 0.0.8** (`../node_modules`). N+1 random simulation uses **TLC 2026.11.03** (`tla2tools.jar`), because tla-ts 0.0.8 has no simulation mode.
Model 1 (`../Orchestration.tla`) is the starting point. Every model-1 fix (R1-R8) is built in, and each fix has a mutant switch.

## How to run

```sh
cd tla/v2
npm run check          # main configs (exhaustive), about 31 min at -workers 4 on a shared Mac
npm run check:all      # main + findings + witnesses + mutants + coverage + N+1 (about 2.5 h; N+1 exhaustive attempts may TIMEOUT, which is accepted)
npm run check:mutants  # or :witnesses, :coverage
node run.mjs MC_Del    # one config by name
node report.mjs        # markdown tables from out/results.json
```

- **Generated configs.** `gen.mjs` holds one table and writes every `.cfg` plus `configs.json`. Edit the table, never a `.cfg`.
- **Runner.** `run.mjs` runs each config, writes `out/<name>.log` and merges a row into `out/results.json`. It exits 0 only when every config gives its expected result and coverage has no gap.
- **No `-deadlock` flag.** Each module has a `Done` action that stutters only in documented terminal states (`RL_Quiescent`, `D_Quiescent`). Any other state with no enabled action is reported as a deadlock (AC-M7).
- **Coverage (AC-M5).** `COV_*` configs run TLC 2026.11 with `-coverage 1` (exhaustive) and count, per action, the states it generated. The tla-ts dot dump builds one string in memory and fails above about 300k states, so it is not used.
- **Mutants.** A mutant is a config that sets `Mut = "<guard>"`. `M("<guard>")` in the spec turns off exactly that one guard. This keeps every mutant in sync with the spec.

## Files

| File | Content |
|---|---|
| `RunBase.tla` | Shared run core: queue, hold reason, live run set, run ordinal, interrupt request; `S1`; the mutant switch `M`. |
| `RunLifecycle.tla` | Queue, dispatch modes, attempts, provider events, interrupt (R7), usage-limit resume (F9), retries. |
| `Delegation.tla` | Tasks, wake delivery, wait, read/ack, caps, access clamp, stop cascade, outbox, crash/replay, questions. |
| `CompositionStub.tla` | Both modules `INSTANCE`d over one copy of the RunBase variables. Proves they compose; `Composition` will replace its `Next`. |
| `gen.mjs`, `configs.json` | Config table and generated configs (`MC_*`, `COV_*`, `witnesses/W_*`, `mutants/M_*`). |
| `run.mjs`, `report.mjs`, `package.json` | Runner with expected results, report tables, npm scripts. |

## Composition seam

```
                 RunBase (queue, held, liveSet, rord, intReq, Mut)
                 /                                  \
   RunLifecycle EXTENDS                       Delegation EXTENDS
   provider events, attempts,                 tasks, cohorts, outbox,
   usage limit, retries                       questions, stop cascade
                 \                                  /
          CompositionStub: RL == INSTANCE ..., D == INSTANCE ...
          (one copy of the base vars; RL!Stop vs D!Stop never clash)
```

- **Run end.** In `Composition`, `RL!IngestEnd` (a root terminal of the current attempt) must conjoin the Delegation run-end hook: the settle notice, the R9 sweep and the question catch-up of `D!EndRun`.
- **Run start.** `RL!StartFromQueue` on a wake message must conjoin `D!StartFromQueueWake` (accept the delivery).
- **Narrowing (D3).** `D!NarrowAccess` on a live descendant must trigger `RL!RestartActive` when the harness cannot reconfigure in place.

## What is modeled

### RunLifecycle (one thread; T3: Orchestrator.ts:5834-6117, UsageLimitRecoveryWorker.ts)

| Area | Model |
|---|---|
| Queue | FIFO of person and server messages (wake, follow-up). `held` has a reason: `stop`, `fail`, `limit`. A client send releases any hold; a server message never does (:6117). |
| Dispatch | `PersonSend(t, mode)`: no active run (any mode), `queue`, `now` (start_immediately behind an active run queues, :6110), or a steer/restart that is too late or comes after Stop (:5842, :5857). `Steer(t)`: native steer joins the active attempt. `RestartActive(t, mode)`: restart, or steer on a provider without native steering. |
| Attempts | `att`, `why ∈ {initial, steering_restart, retry, provider_recovery}`. A restart supersedes the attempt; the old turn later reports `interrupted` as a stale event (:5449-5479). |
| Provider | `ProviderTerminal(t, o)` puts an event `(run, attempt, outcome)` in `pev`. Outcomes: completed, failed, interrupted (only after a request), usage_limit, transient, crash. `ChildStart`/`ChildEnd`: a non-root node (tool call, native subagent). |
| Engine | `IngestEnd` (root terminal of the current attempt ends the run), `RetryAttempt` (transient or crash with budget and no Stop), `IngestChild` (never ends the run), `IngestStale` (event of a superseded attempt or ended run: dropped). |
| Interrupt (R7) | `RequestInterrupt(t)` marks the request and holds the queue; only a provider `interrupted` event ends the run. |
| Usage limit (F9) | A usage-limit end holds the queue (`limit`) and arms the resume for that run. `ResetPasses`, then `UsageLimitRecovery` starts a resume run at once (start_immediately, created by server). A newer run disarms it. |

### Delegation (T3: Orchestrator.ts:7955-14255, OrchestratorMcpService.ts)

Model 1, plus:

| Change | Why |
|---|---|
| Held reason; `RootFail` holds the queue (`fail`) | T3 holds all queued runs on a provider failure (Orchestrator.ts:1780). This found R12. |
| `InterruptChat(t)` | DESIGN RL6: a plain interrupt with no hold and no disposal. Needed to test R9. |
| R9 sweep on every run end | DESIGN RL10 Δ. Mutant `R9_sweepInterrupted` restores model 1's rule. |
| R10 fan-out counts unsettled tasks | DESIGN T1 Δ / D2. `S5` checks unsettled tasks. |
| Questions rebuilt for S12a | `qAt` = the one agent target; `qVis` = agents that have it in context. Raise routes to the nearest unblocked ancestor (R8). When a thread blocks, questions aimed at it move up and their old visibility is cleared. The person may always answer; an answer applies only to a pending question. |
| Stop effects carry a stop id and leave a receipt `(child, stop id)` | DESIGN S-3. Mutant `S10_stopReceipt` shows the receipt is needed. |
| `S10` and `S10a` are checked | `replayBad`: a replay of a kept effect changed the domain state. `receiptBad`: applying an effect whose receipt exists changed it. |
| Flat `Next`, constant quantifier bounds | So that TLC splits and names each action (coverage). |

### Abstracted (deliberate)

- **RunLifecycle.** One thread: threads share no state in this module. Steered messages are part of the run context, so a retry attempt carries them. Provider steering support is chosen per send, which over-approximates a provider switch. The queue order when a held queue is released and a new message starts at once is not modeled; the new message joins the back.
- **Delegation.** Runs collapse to live and ended (attempts are RunLifecycle's job). One task per child; follow-up task runs on a child (T3 `recordSubagentFollowup`, Orchestrator.ts:8202) are not modeled, so F1 is reached through wait or read inside the child's task run. T3's wake that starts at once when no run is active (and so skips the hold) is not modeled. Questions decide in one step; DESIGN Q2 splits publish and decide, which is Placement's concern. DESIGN Q9 also re-routes a question whose question run ended unanswered; here it stays visible to that thread.
- **Both.** No remote children, approvals, archive or delete. Access has two levels.

## Properties

| ID | Module | Formula | Kind |
|---|---|---|---|
| S1 | both | `S1_AtMostOneActiveRun`: at most one live run ordinal per thread | invariant |
| S2 | RL | `S2_OnlyRootEndsRun`: a run leaves live only in a step that consumes the root terminal of its current attempt | action property |
| S2 | Del | `D_S2_OnlyRootEndsRun`: only RootComplete, RootFail or ConfirmInterrupt end a run | action property |
| R7 | RL | `R7_NoNewAttemptAfterStop`; `RL_StopQuiesces` | action property; liveness |
| S3 | Del | `S3_ExactlyOnce`: pushes ≤ 1, and pushes = 1 ⇒ delivered or acknowledged | invariant |
| S4 | Del | `S4_NoWakeIntoLiveRun`: a settled_only result is never claimed while its spawning run is live; wakes are queue_after_active only | invariant |
| S5 / S5a | Del | `S5_DepthFanout`: depth ≤ MaxDepth, unsettled tasks ≤ MaxFanout (all tasks here are Zeron-created, so S5 = S5a) | invariant |
| S5 original | Del | `S5_OriginalTotalFanout`: every child ever created ≤ MaxFanout. Fails by design under R10 (`MC_Del_S5Original`); S5a replaces it | invariant (finding) |
| S6 / S6a | Del | `S6_AccessNarrow` (abstract, clamp at once) | invariant |
| S7 / S7a | Del | `S7_AckFromLiveRun`: delivered or acknowledged ⇒ an agent run got it | invariant |
| S10 | Del | `S10_ReplayIdempotent` | invariant over a ghost |
| S10a | Del | `S10a_ReceiptNoop` | invariant over a ghost |
| S11 | Del | `S11_NoDelegateAfterStop` (no delegation once the stop reached the thread), `S11_NoOrphanRunning` | action property; invariant |
| S11 literal | Del | `S11_NoDelegateInStoppedTree` (no thread in a stopped subtree delegates) | action property (finding) |
| S12a | Del | `S12a_OneAgentAnswerer`, `S12a_FirstAnswerWins` | invariants |
| L1 | Del | `L1_ResultHandled` | liveness |
| L2 | Del | `L2_StopQuiesces`: a doomed thread eventually is not live and is held | liveness |
| L3 | RL | `L3_QueueDrains` | liveness |
| L4 | Del | `L4_QuestionReachesAnswerer` | liveness |
| L6 | Del | `L6_ParentWoken` | liveness |
| R11 | RL | `R11_LimitHoldReleased` | liveness |
| R14 | RL | `R14_StopDisarmsResume`: a thread held by Stop has no armed usage-limit resume | invariant |
| R15 | Del | via `L4_QuestionReachesAnswerer` with `InterruptChat` (catch-up on every end) | liveness |
| AC-M5 | Del | `D_NoStaleWakeQueued`, `D_NoStaleQuestionWake` (former dead actions) | invariants |
| extra | RL / Del | `RL_NoLostSteer`, `RL_RetryBudget`, `RL_BoundsNotHit`; `D_NoStrandedResult`, `D_BoundsNotHit` | |

Fairness: weak fairness on every engine action and on the provider emitting some root terminal. None on person or agent actions. Liveness configs have no SYMMETRY (unsound with liveness) and run on one worker.

## Bounds (AC-M3)

All runs: `-workers 4` on a 14-core Mac shared with other agents' checkers, so times are high and vary by about 2x. Runner time limit 20 min per config.

| Module | Key dimension | N (exhaustive) | N+1 |
|---|---|---|---|
| RunLifecycle | person messages `MaxPerson` (threads share no state here, so thread count is not a useful dimension) | P=2 safety (`MC_RL`); P=1 liveness (`MC_RL_Live`). Other budgets: queue 2, server 1, retry 1, stop 1, usage limit 1, crash 1 | P=3 exhaustive attempt (`MC_RL_N1`) and TLC random simulation (`MC_RL_N1_sim`, 3000 traces, depth 100); P=2 liveness attempt (`MC_RL_Live_N1`) |
| Delegation | threads | 3 threads `{r, a, b}`: depth 1 fan-out 2 (`MC_Del`), depth 2 chain (`MC_Del_Deep`), questions depth 2 (`MC_Del_Q`); liveness `MC_Del_Live`, `MC_Del_QLive` | 4 threads: exhaustive attempt with smaller budgets (`MC_Del_N1x`); TLC random simulation with full budgets (`MC_Del_N1`) and a 4-deep question chain (`MC_Del_Q_N1`) |

- **Simulation is safety only.** The `tlc-sim` rows check invariants on random traces. They give no proof, and liveness and coverage at N+1 are not checked.
- **SYMMETRY.** Only on the Delegation safety configs, over the non-root threads. Liveness configs have none.
- **Coverage at N+1.** Coverage at N is already complete (every action of both modules fires), so N+1 cannot add action coverage. N+1 only adds depth and interleavings.

### Mutant gaps (AC-M6)

Every property has at least one mutant that the checker catches, except these:

| Property | Why no single-guard mutant fails it |
|---|---|
| `D_S2_OnlyRootEndsRun` | Holds by construction: Delegation collapses runs to live and ended, and only `RootComplete`, `RootFail` and `ConfirmInterrupt` write ended. RunLifecycle's `S2_OnlyRootEndsRun` is the real S2 check (2 mutants). |
| `RL_StopQuiesces` | Every loop that a removed guard opens (retry after Stop, restart, resume) is cut by a saturating budget, so a stopped thread still goes idle. The safety form `R7_NoNewAttemptAfterStop` catches the same guard (`M_RL_R7_retryAfterStop`). |

`TypeOK` and `BoundsNotHit` are sanity checks and have no mutant.

**Deadlock mutants (AC-M7).** Removing a guard rarely creates a deadlock by itself, so each deadlock mutant strands work that `Quiescent` says is owed. `M_RL_Deadlock_stale`: a stale provider event is never discarded. `M_D_Deadlock_genGate`: without the generation gate, a replayed offer queues a stale wake; it sits at the queue head, `StartFromQueueWake` refuses it, and `Done` refuses because the queue is not idle (`QueueIdle`). Both report a deadlock without `-deadlock`.

Designed survivors (kept as evidence, expected `PASS`): `M_D_S3_settleDedupe` and `M_D_S3_planSkip` (each guard fully covers the other) and `M_RL_L3_heldNeverCleared` (L3 says nothing about held queues). See Differences 3 below.

### Results

| Config | Module | Kind | Distinct states | Depth | Time | Expected | Got | Match |
|---|---|---|---|---|---|---|---|---|
| MC_RL | RunLifecycle | main | 1,552,774 | 42 | 224s | PASS | PASS | yes |
| MC_RL_N1 | RunLifecycle | n1 | - | - | 1500s | PASS or TIMEOUT | TIMEOUT | yes |
| MC_RL_Live | RunLifecycle | main | 82,539 | 32 | 35s | PASS | PASS | yes |
| MC_RL_N1_sim | RunLifecycle | n1 | 12000 traces | - | 37s | PASS | PASS | yes |
| MC_RL_Live_N1 | RunLifecycle | n1 | 1,552,774 | 42 | 996s | PASS or TIMEOUT | PASS | yes |
| MC_RL_T3Limit | RunLifecycle | finding | 11,054 | 14 | 3s | Temporal R11_LimitHoldReleased | Temporal R11_LimitHoldReleased | yes |
| COV_RL | RunLifecycle | coverage | 82,539 | 32 | 7s | PASS | PASS | yes |
| MC_Del | Delegation | main | 2,854,988 | 25 | 209s | PASS | PASS | yes |
| MC_Del_Deep | Delegation | main | 2,586,973 | 24 | 188s | PASS | PASS | yes |
| MC_Del_Q | Delegation | main | 4,511,153 | 30 | 305s | PASS | PASS | yes |
| MC_Del_Live | Delegation | main | 1,000,458 | 24 | 803s | PASS | PASS | yes |
| MC_Del_QLive | Delegation | main | 354,256 | 28 | 113s | PASS | PASS | yes |
| MC_Del_T3FailHold | Delegation | finding | 19,310 | 10 | 3s | Temporal L1_ResultHandled | Temporal L1_ResultHandled | yes |
| MC_Del_S5Original | Delegation | finding | 1,268 | 8 | 0s | Invariant S5_OriginalTotalFanout | Invariant S5_OriginalTotalFanout | yes |
| MC_Del_StopTree | Delegation | finding | 241 | 7 | 0s | Action property S11_NoDelegateInStoppedTree | Action property S11_NoDelegateInStoppedTree | yes |
| MC_Del_StopTreeFixed | Delegation | finding | 731,131 | 23 | 54s | PASS | PASS | yes |
| MC_Del_N1x | Delegation | n1 | 3,563,228 | 30 | 418s | PASS or TIMEOUT | PASS | yes |
| MC_Del_N1 | Delegation | n1 | 12000 traces | - | 23s | PASS | PASS | yes |
| MC_Del_Q_N1 | Delegation | n1 | 12000 traces | - | 24s | PASS | PASS | yes |
| COV_Del_Q | Delegation | coverage | 354,256 | 28 | 20s | PASS | PASS | yes |
| COV_Del_Main | Delegation | coverage | 5,697,420 | 25 | 432s | PASS | PASS | yes |
| COV_Del_Stop | Delegation | coverage | 453,672 | 24 | 30s | PASS | PASS | yes |
| CompositionStub | CompositionStub | main | 1 | 1 | 0s | PASS | PASS | yes |

### Witnesses

| Config | Witness invariant | States to violation | Expected | Got | Match |
|---|---|---|---|---|---|
| W_RL_F9 | W_F9_UsageLimitResumed | 1,265 | Invariant W_F9_UsageLimitResumed | Invariant W_F9_UsageLimitResumed | yes |
| W_RL_Steer | W_SteerApplied | 84 | Invariant W_SteerApplied | Invariant W_SteerApplied | yes |
| W_RL_Restart | W_RestartCompleted | 76 | Invariant W_RestartCompleted | Invariant W_RestartCompleted | yes |
| W_RL_Retry | W_RetryCompleted | 141 | Invariant W_RetryCompleted | Invariant W_RetryCompleted | yes |
| W_RL_Recovery | W_RecoveryCompleted | 148 | Invariant W_RecoveryCompleted | Invariant W_RecoveryCompleted | yes |
| W_RL_Interrupt | W_InterruptConfirmed | 129 | Invariant W_InterruptConfirmed | Invariant W_InterruptConfirmed | yes |
| W_RL_R11Held | W_R11_HeldAfterResume | 2,072 | Invariant W_R11_HeldAfterResume | Invariant W_R11_HeldAfterResume | yes |
| W_RL_R11Fixed | W_R11_HeldAfterResume | 82,539 | PASS | PASS | yes |
| W_D_F1 | W_F1_GrandchildToRoot | 55,138 | Invariant W_F1_GrandchildToRoot | Invariant W_F1_GrandchildToRoot | yes |
| W_D_F2 | W_F2_PushedNoPoll | 17,601 | Invariant W_F2_PushedNoPoll | Invariant W_F2_PushedNoPoll | yes |
| W_D_F3 | W_F3_Coalesced | 215,361 | Invariant W_F3_Coalesced | Invariant W_F3_Coalesced | yes |
| W_D_F4 | W_F4_WaitInRun | 586 | Invariant W_F4_WaitInRun | Invariant W_F4_WaitInRun | yes |
| W_D_F7 | W_F7_QuestionAnswerable | 210 | Invariant W_F7_QuestionAnswerable | Invariant W_F7_QuestionAnswerable | yes |
| W_D_R8 | W_R8_NestedPastBlocked | 15,290 | Invariant W_R8_NestedPastBlocked | Invariant W_R8_NestedPastBlocked | yes |
| W_D_F8 | W_F8_StopCascades | 2,727 | Invariant W_F8_StopCascades | Invariant W_F8_StopCascades | yes |

### Mutants

| Config | Guard removed (Mut) | Target property | States | Expected | Got | Match |
|---|---|---|---|---|---|---|
| M_RL_S1_start | `S1_start` | S1_AtMostOneActiveRun | 42 | Invariant S1_AtMostOneActiveRun | Invariant S1_AtMostOneActiveRun | yes |
| M_RL_S2_child | `S2_child` | S2_OnlyRootEndsRun | 159 | Action property S2_OnlyRootEndsRun | Action property S2_OnlyRootEndsRun | yes |
| M_RL_S2_stale | `S2_stale` | S2_OnlyRootEndsRun | 82 | Action property S2_OnlyRootEndsRun | Action property S2_OnlyRootEndsRun | yes |
| M_RL_R7_retryAfterStop | `R7_retryAfterStop` | R7_NoNewAttemptAfterStop | 131 | Action property R7_NoNewAttemptAfterStop | Action property R7_NoNewAttemptAfterStop | yes |
| M_RL_Steer_tooLate | `Steer_tooLate` | RL_NoLostSteer | 98 | Action property RL_NoLostSteer | Action property RL_NoLostSteer | yes |
| M_RL_RetryBudget | `L3_retryBudget` | RL_RetryBudget | 925 | Invariant RL_RetryBudget | Invariant RL_RetryBudget | yes |
| M_RL_L3_retryBudget | `L3_retryBudget` | L3_QueueDrains | 209,463 | Temporal L3_QueueDrains | Temporal L3_QueueDrains | yes |
| M_RL_L3_heldNeverCleared | `L3_heldNeverCleared` | L3_QueueDrains | 53,608 | PASS | PASS | yes |
| M_RL_R14_stopDisarm | `R14_stopDisarm` | R14_StopDisarmsResume | 139 | Invariant R14_StopDisarmsResume | Invariant R14_StopDisarmsResume | yes |
| M_RL_Deadlock_stale | `D_staleDiscard` | deadlock | 828 | Deadlock | Deadlock | yes |
| M_D_S1_start | `S1_start` | S1_AtMostOneActiveRun | 7,712 | Invariant S1_AtMostOneActiveRun | Invariant S1_AtMostOneActiveRun | yes |
| M_D_S3_readHandled | `S3_readHandled` | S3_ExactlyOnce | 6,462 | Invariant S3_ExactlyOnce | Invariant S3_ExactlyOnce | yes |
| M_D_S3_settleDedupe | `S3_settleDedupe` | S3_ExactlyOnce, S10_ReplayIdempotent | 2,854,988 | PASS | PASS | yes |
| M_D_S3_planSkip | `S3_planSkip` | S3_ExactlyOnce, S10_ReplayIdempotent | 2,854,988 | PASS | PASS | yes |
| M_D_S4_genGate | `S4_genGate` | S10_ReplayIdempotent | 384,040 | Invariant S10_ReplayIdempotent | Invariant S10_ReplayIdempotent | yes |
| M_D_Stale_genGate | `S4_genGate` | D_NoStaleWakeQueued | 216,773 | Invariant D_NoStaleWakeQueued | Invariant D_NoStaleWakeQueued | yes |
| M_D_Deadlock_genGate | `S4_genGate` | deadlock | 2,048,077 | Deadlock | Deadlock | yes |
| M_D_R9_sweepInterrupted | `R9_sweepInterrupted` | D_NoStrandedResult | 5,534 | Invariant D_NoStrandedResult | Invariant D_NoStrandedResult | yes |
| M_D_S4_liveSkip | `S4_liveSkip` | S4_NoWakeIntoLiveRun | 586 | Invariant S4_NoWakeIntoLiveRun | Invariant S4_NoWakeIntoLiveRun | yes |
| M_D_S5_depth | `S5_depth` | S5_DepthFanout | 162 | Invariant S5_DepthFanout | Invariant S5_DepthFanout | yes |
| M_D_S5_fanout | `S5_fanout` | S5_DepthFanout | 61 | Invariant S5_DepthFanout | Invariant S5_DepthFanout | yes |
| M_D_S6_clamp | `S6_clamp` | S6_AccessNarrow | 67 | Invariant S6_AccessNarrow | Invariant S6_AccessNarrow | yes |
| M_D_S7_ackLive | `S7_ackLive` | S7_AckFromLiveRun | 6,299 | Invariant S7_AckFromLiveRun | Invariant S7_AckFromLiveRun | yes |
| M_D_S10_stopReceipt | `S10_stopReceipt` | S10_ReplayIdempotent | 4,179 | Invariant S10_ReplayIdempotent | Invariant S10_ReplayIdempotent | yes |
| M_D_S10a_stopReceipt | `S10_stopReceipt` | S10a_ReceiptNoop | 4,179 | Invariant S10a_ReceiptNoop | Invariant S10a_ReceiptNoop | yes |
| M_D_S11_stopGuard | `S11_stopGuard` | S11_NoDelegateAfterStop | 41 | Action property S11_NoDelegateAfterStop | Action property S11_NoDelegateAfterStop | yes |
| M_D_S11_outbox | `S11_outbox` | S11_NoOrphanRunning | 495 | Invariant S11_NoOrphanRunning | Invariant S11_NoOrphanRunning | yes |
| M_D_R12_stopPrecedence | `R12_stopPrecedence` | S11_NoOrphanRunning | 1,709 | Invariant S11_NoOrphanRunning | Invariant S11_NoOrphanRunning | yes |
| M_D_S12_reroute | `S12_reroute` | S12a_OneAgentAnswerer | 38,261 | Invariant S12a_OneAgentAnswerer | Invariant S12a_OneAgentAnswerer | yes |
| M_D_QW_answerCancel | `QW_answerCancel` | D_NoStaleQuestionWake | 274 | Invariant D_NoStaleQuestionWake | Invariant D_NoStaleQuestionWake | yes |
| M_D_S12_answerOnce | `S12_answerOnce` | S12a_FirstAnswerWins | 645 | Invariant S12a_FirstAnswerWins | Invariant S12a_FirstAnswerWins | yes |
| M_D_L1_waitSweep | `L1_waitSweep` | L1_ResultHandled | 20,137 | Temporal L1_ResultHandled | Temporal L1_ResultHandled | yes |
| M_D_L2_cascade | `L2_cascade` | L2_StopQuiesces | 278,234 | Temporal L2_StopQuiesces | Temporal L2_StopQuiesces | yes |
| M_D_L4_escalate | `L4_escalate` | L4_QuestionReachesAnswerer | 17,798 | Temporal L4_QuestionReachesAnswerer | Temporal L4_QuestionReachesAnswerer | yes |
| M_D_R15_catchUpInterrupted | `R15_catchUpInterrupted` | L4_QuestionReachesAnswerer | 17,430 | Temporal L4_QuestionReachesAnswerer | Temporal L4_QuestionReachesAnswerer | yes |
| M_D_L6_failHold | `FixWakeSkipsFailHold=FALSE` | L6_ParentWoken | 174,980 | Temporal L6_ParentWoken | Temporal L6_ParentWoken | yes |

### Coverage

- RunLifecycle: 16/16 actions taken; missing: none
- Delegation: 25/25 actions taken; missing: none

- `MC_RL_Live_N1` has the same constants as `MC_RL` (P=2), so it explores the same graph; it adds the liveness check at P=2.
- `MC_RL_N1` (P=3 exhaustive) hit the 25-min limit; tla-ts 0.0.8 prints no progress, so the state count at the cut is unknown. `MC_RL_N1_sim` covers P=3 by simulation.
- `MC_Del_N1x` (4 threads, budgets cut to person 1, stop 1, crash 1, duplicate delivery) finished exhaustively.
- The `QW_answerCancel` mutant switch was added after the main runs; with `Mut = "none"` it changes nothing.

## Findings

New requirements continue the numbering after DESIGN's R9 (sweep on every end) and R10 (fan-out counts unsettled tasks).

### R9 confirmed: a plain interrupt strands results without the sweep

Mutant `R9_sweepInterrupted` (model 1's rule: sweep only on completed and failed) fails `D_NoStrandedResult` in 9 steps:

1. Person sends to r; r's run starts. r delegates a with `wait`.
2. `InterruptChat(r)`: an interrupt request with no hold and no disposal.
3. a runs and completes; its settle sees r's spawning run live and `settled_only`, so the task stays **pending**.
4. The provider confirms the interrupt on r. No sweep runs, the cohort stays open, and no delivery is in flight. The result is stuck.

With R9 (sweep on every end) the model passes.

### R11 (new, a Zeron policy choice; T3 is partly safe): a usage-limit hold outlives the auto-resume

Trace `W_RL_R11Held` (`FixLimitRelease = FALSE`, the hold is released only by a person send):

1. Person sends (queued), a server wake is queued, the first run starts.
2. The provider reports a usage limit. The run ends failed; the queue is held with reason `limit` (T3 re-holds, Orchestrator.ts:1780) and the resume is armed.
3. The reset passes; `UsageLimitRecovery` starts a resume run (start_immediately, server-created) and it completes.
4. The queue is still held. The wake and the person's message never run until the person sends again (`MC_RL_T3Limit` fails `R11_LimitHoldReleased`).

**Checked against T3.** T3 is safe for the wake: `releaseUnattendedHolds` (Orchestrator.ts:2527, called on every completed run at :14122) releases held runs whose message came from the server, an agent or a scheduled task, when the newest executed run completes and was not stopped. T3 keeps the person's message held on purpose ("a hold waits for a person to review what they queued"). The model's hold is per thread, not per run, so it cannot express that split. `FixLimitRelease = FALSE` is therefore stricter than T3 for server messages.

**Requirement R11.** When a later run (for example the auto-resume) completes and was not stopped, release held server messages (T3 `releaseUnattendedHolds`). DESIGN must decide if a person message held by a usage limit is also released (the model's fix does this) or waits for review (T3). With the fix, `W_RL_R11Fixed` (the stuck state) is unreachable and `MC_RL_Live` passes R11.

### R14 (kept, T3 already does it): Stop disarms a usage-limit resume

Mutant `R14_stopDisarm` fails `R14_StopDisarmsResume` in 6 steps: the run ends on a usage limit, the person presses Stop on the idle thread, and the resume is still armed, so the thread restarts by itself after the reset.

**Checked against T3.** T3 does disarm it: `holdStoppedThread` (Orchestrator.ts:11099-11150) sets `limitRecovery.autoResume = false` on Stop, and `limitRecoveryCommand` then returns no resume. So R14 is not a T3 bug; it is a T3 rule that DESIGN RL11 must keep. Not checked in T3: the race where Stop reaches a live run and the run then ends on a usage limit; the worker may arm a new recovery for that run.

**Requirement R14.** Stop disarms a pending usage-limit resume, and a usage-limit end of a run whose interrupt was requested keeps the Stop hold and arms nothing.

### R12 (new): a provider-failure hold strands result wakes

Config `MC_Del_T3FailHold` (`FixWakeSkipsFailHold = FALSE`) fails `L1_ResultHandled`:

1. r delegates a with `wait`; a starts.
2. r's run fails, so r's queue is held (`fail`). T3 holds every queued run of the thread, delegated-result wakes included (Orchestrator.ts:1780). A client send releases it (:6117), and so does a later completed run for server messages (`releaseUnattendedHolds`, :2527), but no later run comes here.
3. a completes. The R1/R9 sweep claimed nothing (a was still running), so the settle claims a's result and the offer queues a wake on r.
4. Nothing ever starts the wake. The result waits for the person forever (also `L6_ParentWoken`, mutant `M_D_L6_failHold`).

T3 differs in this exact order: a wake dispatched while no run is active starts at once and skips the hold (:6110). T3 strands the other order, where the wake was queued behind r's live run and then r failed. So in T3 the same result is delivered or stranded depending on timing. DESIGN RL2's `¬held` guard strands both orders.

**Requirement R12.** A provider-failure hold does not hold result or question wakes. A Stop hold always wins: a failure never downgrades a `stop` hold to `fail`.

The second sentence came from the checker. A first version of the fix let wakes pass any `fail` hold, and `MC_Del_Q` failed `S11_NoOrphanRunning` in 9 steps: Stop(r), then r's run fails (the hold became `fail`), a raises a question, and the question wake started a new run on the stopped r. Mutant `R12_stopPrecedence` keeps that trace.

### R15 (new): a plain interrupt strands a child question

`MC_Del_QLive` failed `L4_QuestionReachesAnswerer` before this fix (10 steps):

1. r delegates a with `wait`; a starts.
2. `InterruptChat(r)` (no cascade, so a keeps running).
3. a asks a question. The wake is skipped because r's wait will show it.
4. The wait is abandoned, then the interrupt is confirmed. The question catch-up runs only on completed and failed ends (T3 DelegatedTaskQuestionReactor.ts:48), so nobody ever sees a's question, and a stays blocked.

**Requirement R15.** Run the question catch-up (DESIGN Q8) on every run end, like the R9 sweep. Mutant `R15_catchUpInterrupted` keeps the trace.

### S11 as worded is not met by an asynchronous cascade (R13, and a conflict with DESIGN's locality rule)

`MC_Del_StopTree` fails `S11_NoDelegateInStoppedTree` in 7 steps: r delegates a; Stop(r) queues the durable stop effect for a; a starts and delegates b before the effect reaches it. The cascade then stops b too (`S11_NoOrphanRunning`, `L2_StopQuiesces` pass), but a delegation did start after Stop.

**R13 (candidate).** Delegate also refuses while a stop effect is pending for the thread or any ancestor, or an ancestor has an interrupt request (`StopCheckAnc = TRUE`, `MC_Del_StopTreeFixed`). This reads another host's outbox, which breaks DESIGN §3's locality rule. With locality, the window between Stop and the effect's arrival cannot close. Proposed: keep R13 as same-host only, and reword S11 as "once the stop reaches a thread, it starts no delegation, and every descendant ends stopped or terminal" (checked: `S11_NoDelegateAfterStop`, `S11_NoOrphanRunning`).

### DoOffer generation gate is needed (DESIGN's S4 mutant), but S10 catches it, not S4

Mutant `S4_genGate` fails `S10_ReplayIdempotent` in 14 steps: a's result is read (the offered wake is cancelled), the offer effect for generation 1 is applied and kept by a crash, b settles and reserves generation 2, then the replayed generation-1 offer enqueues a wake. That wake is stale at start and is dropped, and the real generation-2 offer is now a no-op, so b's claimed result waits on a wake that never comes (an L6 violation).

### Stop effects need a receipt (DESIGN S-3), not only an idempotent body

Mutants `S10_stopReceipt` fail S10 and S10a in 8 steps: r delegates a; Stop(r); the stop effect for a is applied and kept by a crash; the person sends to a, which resumes it; the replayed stop effect stops a again.

## Alignment with DESIGN.md (§3 state tables, Decisions D1-D5)

| DESIGN row | Spec action | Note |
|---|---|---|
| RL1 PersonSend | `PersonSend(t, mode)` (RL), `PersonSend(t)` (Del) | RL adds the dispatch mode. |
| RL2 StartFromQueue | `StartFromQueue` (RL, Del); Del also `StartFromQueueWake` (T6 accept) | The "stale wake: drop" branch is unreachable; it is now the invariant `D_NoStaleWakeQueued` (see Coverage). |
| RL3 Steer / RL4 RestartActive / RL5 RetryAttempt | `Steer`, `RestartActive`, `RetryAttempt` | RetryAttempt also covers provider_recovery. |
| RL6 RequestInterrupt | `RequestInterrupt` (RL, holds the queue); Del `Stop` / `ThreadStop` and `InterruptChat` (no hold, no disposal) | |
| RL7-RL9 ConfirmInterrupt / RootComplete / RootFail | Del: same names. RL: `IngestEnd` by outcome, because RL models the provider event stream | |
| RL10 EndRun | Del `EndRun` | R9 sweep on every end is built in. |
| RL11 UsageLimitRecovery | `UsageLimitRecovery` + `ResetPasses` | See difference 1. |
| T1-T11 | `Delegate`, `DoNotice`, `DoOffer`, `StartFromQueueWake`, `ReadResult`, `WaitReturn` / `WaitReturnQuestion`, `WaitTimeout`, `WaitAbandon`, `LateWaitAck` | |
| Q1, Q3-Q8 | `RaiseQuestion`, `StartQuestion`, `WaitReturnQuestion`, `AgentAnswer` (Q5+Q6), `PersonAnswer`, `EndRun` catch-up | |
| S-1..S-5 | `Stop`, `ThreadStop`, `DoStop` (receipt `(c, stop id)`), Delegate guard, `PersonSend` | |

Differences that need a DESIGN change:

1. **RL11 guard `¬held` never fires after a usage limit.** T3 holds the queue on a usage limit (Orchestrator.ts:1780, :6115). With `¬held`, F9 is dead. The model follows T3: the resume starts at once despite a `limit` hold (start_immediately, server-created). R11 then releases the `limit` hold when the resume completes, and R14 lets a Stop disarm it (as T3 `holdStoppedThread` does).
2. **RL9 `¬ulUsed` (once per thread).** T3 arms once per (run, reset time). The model arms per run; a newer run disarms.
3. **§9.1 mutant choices that the checker does not catch** (AC-M6 would fail with them):
   - "S3: settle receipt": survives. Settle dedupe and "planning skips handled tasks" each fully cover the other. Keep both, but use `S3_readHandled` (read acknowledges only an unhandled result) as the S3 mutant.
   - "L3: held never cleared": survives. L3 says nothing about held queues. Use `L3_retryBudget`.
   - "S4: gen check in DoOffer": see the mutants table (`M_D_S4_genGate`).
4. **Q2 / Q9.** Questions decide in one step here; DESIGN splits publish and decide (a Placement concern). Q9's "question run ended unanswered → re-route up" is not modeled; L4 holds without it.
5. **S12a reworded form is the one checked.** `qVis` can hold one agent; the person may always answer; the first applied answer wins.

## ACCEPTANCE.md wording, as the model sees it

| ID | Status |
|---|---|
| S1, S2, S3, S4, S6, S7 | Hold as worded. |
| S5 | Fails as worded under R10 (`MC_Del_S5Original`: total children > MaxFanout once a child settles and a new one starts). S5a (unsettled tasks) replaces it. |
| S10 | Checked only through ghost variables (`replayBad`, `receiptBad`); "replay changes nothing" is not a state predicate. |
| S11 | Fails as worded with an asynchronous cascade (`MC_Del_StopTree`). Reword as `S11_NoDelegateAfterStop` + `S11_NoOrphanRunning`, or accept R13 with cross-host reads. |
| S12 | Original "exactly one answerer" cannot be tested: the person may always answer. S12a (one agent answerer, first answer wins) is checked. |
| L3 | Holds, but says nothing about held queues (designed survivor `M_RL_L3_heldNeverCleared`). R11 covers the usage-limit hold. |

## Round 2 (2026-11-09): design decisions, flags, CI

| Change | Where | Check |
|---|---|---|
| D6: the usage-limit reset releases a `limit` hold (never `stop` / `fail`) | `RunLifecycle!UsageLimitRecovery` | `D6_ReleaseAtReset` (action property); mutants `M_RL_D6_releaseOnReset` (+ `_Live`, fails `R11_LimitHoldReleased`) |
| RL11 fix: the resume starts from a `limit` hold (`held ∈ {none, limit}`) | same | `W_RL_F9` reachable |
| R24 checked | `W_R24_StopThenLimit` | reachable; `R14_StopDisarmsResume` holds there |
| D7: Delegate reads local state only (`StopCheckAnc` stays FALSE) | Delegation | `S11a_StopReachesDescendants` at depth 2 (`MC_Del_Live2`), mutant `M_D_S11a_cascade` |
| D2 design caps | `DesignMaxDepth`, `DesignMaxFanout`, `D2_DesignCaps` | in every safety config except the 4-deep N+1 sim |
| Follow-up tasks: task id = (child, child run) | `MaxFollowUp` (0 = off), `FollowUp`, `NoticeRcpt` in `rcpt` | `MC_Del_FU`, `W_D_FU`, `M_D_FU_runKey` (fails `FU_SettledAfterTaskRun`), `MC_Del_FU_Live` |
| QSplit liveness (R22 under the split) | `D_Fairness` covers question effects; `L4` adds `AtPerson` | `MC_Del_QSLive`, `M_D_R22_QS_catchUp` |
| Small two-level stop cascade (MBT source) | `MC_Del_Stop2`, `W_D_Stop2` | PASS 12,330 states; witness reached |
| Per-host crash, QSplit (ZI agent) | `PartialCrash`, `QSplit`, `qEp` | small exhaustive rows + `_Big` TLC sims |

- **Runners.** `heavy.sh` is a FIFO ticket queue with 2 slots (`HEAVY_PRIORITY=0` jumps the queue). `run.mjs` takes `BFS_ENGINE=tlc` to run BFS rows on TLC: tla-ts 0.0.8 now needs more than 20 min for `MC_Del` under load, TLC needs about 3 min (2,854,988 states, same count).
- **CI.** `./ci.sh fast` (quick tier, both proofs, ZI tiny, Composition tiny) and `./ci.sh nightly` (every runner, N+1, simulations). Each writes `ci-out/<tier>/summary.md`.
