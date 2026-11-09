# State-space trimming notes (not applied)

Scope: cuts that keep every behavior. Nothing here was applied to other agents' files. Each item says what to change, why it is safe, and the condition that keeps it safe.

## Rule for VIEW cuts

A variable may leave the VIEW only if all three hold:

1. **No guard or other update reads it.** It is read only by its own update.
2. **No checked invariant or property reads it.** TLC skips invariant checks on a state it merges by VIEW.
3. **The config checks safety only.** VIEW is unsound for liveness checking, so leave it out of `*_Live*` configs.

Budget counters (`crashLeft`, `dups`, `offs`, `personLeft`, ...) gate later steps, so they always stay. Sticky error flags (`replayBad`, `receiptBad`, `badHost`) are FALSE in every good state, so they cost nothing and give nothing to cut.

## Per module

| Module | Change | Safe because | Condition | Expected gain |
|---|---|---|---|---|
| Delegation | `VIEW` without `kidIn` | Only `DoNotice` (its own update) and `W_F1_GrandchildToRoot` read it | Every config except `MC_Del*` runs that check `W_F1` | Up to 2^children in states where grandchildren were pushed; most at depth 2 (`MC_Del_Deep`, `MC_Del_Q`) |
| Delegation | Keep `SYMMETRY D_Sym` on N+1 BFS attempts. `MC_Del_N1` is sim only today. | Children are interchangeable | No config names a specific child | 3! = 6x at 4 threads |
| Delegation | `doomed`, `pushes`, `replayable`, `zWait`, `answers` stay | They are read by guards (`Delegate`, `CrashEngine`, `EndRun`) or by checked invariants (S3, S7, S11, S12) | - | - |
| RunLifecycle | `VIEW` without `why` | Only the `W_*Completed` witnesses read it. `RL_TypeOK` reads it only for its type. | Not in witness configs | Up to 4x per attempt (initial, steering_restart, retry, provider_recovery) |
| RunLifecycle | `steers`, `lastOut`, `rkind`, `retries` stay | `RL_NoLostSteer` is checked; `UsageLimitRecovery`, `IngestEnd` and `MayRetry` read the others | - | - |
| ContextTransfer | `VIEW` without `abort` | Only `Fail` (its own update) and the liveness property `TransferResolves` read it | Safety configs only (all `CT_*_N`, `CT_*_N1`, mutants, witnesses) | Up to \|Why\|^threads |
| ContextTransfer | `dup` stays | `NoDuplicate` reads it in every main config | - | - |
| NativeSubagents | `endBy`, `how`, `closedEp`, `task`, `pprog` stay | Read by checked invariants (`OnlyRootEndsRun`, `NoStaleClose`, `TaskMirror`, `NoFalseOrphan`) or by guards (`pprog` in `PProgress`) | - | - |
| NativeSubagents | `SYMMETRY Sym` already used | - | - | - |
| Placement | No VIEW cut | `issued`, `relayLog`, `wake`, `seq` gate steps; `execBy`, `badHost`, `dispatched` are S9/S3Safe; `handoff`, `view`, `emitDue` are constant unless move/mutant flags are on, so they cost nothing | - | - |
| Placement | No symmetry | Each device has a fixed role (ParentHost, ChildHost, UserDev). In BaseN1 the third laptop D is the only extra holder. | - | - |
| Placement | Add the Apalache per-action induction (`apalache/run-ind.sh`) next to the BFS for S9; it does not replace it | One step from ANY IndInv state covers states BFS never reaches; it found the CTI that BFS cannot | Needs the `OutWithCmd` strengthening; runs only at MaxSeq=1 (MaxSeq=3 ran out of 4 GB heap) | Not faster: 90-215 s per action, about 1 h single-thread for all ~30 actions |
| ZI_ZeronImpl | `jseq` costs nothing today: it equals \|jr[d]\| unless the `NoJournalGate` mutant is on | It is a pure trace counter | If later uses let it diverge, drop it from VIEW | - |
| ZI_ZeronImpl | Add `SYMMETRY Permutations(Children)` at 3+ threads | Children are interchangeable: one host (`ChildDev`), no child named in properties | Do not add for witnesses that name a child | 2x at 3 threads |

## Not recommended

These cuts hide behavior. List them so nobody applies them by mistake.

- **Do not merge follow-up steps.** Examples: Placement `Exec`+`Resolve`; ZI `*Write`+`*Mark`, `DispatchRun`+`DispatchMark`, receipt+`MarkLedger`. The gap between them is exactly the fault point (FP1-FP4) that the model must cover.
- **Do not renumber run ordinals or generations.** `tCoh`, wake messages and `cGen` compare these values directly.
- **Do not lower `MaxRuns` below its current value to save states.** `D_BoundsNotHit` and `RL_BoundsNotHit` show the bound never bites, so a lower bound removes no reachable state. It only risks a silent cut.

## Applied (2026-11-08, Job 6)

| Module | Cut | Where | Measured |
|---|---|---|---|
| RunLifecycle | `VIEW RL_View` (no `why`) | `MC_RL`, `MC_RL_N1` (gen.mjs `view: true`; never with W_*Completed witnesses, liveness, coverage or TLC sim) | P=1: 81,979 to 62,447 distinct (-24%). `MC_RL` (P=2): 1,136,086 distinct, PASS |
| Delegation | `VIEW D_View` (no `kidIn`) | `MC_Del`, `MC_Del_Deep`, `MC_Del_Q`, `MC_Del_N1x` (same exclusions, never with W_F1) | Fast `Del_Q`: 164,231 to 157,775; no gain at depth 1 (kidIn needs grandchildren) |
| ContextTransfer | `VIEW` without `abort` | tried on all 52 CT configs, then removed | 0 states saved in every config: `abort` is a function of the other variables, so the cut buys nothing |
| ZI_ZeronImpl | `SYMMETRY` over children | owned by the ZI configs (see ZI report) | - |

tla-ts 0.0.8 honors `VIEW` (checked on RL at P=1 before applying). Any new Delegation or RunLifecycle variable must be added to `D_View` / `RL_View`, or VIEW configs merge states that differ in it.
