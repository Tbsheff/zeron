# Placement (Zeron Orchestrator v2, AC-M1 module) and TLAPS setup

Checker: tla-ts 0.0.8 (`../node_modules`). Prover: tlapm 1.6.0-pre (`--version` prints `c8ddc44`).

## Files

| File | What |
|---|---|
| `Placement.tla` | The model: devices, hosts, chat ledgers, relay, offline, Cloud sleep, crash, rollback, host move (flag). |
| `MC_PL_*.cfg` | One config per run (passing, liveness, witnesses, findings, mutants). The first line of each says what it checks. |
| `run-pl.mjs` | Runner. `node run-pl.mjs` runs every config; `node run-pl.mjs MC_PL_Base` runs one; `node run-pl.mjs --coverage` writes `out-pl/coverage.json` (AC-M5). Uses `-workers 4`. |
| `out-pl/` | Logs, `results.json`, `coverage.json`, proof logs (`proof*.log`). |
| `PlacementInv.tla` | The S9 inductive invariant `IndInv` (no TLAPS import, so tla-ts can check it). |
| `PlacementProof.tla` | TLAPS proof of S9 (`OneHost`) for the v2 rules. |
| `PlToyProof.tla`, `PlToyBroken.tla` | Toy proof (passes) and its broken twin (must fail): checks the prover install. |

## tlapm install

- **Installed** the official release `tlapm-1.6.0-pre-arm64-darwin.tar.gz` (github.com/tlaplus/tlapm, rolling pre-release of 2026-11-07) into `~/.local/tlapm`. No sudo, no Homebrew formula exists. Backends ship inside: z3 4.8.9, zenon, Isabelle, ls4.
- **PATH:** add `export PATH="$HOME/.local/tlapm/bin:$PATH"`. Shell profiles were not changed.
- **Layout** matches what `tla-ts prove` (newer tla-ts, `run-cli.ts:321-339`) looks for: `bin/tlapm` and `lib/tlapm/stdlib` beside it.
- **Toy proof** (`PlToyProof.tla`, 2-variable counter, inductive invariant, PTL step): 12/12 obligations proved by `tlapm`, by `tla-ts tlapm` (0.0.8), and by the vendored newer `tla-ts prove` run from source (`bun …/vendor/tla-ts/packages/tla-ts/src/cli.ts prove`), which spawns tlapm from PATH. `PlToyBroken.tla` (guard removed) fails 1/12 in tlapm and in `tla-ts tlapm`.
- **Note:** tla-ts 0.0.8 has no `prove` subcommand (`tla-ts prove X.tla` is read as a model-check run). Its `tla-ts tlapm` uses its own provers. CI that wants `tla-ts prove` needs the newer tla-ts.

## Model

```
 person on B ──Run row──► P ledger (chat2 room) ──pull──► host A drains ──► parent run
            └─RELAY_COMMAND──────────────────────────────► host A (same claim)
 parent run on A ──kick row──► K ledger ──wake receipt──► Cloud C wakes, drains ──► child run
 child done row in K ──(A reads K's room)──► A settles res (fixed id ResId) ──► A drains ──► wake run gets result
                      └─(push road, needs a new edge grant)── C writes res into P
```

- **Ledger.** Rows replicate by union (Loro import); push sends the whole local outbox; pull imports the whole room. Every guard reads sets, so order inside one room snapshot does not matter. Duplicates come from the two roads and from relay redelivery (`DupBudget`).
- **Host drain.** `Claim` marks the processed ledger first (mark-before-execute). `Exec` lands the message and dispatches the provider (one step; see assumptions). `Resolve` writes the outcome.
- **Modes.** `Mode = "zeron"`: today's rules (ledger keyed on command id only, crash sweep rejects, TTL on every Run, no dispatch gate on message id, sleep checks turns and waiting commands only). `Mode = "v2"`: ledger also keyed on message id (`orch_receipts`, DESIGN §4), replay completes from the run journal or the doc, orchestrator entries carry no TTL, Cloud sleeps only with a flushed outbox and no undrained wake receipt.
- **Host move.** `"off"` (Zeron today), `"naive"` (registry flips, old host keeps draining), `"safe"` (fenced: `Release` then `Accept`).
- **Names from DESIGN.md.** `OneHost == S9`, `ReplayIsStutter == S10`, `RemoteChildResult` (pull-road settle, F11).

### Assumptions (stated, not checked)

- Dispatch and the landed user message commit together. In the code, `write_user_message` runs before dispatch; a crash between them leaves a landed but unrun message. RunLifecycle owns that window.
- Relay loss happens only while the target is offline (H). Laptops come back on their own; a sleeping Cloud machine wakes only on a wake receipt.
- No clocks. Ids are `<<device, seq>>`; the result id is `<<child, child run>>`.
- `CommuteOK` (DESIGN §4 trace merge) is not in this module.

## Properties

| Id | Formula | Meaning |
|---|---|---|
| S9 | `OneExecutor /\ HostOnly /\ NoDoubleDispatch` | One executor per command id; only the host dispatches; a message id is dispatched at most once. |
| S3 (cross-device) | `S3Safe` + `LRes` | The parent gets the child's result at most once, only after the child finished, and eventually. |
| S10 | `ReplayIdem /\ OutUnique` | Replay applied to its own result changes nothing; an attempt never gets two outcomes. |
| L | `LRun`, `LKick`, `LChild` | A send runs (or visibly expires); a delegation runs on its host; the child finishes. |
| F11 | `F11Witness` (must fail) | A child on Cloud completes and its parent on laptop A gets the result. |

## Results (tla-ts 0.0.8, `-workers 4`, exhaustive BFS)

Base bounds (AC-M2): laptops A, B + Cloud C; P on A, K on C; budgets crash 1, offline 1 (A, C), expire 1, retry 1, duplicate 1, MaxSeq 3. Passing configs also check deadlock (AC-M7): the only end states are `TerminalOK` (result delivered, or a send dead and given up).

| Config | Checks | Distinct states | Depth | Result | Time |
|---|---|---|---|---|---|
| MC_PL_Base | S9, S3Safe, S10, TypeOK, deadlock | 858,582 | 38 | Passes | 131 s |
| MC_PL_BaseN1 | same, AC-M3 N+1: third laptop D holds P | 7,418,740 | 40 | Passes | 500 s |
| MC_PL_Laptop | child on laptop B | 1,097,595 | 38 | Passes | 110 s |
| MC_PL_Push | both result roads (expire/retry/dup 0) | 1,038,415 | 33 | Passes | 183 s |
| MC_PL_Retry2 | two person retries (crash/dup 0) | 972,894 | 40 | Passes | 72 s |
| MC_PL_Rollback | SQLite rollback (crash 0) | 2,326,642 | 38 | Passes | 144 s |
| MC_PL_MoveSafe | fenced host move (expire/retry/dup 0) | 3,171,058 | 37 | Passes | 212 s |
| MC_PL_Live | LRun, LKick, LChild, LRes (only Cloud sleeps, dup 0) | 159,822 | 36 | Passes | 60 s |
| MC_PL_LivePush | same, push road | 301,445 | 31 | Passes | 118 s |

**Witnesses (must fail, all do):** F11 (depth 16), retry-after-expiry runs once (8), safe handover still delivers (17).

**Zeron today and naive move (expected failures):**

| Config | Fails | Depth | Trace |
|---|---|---|---|
| MC_PL_Zeron | S9 (`run` dispatched twice) | 11 | Expire, RetryUser(A), Claim, RetryUser(A), Claim, Exec, Exec |
| MC_PL_ZeronLive | LRun | 18 | Relay, IngestClaim, Crash(A), Sweep: the send is rejected and needs a person |
| MC_PL_MoveNaive | S9 (HostOnly) | 6 | IngestClaim on A, MoveNaive, Exec on A after the registry flipped |

**Mutants (AC-M6), one guard removed, each on a passing twin config:**

| Mutant | Twin | Fails | Depth |
|---|---|---|---|
| NoHostCheck (claim without positive is_host) | Base | S9 | 4 |
| NoLedgerCheck (claim ignores processed ledger) | Retry2 | S9 | 11 |
| NoMidGate (ledger keyed on command id only) | Retry2 | S9 | 11 |
| ReplayNoRanCheck (replay ignores journal and doc) | Base | S9 | 7 |
| ReplayRequeue (replay re-queues a fresh attempt) | Base | S10 | 4 |
| NoDocGate (dispatch trusts SQLite, not the doc) | Rollback | S9 | 8 |
| NoWakeReceipt (room ACK without host wake receipt) | Live | LKick | 20 |
| VolatileEmit (push-road result held in memory) | LivePush | LRes | 25 |
| SleepUnflushed (Cloud sleeps with unacked rows) | Live | LRes | 19 |
| SleepUndiscovered (Cloud sleeps with an undrained receipt) | Live | Passes: redundant here, the receipt stays at the edge and re-wakes the machine | - |

NoLedgerCheck passes at retry budget 1: the doc gate covers it. It fails only with two retries.

**Coverage (AC-M5): all 32 named actions of `Next` are enabled in some reachable state.**

- `node run-pl.mjs --coverage` dumps the labelled state graph of seven small configs (`MC_PL_Cov*.cfg`) and lists the actions that fired: 24 of 32 (`out-pl/coverage.json`). tla-ts holds the whole dot graph in memory, so configs above about 250k states run out of heap; the move and retry configs could not be dumped.
- Witness invariants cover the other 8: `RetryUser`, `ExpireA` (MC_PL_W_Retry trace), `MoveNaive` (MC_PL_MoveNaive trace), `Release`, `Accept`, `RegistrySync` (MC_PL_W_Sync), `ClaimDupA` (MC_PL_W_Dup), `IngestDupA` enabled (MC_PL_W_IngestDup), `RequeueA` (MC_PL_W_Requeue).
- Mode-gated actions are dead by design in the other mode: `SweepA` only in Zeron mode; `ClaimDupA`, `IngestDupA`, `ClaimLandedA`, `RecoverDoneA`, `RecoverRedoA` only in v2; `EmitResult` and `RequeueA` only under their mutants; move actions only with `HostMove` set.
- **AC-M3:** BaseN1 (third laptop) passes with no new violation. Its action set was not dumped (7.4M states); D runs the same parameterized actions as B.

**Proof invariant check:** `MC_PL_IndInv` model-checks the S9 proof's `IndInv` (`PlacementInv.tla`) on the liveness bounds: passes, 159,822 states. So the open proof steps need proof work, not a different invariant.

## Findings (provisional ids PL-1..PL-4; R9-R15 are taken in README.md, the coordinator assigns R numbers)

- **PL-1 (model plus code path, not reproduced): a person's retry can run one message twice.** `retry_delivery` (doc_host.rs, "Dead attempts") counts a pending attempt as live only if it is not in the local processed ledger, so an attempt the host already claimed is not live. During claim → `resolve_request_attachments` / worktree creation → `write_user_message`, a second retry mints another id. `write_user_message` dedupes the entry by message id (doc_host.rs:882-890), but `dispatch_inner` (sessions.rs:836) has no message-id gate, so the provider runs twice. Two devices that each see the dead attempt can do the same. Model trace (MC_PL_Zeron, 11 steps): Expire, RetryUser, Claim, RetryUser, Claim, Exec, Exec. Fix: key the executor receipt on the message id before dispatch (DESIGN's `orch_receipts` rule, extended to Run/Steer), or count claimed-unresolved attempts as live.
- **PL-2 (refines DESIGN §2/§4): dispatch must check the doc, not only SQLite.** An update-trial rollback restores `processed_commands` and `orch_receipts` but not docs. Re-applying an entry with no receipt re-dispatches a run that already landed (NoDocGate, 8 steps). Receipt-then-mark needs a doc check (landed message or notice id) before any provider start.
- **PL-3 (model plus code path, not reproduced): Cloud sleep must wait for the chat2 outbox.** `quiesce_for_sleep` (doc_host.rs:1155) declines on turns in flight and waiting commands, not on unacked rows. A child's final notice can sit on a sleeping machine, and the parent never gets the result (SleepUnflushed, LRes). Likelihood is low (idle minutes first), higher when the edge is unreachable at turn end.
- **PL-4 (refines DESIGN §4 "fenced handoff"): host handover needs four conditions** (MoveSafe passes, MoveNaive fails in 6 steps): the old host stops draining and has no claim in flight; it publishes rows, ledger ids, receipts and journal in one release; the new host accepts only after it holds every released row; non-hosts refuse stale relays.
- **Open:** the real device-room wake is consumed at delivery, and the host keeps its own discovery receipt. If a Cloud machine can sleep between that ACK and joining the room, nothing wakes it again. The model keeps the receipt at the edge, so it cannot show this. Check the edge idle accounting.
- **Confirmed from DESIGN, not new:** orchestrator entries need no TTL (ZeronLive: crash, sweep, the send is lost without a person); a Cloud child cannot write its parent's room, so the pull road is the design (Push config shows both roads together stay exactly-once).

## S9 proof (AC-M4, `PlacementProof.tla`)

Scope: v2 rules, no host move, no rollback, no mutant (fixed by `ASSUME`); any number of devices, any budgets. Inductive invariant: `TypeOKP` plus 12 conjuncts (`CmdShape`, `IdUnique`, `NetOK`, `OutAtHost`, `MsgAtTarget`, `DispatchDoc`, `ProcAtHost`, `LiveIsOpen`, `OpenUnique`, `OpenMid`, `ExecNotRun`, `ExecByHost`).

- **Status: proved (2026-11-09).** `IndInv` now also has `OutWithCmd` (an out row sits in the same row set as its command row, in the command's chat; Apalache found the CTI without it). tlapm proves `InitInv`, `StepInv` (all 33 action cases, through shared lemmas `Fresh`, `ProcOwn`, `IdChat`, `OutChat`, `CRStep`, `OWRep`, `FrameOpen`), `InvS9` and `S9Safe`: **3,280 of 3,280 obligations** in one full run (`out-pl/proof9.log`, with fingerprints). The proof adds `ASSUME TwoHosts == ChildHost # ParentHost`, which Placement.tla already assumes. `MC_PL_IndInv` passes with the new conjunct (159,822 states).
- **S10 (step form).** `ReplayStepIsStutter` (Placement.tla; the state form `ReplayIsStutter` is unchanged): a replay step on an item whose receipt exists (an out row for the attempt, or the message already ran here per journal or landed message) leaves the domain variables unchanged. The processed id is not the receipt, because the ledger is marked before execution. Model check: `MC_PL_S10Step` passes (858,582 states); `MC_PL_M_ReplayRequeueS10` fails it in 7 states. Proof: `PlacementS10Proof.tla`, **803 of 803 obligations** (`out-pl/s10proof.log`), using IndInv plus `JournalMid` and `ExecNoJrnl`; it takes `StepInv` from PlacementProof.tla, so CI runs both files.
- **CI risk:** some older steps use `SMTT(40)` near their limit; a cold `--nofp` run was not done. Add `--stretch 2` if a step times out.
- **Effort estimate (one engineer with tlapm):** close the S9 tail, 1-2 days (two shared lemmas: fresh ids are unprocessed; only the host writes outcome and message rows; then per-case hints). S3 at Placement level (`S3Safe`), 0.5-1 day after that: one more conjunct, `S3Conj` in `PlacementInv.tla`, already holds in all reachable states of MC_PL_IndInv; the case generator is reused. Full S3 (exactly-once task results, settle keyed on child and child run, never delivered and disposed) lives in Delegation/Composition: 4-6 days, crash and duplicate paths are most of it. S10: prove DESIGN's action form `ReplayIsStutter` (a replay step on a receipted item changes nothing), 1-2 days, since it follows from the `HasOut` / `RanBefore` guards. The state form here (`ReplayIdem`, equality over a set-comprehension operator) suits SMT poorly; this module's alias `ReplayIsStutter == S10` is the state form, not DESIGN's.
- **Rerun:** `PATH=$HOME/.local/tlapm/bin:$PATH tlapm --threads 4 PlacementProof.tla` (about 15 min cold, fingerprints in `.tlacache`).
