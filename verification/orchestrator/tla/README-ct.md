# ContextTransfer and NativeSubagents (Zeron Orchestrator v2)

Two TLA+ modules for AC-M1, checked with tla-ts 0.0.8. Deadlock checking is on in every run (the runner does not pass `-deadlock`).

## Files

| File | What |
|---|---|
| `ContextTransfer.tla` | Fork, merge-back, provider switch (catch-up), subagent spawn, subagent result. Lazy resolution, strategies, budget, delivery. |
| `NativeSubagents.tla` | Harness-native subagents in a run: spawn, progress, end, resume, orphan close, background work, crash recovery. |
| `run-ct.mjs` | Writes every config from its job tables, runs them, checks each result against its expected outcome. `node run-ct.mjs [ns|ct|<job prefix>...] [--gen]`. |
| `CT_*.cfg`, `NS_*.cfg` | Generated configs. `_N` and `_N1` bounds, `_Live` liveness, `_Wit_` witnesses, `_Mut_` mutants, `_Cov_` coverage. |
| `out-ct/` | One log per job, `results-<selection>.json`, `run-all.txt`. |

All files use the `CT_` / `NS_` prefix or the `-ct` suffix. Nothing in `mutants/`, `witnesses/`, `out/` or the other modules was touched.

## DESIGN.md mapping

| DESIGN.md | Spec |
|---|---|
| X1 CreateTransfer | `UserSend` (catch-up `provider_switch`), `Fork`, `SpawnChild`, `MergeBack`, `SubagentResult` |
| X2 ResolveNative | `ResolveSwitch` native_resume branch, `ResolveLineage` native_fork branch |
| X3 ResolvePortable | `Resolved` via `HandoffOrFail` (delta, full_summary, portable_replay, merge_delta, result) |
| X4 FailTransfer | `Fail` (reasons `unsupported`, `budget`) |
| X5 ConsumeTransfer | `Settle` from `PersistInjected` / `PersistInline`; native branches consume directly |
| X6 SupersedeTransfer | `UserSend`, `MergeBack`, `Uncertain`, `RecoverDelivery` |
| (none) delivery | `DeliverBatch`, `Inject`, `PersistInjected`, `TurnStart`, `PersistInline`, `Uncertain`, `RecoverDelivery` (T3 `deliverContextHandoffs`; DESIGN.md has no delivery state, see R12) |
| S8 `ContextOrder` | `ContextOrder == NoFuture /\ ExactOrStated /\ NoDuplicate /\ NoSilentDrop` |
| F5 `MergeDeltaDelivered` | witness `W_F5_MergeBack` |
| F6 `SwitchWithPacket` | witness `W_F6_BudgetedSwitch` |
| L5 `TransferResolves` | `TransferResolves` (conditional form, see "Criteria") |
| N1 NativeStart | `Handle` on a `spawn` event |
| N2 NativeEnd | `Handle` on an `end` event |
| N3 NativeOrphan | `Handle` on `rootEnd` (foreground only), on `sessEnd` (background), `Recover` (all) |
| S2 / S13 `NativeNeverEndsRun` | `OnlyRootEndsRun` (alias `NativeNeverEndsRun`), `OrphansClosed`, `NoFalseOrphan`, `OrphanEventuallyClosed` |
| F10 `NativeInTaskList` | `TaskMirror` (alias `NativeInTaskList`), witness `W_F10_TaskCompleted` |

## Modeled vs abstracted

### ContextTransfer

Modeled:
- One append-only journal; message id = position; a thread log is the journal filtered by thread. A source point is an id watermark.
- Fork and subagent spawn are lazy: they record a pending transfer and copy nothing. `Hist(t, w)` is the thread's own messages plus its source's messages up to the fork point.
- Provider threads per (thread, harness), with two views: what the native thread holds (`pcov`, `pst` = told-omitted) and what the engine has persisted (`ecov` = delivered item ids, `seen` = last-seen point, `dpend` = delivery pending).
- Catch-up on every send: if the chosen harness has not seen the whole history, a `provider_switch` transfer is created. This covers A→B, A→B→A (delta since the target last saw the thread), a first run on a harness, and an interrupted turn.
- Strategies: native_resume, native_fork (same harness, strong refs, cut at the turn boundary), delta_since_target_last_seen, full_summary (new native thread; also on a stale resume), portable_replay (fork/spawn into a new native thread), merge_delta, result, and unsupported → failed with a reason.
- Budget: fit, or cut to the anchors (first user, last user, last assistant) plus the newest k items with the omitted ids stated under a coverage header, or fail with reason `budget`.
- Delivery as in T3: one batch per run start, deduped against delivered ids, persisted `pending` before injection, then `injected` or `inline` (with the user message).
- Concurrency: two threads run independent pipelines; the source keeps appending while the target resolves.
- Crash: drops in-flight delivery steps and the provider process. A live turn is interrupted; a starting run is re-driven from its durable Run command. A pending delivery on a native thread is "uncertain": the thread is replaced and context rebuilt (`Uncertain`), or recovery settles it at once (`RecoverDelivery`, R9).
- Merge-back and subagent result append a `ctx` item to the target (T3 detailInTurnItem; DESIGN `MergeBack` part), so later catch-ups carry them.

Abstracted:
- Message text, token counts, the byte budget formula: the budget is a nondeterministic choice among fit / cut(k ≤ MaxCutK) / fail.
- One level of lineage (Main → t2). No rollback, no checkpoints, no summarizer quality.
- Each Resolve is one durable transaction; native fork is atomic. The non-inline `TurnStart` persists the input id atomically with sending it (the inline path keeps the crash window).
- Subagent spawn carries the parent's history (a fork). DESIGN.md sends the task prompt only, which is a special case (empty range).
- Subagent result carries the child's last assistant message. DESIGN.md says "child run start → end".
- Merge-back is refused while the target has active work (T3). DESIGN.md queues it "like a wake"; not modeled.

### NativeSubagents

Modeled:
- One chat, runs in sequence. The harness emits an ordered event stream: `spawn`, `progress`, `end` per subagent activation, `rootEnd` per run, and the engine observes `sessEnd` when the provider process exits.
- A subagent is keyed by toolUseId plus an activation epoch (resume: Codex `resume_agent`, Claude SendMessage). Foreground subagents end inside the root turn (the root waits for its tools). Background subagents may outlive it.
- Engine reducer updates the subagent record (`{chatId}--sub--{toolUseId}`) and the unified task row in the same step (F10).
- Root-only run end with a run-ordinal guard. Orphan close: foreground at run end, background at process end, everything at crash recovery.
- Environment: interrupt (may kill foreground subagents with no end events), process exit between runs, engine crash (kills the provider process, loses events in flight), at-least-once replay of an already applied event, late.

Abstracted:
- The event pipe holds at most `MaxInbox` events (the harness blocks when it is full). Interrupt may overfill it once.
- No result delivery is modeled, only where the end lands (`event` inside the owning live run, `late` otherwise). See R10.
- Native subagents are not admitted or capped (DESIGN.md: S5 applies to app-owned tasks only).

## Bounds

tla-ts 0.0.8 has no simulation mode (`-simulate` is ignored and runs BFS; no `-depth`, no `-coverage`). Every check below is exhaustive BFS. Liveness configs use no `SYMMETRY` and smaller bounds. Fairness is weak fairness on engine steps only; user sends, harness events, crash, replay, stale resume and budget choice have no fairness, and all environment budgets are finite.

| Family | N | N+1 | Fixed |
|---|---|---|---|
| CT switch (`CT_Switch_*`) | 1 thread, journal 6 (3 runs) | journal 7; also journal 8 (`N2`) | harnesses pA (inject) + pB (inline), MaxX 3, crash 1, stale resume 1, budget fit/cut(k<=1)/fail |
| CT fork + merge-back (`CT_Fork_*`) | 2 threads, journal 6 | journal 7 | Main uses pA only, t2 picks pA (native fork) or pB (portable), MaxX 4, crash 1, budget fit/cut |
| CT subagent spawn + result (`CT_Sub_*`) | 2 threads, journal 6 | journal 7 | as fork |
| CT unsupported (`CT_Unsup_N`) | fork family with pB unable to take handoffs | - | - |
| CT liveness (`CT_Live_*`) | switch and fork families at journal 6, no symmetry | - | weak fairness on resolve, delivery, recovery |
| NS (`NS_N`, `NS_N1_Runs`) | 2 subagents, 2 runs, 2 activations, inbox 2 | 3 runs (key dimension) | crash, replay, interrupt, process exit: 1 each; symmetry on subagents |
| NS subagent dimension (`NS_SubsPair_*`) | 2 subagents, 1 activation | 3 subagents, 1 activation | as NS |
| NS liveness (`NS_Live`) | 1 subagent, 2 runs, no symmetry | - | weak fairness on `Handle`, `Recover` |

Key dimension for CT is history length (journal). For NS it is runs, since S2 and S13 are about run boundaries. `NS_N1_Subs` (3 subagents with 2 activations) did not finish in 15 minutes (TIMEOUT, twice); the subagent dimension is checked instead at 1 activation (`NS_SubsPair_N` → `N1`, both pass). Final run: `-workers 4`, one check at a time, on a machine shared with other model runs, so wall times are pessimistic. N+1 adds no violation and no action coverage: every action is already enabled at N (coverage table).

## Results

`NS_N1_Subs` is opt-in (`node run-ct.mjs NS_N1_Subs`); it timed out at 15 minutes twice (see Bounds). All other jobs run by default. `node run-ct.mjs` runs 88 jobs, all as expected, exit 0 (`out-ct/run-all.txt`, `out-ct/results-all.json`).

| job | distinct | depth | result | as expected | secs |
|---|---|---|---|---|---|
| NS_N | 2886661 | 34 | PASS | yes | 49 |
| NS_N1_Runs | 14125289 | 37 | PASS | yes | 191 |
| NS_SubsPair_N | 394237 | 24 | PASS | yes | 5 |
| NS_SubsPair_N1 | 5929022 | 30 | PASS | yes | 70 |
| NS_Live | 47646 | 24 | PASS | yes | 5 |
| CT_Switch_N | 15996 | 24 | PASS | yes | 2 |
| CT_Switch_N1 | 28268 | 25 | PASS | yes | 2 |
| CT_Fork_N | 182716 | 30 | PASS | yes | 13 |
| CT_Fork_N1 | 721958 | 33 | PASS | yes | 43 |
| CT_Sub_N | 37453 | 29 | PASS | yes | 3 |
| CT_Sub_N1 | 287383 | 32 | PASS | yes | 16 |
| CT_Switch_N2 | 47650 | 27 | PASS | yes | 2 |
| CT_Unsup_N | 36870 | 27 | PASS | yes | 3 |
| CT_Live_Switch | 15996 | 24 | PASS | yes | 3 |
| CT_Live_Fork | 182716 | 30 | PASS | yes | 48 |
| NS_N1_Subs (opt-in) | - | - | TIMEOUT at 15 min (twice, earlier runs) | no | 901 |

## Coverage (AC-M5)

Each action has a `Cov_X == ~ENABLED X` invariant; the checker must violate it. Feature-flagged actions are covered in the family that enables them.

34 of 34 actions are enabled in some reachable state (14 NS, 20 CT). `Cov_Fork`, `Cov_MergeBack`, `Cov_ResolveFork`, `Cov_ResolveMerge` use the fork family; `Cov_SpawnChild`, `Cov_SubagentResult`, `Cov_ResolveSpawn`, `Cov_ResolveResult` use the subagent family; the rest use the switch family or `NS_N`.

| job | distinct | depth | result | as expected |
|---|---|---|---|---|
| NS_Cov_StartRun | - | - | VIOLATED Cov_StartRun | yes |
| NS_Cov_PSpawn | 2 | 2 | VIOLATED Cov_PSpawn | yes |
| NS_Cov_PSpawnBg | 2 | 2 | VIOLATED Cov_PSpawnBg | yes |
| NS_Cov_PResume | 49 | 5 | VIOLATED Cov_PResume | yes |
| NS_Cov_PProgress | 4 | 3 | VIOLATED Cov_PProgress | yes |
| NS_Cov_PSubEnd | 4 | 3 | VIOLATED Cov_PSubEnd | yes |
| NS_Cov_PRootEnd | 2 | 2 | VIOLATED Cov_PRootEnd | yes |
| NS_Cov_PInterrupt | 2 | 2 | VIOLATED Cov_PInterrupt | yes |
| NS_Cov_PSessionEnd | 6 | 3 | VIOLATED Cov_PSessionEnd | yes |
| NS_Cov_Crash | - | - | VIOLATED Cov_Crash | yes |
| NS_Cov_Replay | 18 | 4 | VIOLATED Cov_Replay | yes |
| NS_Cov_Handle | 4 | 3 | VIOLATED Cov_Handle | yes |
| NS_Cov_Recover | 3 | 2 | VIOLATED Cov_Recover | yes |
| NS_Cov_Terminated | 618 | 7 | VIOLATED Cov_Terminated | yes |
| CT_Cov_UserSend | - | - | VIOLATED Cov_UserSend | yes |
| CT_Cov_Fork | 6 | 4 | VIOLATED Cov_Fork | yes |
| CT_Cov_MergeBack | 115 | 9 | VIOLATED Cov_MergeBack | yes |
| CT_Cov_SpawnChild | 12 | 6 | VIOLATED Cov_SpawnChild | yes |
| CT_Cov_SubagentResult | 108 | 11 | VIOLATED Cov_SubagentResult | yes |
| CT_Cov_ResolveSwitch | 16 | 5 | VIOLATED Cov_ResolveSwitch | yes |
| CT_Cov_ResolveFork | 16 | 6 | VIOLATED Cov_ResolveFork | yes |
| CT_Cov_ResolveMerge | 537 | 11 | VIOLATED Cov_ResolveMerge | yes |
| CT_Cov_ResolveSpawn | 22 | 8 | VIOLATED Cov_ResolveSpawn | yes |
| CT_Cov_ResolveResult | 908 | 14 | VIOLATED Cov_ResolveResult | yes |
| CT_Cov_DeliverBatch | 28 | 6 | VIOLATED Cov_DeliverBatch | yes |
| CT_Cov_Uncertain | 126 | 8 | VIOLATED Cov_Uncertain | yes |
| CT_Cov_Inject | 72 | 7 | VIOLATED Cov_Inject | yes |
| CT_Cov_PersistInjected | 144 | 8 | VIOLATED Cov_PersistInjected | yes |
| CT_Cov_TurnStart | 2 | 2 | VIOLATED Cov_TurnStart | yes |
| CT_Cov_PersistInline | 125 | 8 | VIOLATED Cov_PersistInline | yes |
| CT_Cov_Complete | 5 | 3 | VIOLATED Cov_Complete | yes |
| CT_Cov_Crash | - | - | VIOLATED Cov_Crash | yes |
| CT_Cov_RecoverDelivery | 268 | 9 | VIOLATED Cov_RecoverDelivery | yes |
| CT_Cov_Terminated | 423 | 10 | VIOLATED Cov_Terminated | yes |

## Mutants (AC-M6)

Each mutant is one guard removed (a `M_*` constant set to TRUE). Each must violate the property it targets, named in the table; a different violation counts as a miss.

21 of 21 mutants caught, each by its target property (11 CT incl. one liveness, 10 NS incl. one liveness).

| job | expected | result | as expected | distinct |
|---|---|---|---|---|
| NS_Mut_AnyEndEndsRun | OnlyRootEndsRun | VIOLATED OnlyRootEndsRun | yes | 140 |
| NS_Mut_NoRunGuard | OnlyRootEndsRun | VIOLATED OnlyRootEndsRun | yes | 629 |
| NS_Mut_NoOrphanClose | OrphansClosed | VIOLATED OrphansClosed | yes | 153 |
| NS_Mut_NoSessClose | OrphansClosed | VIOLATED OrphansClosed | yes | 1447 |
| NS_Mut_NoEpochGuard | NoStaleClose | VIOLATED NoStaleClose | yes | 5726 |
| NS_Mut_ReopenTerminal | OrphansClosed | VIOLATED OrphansClosed | yes | 5288 |
| NS_Mut_NoTaskMirror | TaskMirror | VIOLATED TaskMirror | yes | 153 |
| NS_Mut_NoRecoveryClose | OrphansClosed | VIOLATED OrphansClosed | yes | 155 |
| NS_Mut_OrphanCloseBg | NoFalseOrphan | VIOLATED NoFalseOrphan | yes | 221 |
| NS_Mut_NoOrphanClose_Live | OrphanEventuallyClosed | VIOLATED OrphanEventuallyClosed | yes | 37573 |
| CT_Mut_NoRecoverDelivery_Live | TransferResolves | VIOLATED TransferResolves | yes | 14872 |
| CT_Mut_ReadHead | NoFuture | VIOLATED NoFuture | yes | 19 |
| CT_Mut_InputInHandoff | NoDuplicate | VIOLATED NoDuplicate | yes | 125 |
| CT_Mut_NoDedup | NoDuplicate | VIOLATED NoDuplicate | yes | 144 |
| CT_Mut_NoUncertainGuard | NoDuplicate | VIOLATED NoDuplicate | yes | 1196 |
| CT_Mut_DeltaFromLeaving | NoSilentDrop | VIOLATED NoSilentDrop | yes | 5804 |
| CT_Mut_NoAnchors | ExactOrStated | VIOLATED ExactOrStated | yes | 28 |
| CT_Mut_SilentCut | ExactOrStated | VIOLATED ExactOrStated | yes | 168 |
| CT_Mut_ForkNoRollback | NoFuture | VIOLATED NoFuture | yes | 78 |
| CT_Mut_SkipFailed | NoSilentDrop | VIOLATED NoSilentDrop | yes | 60 |
| CT_Mut_ReplaceNoSummary | NoSilentDrop | VIOLATED NoSilentDrop | yes | 518 |

Notes on traces:
- `CT_Mut_ReadHead`: shortest trace is a catch-up that reads the head and so includes the run's own input (past its source point). The fork scenario (source appends after the fork point, then resolution) is reachable; see witness `W_ConcurrentAppend`.
- `CT_Mut_NoDedup`: turn starts with u1 on pA, crash, next send on pA builds a catch-up that holds u1 again.
- `CT_Mut_NoRecoverDelivery_Live`: A, A, switch to B, `DeliverBatch`, inline `TurnStart`, crash, then stutter: the transfer never settles.

## Witnesses

Each witness is a negated success state; the checker must violate it.

18 of 18 witnesses violated (reachable).

| job | result | as expected | distinct | depth |
|---|---|---|---|---|
| NS_Wit_F10_TaskCompleted | VIOLATED W_F10_TaskCompleted | yes | 140 | 6 |
| NS_Wit_BgOutlivesRun | VIOLATED W_BgOutlivesRun | yes | 221 | 6 |
| NS_Wit_OrphanClosed | VIOLATED W_OrphanClosed | yes | 153 | 6 |
| NS_Wit_ResumedCompleted | VIOLATED W_ResumedCompleted | yes | 5672 | 10 |
| NS_Wit_SessionClosed | VIOLATED W_SessionClosed | yes | 1447 | 8 |
| NS_Wit_RecoveryClosed | VIOLATED W_RecoveryClosed | yes | 155 | 6 |
| NS_Wit_BgResultNoRun | VIOLATED W_BgResultNoRun | yes | 1436 | 8 |
| CT_Wit_F5_MergeBack | VIOLATED W_F5_MergeBack | yes | 6914 | 15 |
| CT_Wit_F6_BudgetedSwitch | VIOLATED W_F6_BudgetedSwitch | yes | 1422 | 12 |
| CT_Wit_ABA_Delta | VIOLATED W_ABA_Delta | yes | 5193 | 15 |
| CT_Wit_ConcurrentAppend | VIOLATED W_ConcurrentAppend | yes | 47 | 8 |
| CT_Wit_CrashMidResolve | VIOLATED W_CrashMidResolve | yes | 521 | 10 |
| CT_Wit_Unsupported | VIOLATED W_Unsupported | yes | 31 | 7 |
| CT_Wit_BudgetFail | VIOLATED W_BudgetFail | yes | 27 | 6 |
| CT_Wit_ReplacedThenFull | VIOLATED W_ReplacedThenFull | yes | 2555 | 13 |
| CT_Wit_NativeFork | VIOLATED W_NativeFork | yes | 28 | 7 |
| CT_Wit_SubagentResult | VIOLATED W_SubagentResult | yes | 6096 | 18 |
| CT_Wit_Inline | VIOLATED W_Inline | yes | 267 | 9 |

| Witness | Shows |
|---|---|
| `W_F5_MergeBack` | F5: a merge-back with a non-empty delta is consumed and its ids are in the target's native thread |
| `W_F6_BudgetedSwitch` | F6: a switch packet consumed with a real cut (omitted ids, coverage header, non-empty selection) |
| `W_ABA_Delta` | A→B→A: the return uses `delta` from A's last-seen point (> 0), after an earlier switch to B |
| `W_ConcurrentAppend` | the source gained messages after the source point before a fork or merge-back resolved |
| `W_CrashMidResolve` | a crash while the target run was starting, then the transfer is consumed |
| `W_Unsupported`, `W_BudgetFail` | failed with reason `unsupported` / `budget` |
| `W_ReplacedThenFull` | uncertain delivery → native thread replaced → full summary consumed |
| `W_NativeFork`, `W_SubagentResult`, `W_Inline` | native fork, subagent result, inline delivery all happen |
| `W_F10_TaskCompleted` | F10: a native subagent shows `completed` in the task row |
| `W_BgOutlivesRun` | a background subagent is still running after its run ended |
| `W_OrphanClosed`, `W_SessionClosed`, `W_RecoveryClosed` | each close path happens |
| `W_ResumedCompleted` | a resumed subagent (activation 2) completes |
| `W_BgResultNoRun` | finding R10 |

## Deadlock and terminal states (AC-M7)

Deadlock checking is on. `Next` has one stutter disjunct, `Terminated`, enabled only in these documented terminal states:
- ContextTransfer `Quiescent`: no run in progress on any thread, no delivery in flight, and the journal (`MaxMsg`) or the transfer budget (`MaxX`) is spent.
- NativeSubagents `Quiescent`: every run used (`MaxRuns`), no root turn, no event in flight, no recovery pending, no subagent running at the harness.

No N or N+1 run reported a deadlock.

## Findings: new requirements (R9+)

Each finding names the job whose log holds the trace (`out-ct/<job>.log`). The real model has no violation. Two kinds of finding:
- Behavior that fails a property or shows a gap: R9 (T3 behavior fails L5), R10 (witness shows a gap in DESIGN.md §6), R11 (DESIGN.md N3 as written fails `NoFalseOrphan`).
- Guards DESIGN.md omits, shown necessary by mutant traces: R12, R13, R14.


- **R9 Settle uncertain deliveries at recovery.** T3 detects an uncertain delivery only at the next turn start (`ProviderTurnStartService.ts:1253`, "Uncertain native history injection"); `ProviderRuntimeRecoveryService.ts` has no handoff-delivery handling. Trace (`CT_Mut_NoRecoverDelivery_Live`): inline packet sent with the turn (`TurnStart`), crash before `PersistInline`; the run is interrupted, the transfer stays `resolved`/`pending` forever if nobody sends again, so L5 fails. Requirement: on recovery, mark such transfers superseded (`uncertain`) and mark the native thread for replacement at once.
- **R10 Deliver background native results that land after the root turn.** DESIGN.md §6 delivers native results "inline to the parent's own run, as the tool result. No wake." Trace (`NS_Wit_BgResultNoRun`): background subagent spawned, root turn ends, subagent completes with no live parent run; its result has no delivery target. Requirement: hold the parent's result while background work runs (T3 `backgroundWorkHoldsCompletion`) and deliver a late background result as a wake or into the next run.
- **R11 Orphan close must skip background subagents.** DESIGN.md N3 closes every running native task at EndRun. Trace (`NS_Mut_OrphanCloseBg`, violates `NoFalseOrphan`): background subagent running at the harness, root turn ends, engine closes it "parent run ended"; its real end later is ignored, so its result is lost. Requirement: N3 closes foreground tasks only; background tasks close on provider-process exit or crash recovery.
- **R12 Persist delivery state per native thread.** DESIGN.md has no delivery state. Traces: `CT_Mut_NoUncertainGuard` (crash after injection, before it is recorded; retry injects again: duplicate) and `CT_Mut_ReplaceNoSummary` (native thread replaced without rebuilding: the next turn misses history). Requirement: persist `pending` before injecting; on restart with a pending delivery, replace the native thread and send a full packet (T3 `ContextHandoffDeliveryUncertainError`).
- **R13 Filter every packet against the ids the native thread already has.** DESIGN.md S8 says "no item twice" per packet. Trace (`CT_Mut_NoDedup`): turn starts on harness A with input u1, crash, next send on A builds a catch-up from A's last-seen point that contains u1 again. Requirement: keep delivered item ids per native thread and subtract them from every packet (T3 `alreadyDeliveredItemIds`). Zeron's `fork_history_prompt` has no such filter today.
- **R14 The switch packet ends before the run's own message.** DESIGN.md says "→ latest". Trace (`CT_Mut_InputInHandoff`): the packet includes the new user message, which then also arrives as the turn input. Requirement: the switch point is the last message before the run's input.

## Modeled choice (no trace)

- A resolved packet for harness A is superseded when the next run uses harness B. DESIGN.md X6 supersedes only pending transfers. The model lets the catch-up rebuild coverage instead; no mutant isolates this rule at these bounds.

## Criteria that look wrong or untestable

- **L5 as written contradicts lazy resolution.** "Every context transfer eventually resolves" fails for a fork nobody messages (T3: a transfer "may stay unresolved until first use"). Checked form: a transfer whose target run has started is eventually consumed, failed with a reason, or superseded, or that run start is aborted with a recorded reason. Suggest rewording L5 to that.
- **S13 "orphans close when the parent run ends" is wrong for background subagents** (Claude background Task, T3 `pendingBackgroundTasks`). They outlive the root turn by design. Checked form: foreground subagents close at run end; background ones close at provider-process exit or crash recovery; an orphan close never closes work the harness is still doing (`NoFalseOrphan`). See R10, R11.
- **S8 does not say whether the anchors must survive a cut.** The task brief requires first user, last user and last assistant in every cut; T3 `selectHistory` and DESIGN.md §5 ("skips an oversized item whole") can drop an anchor and still emit a header. The model enforces anchors (`ExactOrStated`); a cut that cannot keep them fails with reason `budget`. Decide which rule S8 means.
- **S8 "no duplicate" needs a scope.** Per packet it is trivial; the bugs are across packets and crashes (R12, R13). The model checks it per native thread over the whole run.
- **F10 is testable only as projection equality** (task row = subagent record). "Same status words as app-owned tasks" is a UI mapping, not a state property.
- **AC-M3 for NativeSubagents in the subagent dimension.** See the bounds table: 3 subagents does not finish inside 15 minutes on this machine (shared with other model runs). Runs are the key dimension for S2/S13 (orphans cross run boundaries); N+1 runs passes.
- **DESIGN.md bounds table** (CT "2 threads, 4 msgs each, 2 harnesses / 5 msgs"; NS "1 thread, 2 natives, 2 runs / 3 natives") differ from the bounds used here; this README is the measured source.
