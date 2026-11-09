# Build status

Keep `ZERON_ORCH_V2` off by default. The feature code is built. Full runtime
refinement and the required week of use are still in progress. A passing local
test does not close those release checks.

| Build slice | Code | Check |
| --- | --- | --- |
| Save state and effects | `crates/engine/src/orchestrator/{driver,store}.rs` | Atomic receipt, state, trace and outbox writes; crash and retry tests |
| Run and delegate | `crates/orch/src/{run,delegation,followup}.rs`; engine `wire/tasks.rs` | Worktrees, wait and async results, follow-ups, Stop, usage reset, observed-result receipts |
| Ask questions | `crates/orch/src/questions.rs`; engine `questions.rs` and `wire/questions.rs` | Epoch, holder, person answer, blocked ancestor, remote reorder and terminal catch-up tests |
| Move context | `crates/orch/src/transfer.rs`; engine `transfer.rs`; RPC `orch_transfer.rs` | Pinned ranges, packet budget, retry receipt, saved provider binding and strict resume tests |
| Track native tasks | `crates/orch/src/native.rs`; engine `native.rs` and session stream | Foreground and background tasks, late results, queue-full retry, final text and failed-save recovery |
| Use two devices | Engine `remote.rs` and `placement.rs`; document commands; edge delegate route | Two independent stores, durable document channels, offline retry, host checks, stale notices and Stop |
| Show saved parts | Document parts; desktop transcript; mobile rows | Document round-trip tests, desktop fixture captures, desktop and mobile compile checks |
| Check the build | `scripts/orch-*`; `.github/workflows/orchestrator-verify.yml` | Reducer tests, model checks, trace merge and replay, proof and release gates |

The two-device tests use provider stand-ins and local document channels. They do
not prove a deployed Cloud service or a real provider connection. The desktop
captures show synthetic saved parts through the app's transcript. They do not
prove the whole task flow through the UI.

## Keep proof scope clear

- Check all 73,134 transitions in the saved delegation graph through the Rust
  reducer. New question, native, remote and transfer paths also have focused
  tests; the old graph alone does not cover them.
- Check generated event sequences with proptest. The three fast Kani harnesses
  add evidence for their named fixtures. They do not prove every Rust state.
- Prove S3 against a generated source wrapper of the checked Delegation model.
  The wrapper generator checks its source link and inverse transform.
- Prove S9 and S10 for the stated Placement protocol. These proofs have fixed
  hosts and exclude rollback. They do not prove every engine effect.
- Replay compact traces through the Rust reducer and a separate TLA receipt
  model. This is partial evidence. Full acceptance also needs mechanism state,
  a complete action map and the Composition refinement check.

See [verification](../orchestrator-v2-verification.md) for commands and tool
limits, and [remaining conformance work](../orchestrator-v2-conformance-gap.md)
for the full trace contract. The approved source documents and their hashes
remain unchanged in this directory.

## Keep release blocked

The fast job can pass while the separate acceptance job stays red. The latter
requires the full conformance checker, current proof receipts and a validated
week of use. Synthetic tests and repeated copies of one database do not count
as that week. Raw runtime captures remain local.

The local runner is `zeron-orchestrator-mac`, with the `orchestrator-verify`
label. The user launchd job `com.zeron.orchestrator-nightly` is set for 02:10
local time. It uses a separate checkout and only fast-forwards the approved
branch. Runner setup does not count as a successful workflow or nightly run.

The Mac workflow accepts branch pushes and manual runs. It has no pull-request
trigger: a fork can change the workflow file used by that event. See
[GitHub's event rules](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target).

## Record model corrections

**R43: Keep transfer anchors and stop on a failed transfer.** A cut must retain
the first user, last user and last assistant messages required by S8a. If those
items cannot fit intact, the transfer fails with a budget reason. The run start
must then stop with a visible reason. The older budget text that skips large
items applies only when the required anchors remain intact. This follows the
checked ContextTransfer rules and L5a; it does not permit a provider to run with
silently missing context.
