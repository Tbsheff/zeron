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

## Record local checks

Checked on 2026-11-09. Passed 54 orchestrator unit tests, 25 transfer tests,
and all 68 runtime integration tests after the final boot fixes. Passed all
547 harness library tests, strict resume, and compact replay of 75 recordings.
The harness receipt uses the same unchanged harness source. These tests use
provider stand-ins; they do not prove deployed provider or Cloud behavior.

The final engine gate kept its source and binary hashes unchanged. Its source
manifest is `04d5c3220c702aee838fd426b737dcbf0595705e95f9b12945f0765e16b0f17e`.
The isolated app built and passed the signature check. It has not been opened.
See `CHECKS.json` for the build and test receipt summary. The full release gate
remains open.

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

See [the isolated build guide](TRY.md) for the staging profile, local port and
manual use cases. Use that profile for the real-use window; automated trials
alone do not start it.

## Keep release blocked

The fast job can pass while the separate acceptance job stays red. The latter
requires the full conformance checker, current proof receipts and a validated
week of use. Synthetic tests and repeated copies of one database do not count
as that week. Raw runtime captures remain local.

The local runner is `zeron-orchestrator-mac`, with the `orchestrator-verify`
label. The user launchd job `com.zeron.orchestrator-nightly` is set for 02:10
local time. It uses a separate checkout and only fast-forwards the approved
branch. Runner setup does not count as a successful workflow or nightly run.
The first manual job passed runner, checkout and tool setup. It passed the
formal, reducer, selected Kani, S3 projection and trace merge checks at
`1f85467b`. Integration and capture inventory failures keep that run red;
the next source batch fixes them. Strict branch checks require
`orchestrator-fast` and `orchestrator-verify`. The owner can still push
development checkpoints; that bypass does not approve a release.

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

**R44: Keep reporting loss separate from process death.** A closed provider
stream proves that this connection cannot report more results. It does not
prove that the provider process died. The new operational native model keeps
those facts separate and binds task closure to its source runtime. It checks
root ownership, orphan closure, background lifetime and saved task documents.
The old process model and its results remain separate. The new product and
its full refinement are still being checked. Concurrent old and new runtime
ownership remains an unproved candidate; normal replacement waits for teardown.

**R45: Buffer a terminal event before closing its run.** A provider can report
completion while a wait call is still open. The engine first saves that event,
abandons the open wait, then ingests the terminal event. Permit observation
into the inbox while the wait is open. Keep the guard that prevents the run
from closing until the wait clears. The model update and its checks are in
progress.

**R46: Bind calls to the issued attempt.** A live logical run alone does not
authorize an old MCP process. Require its actual runtime id, accepted attempt
and current access. Install, request issue, reconfigure and accepted steer are
distinct events. A real tool call can occur inside `Harness::run` before that
call returns its stream. That call does not prove provider context acceptance.
The operational model must permit this order without inventing that proof.

**R47: Recover uncertain context with a fresh provider identity.** Providers
report session identity at different points in prompt submission. Neither a
missing nor a present `SessionStarted` proves context acceptance. Keep an
unconfirmed packet intact, retain external source ranges outside the target
transcript, and retry uncertain delivery with a fresh provider identity. Keep
same-input packet bytes stable. Apply one total budget to recovered and new
ranges. A saved budget failure must keep failing on retry. Only a saved
successful-turn delivery receipt confirms delivery under the adapter contract;
it does not prove what an opaque language model retained internally.

**R48: Prepare context after the logical run starts.** The reducer starts a
logical run before the engine selects its provider and prepares its context
packet. Model these as separate steps. A prepared request records its provider,
source ranges and packet. A saved successful-turn delivery receipt confirms
delivery under the adapter contract. The logical terminal can arrive first; a
failed confirmation save must leave delivery uncertain. Stream life and session
identity do not prove that context was accepted. Keep the earlier
ContextTransfer proof separate from this operational model. Its joint safety,
progress and actual trace checks remain in progress.

**R49: Recheck effect ownership and retire observed effects atomically.** An old
pending snapshot must not execute an effect that another worker has completed.
Check its durable state after claiming it. Keep per-thread order while an older
effect remains pending. Publish and retire an already-satisfied interrupt effect
in one transaction; separate cleanup can let a claimed interrupt stop a newer
runtime. Boot recovery uses the same host, journal, attempt, input and Stop
checks as normal dispatch. Register a lost attempt before adding its revival
marker; that marker must not make recovery reject itself as an existing runtime.
Keep workers and direct drains blocked until recovery succeeds. A failed scan
must keep that gate closed. The boot and effect regression tests pass.

**R50: Pin a question before its callback is queued.** A provider request must
keep the source attempt that issued it. An async approval hook or spawned task
must not let it use a later turn. Accepted steers need an ordered source cursor;
the engine stream can consume their notices later. Record the read position of
both provider requests and successful steering replies before they enter
separate queues. Keep requests on the correct side of that reply. Validate the
pinned attempt again when applying the question. This proves order against the
adapter acknowledgment; it does not prove an opaque provider-internal turn
identity. The harness order checks and engine source-binding regressions pass.
These checks do not close full implementation refinement.
