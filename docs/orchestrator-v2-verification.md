# Verify orchestrator v2

Keep the flag off. The branch has checks for the reducer, durable receipts, and
bounded formal models. It does not yet have full runtime-to-ZeronImpl conformance
or a completed week of use. These are release blockers.

Run these commands from the branch root:

```sh
scripts/orch-setup.sh
python3 scripts/orch-verify.py fast
python3 scripts/orch-release-check.py
```

Use a local macOS runner with labels `self-hosted`, `macOS`, and
`orchestrator-verify` for the push jobs. The workflow accepts only same-repo
branch pushes and manual runs on `feat/orchestrator-v2`; it has no PR trigger.
Fork code must not execute on this runner. The workflow never selects a paid
hosted runner. Add both `orchestrator-fast` and `orchestrator-verify` as required
branch checks in GitHub settings;
source files cannot set that rule. Install Rust from the repo toolchain, Node,
Python 3, Java 17+, TLAPS, and Kani on that runner. The setup script only installs
branch-local npm tools and TLC. It fails if the native proof tools are absent.
Run `cargo kani setup` once after an explicit Kani install. TLAPS used for the
saved proofs is 1.6.0-pre; its path must be in `PATH`. Tool absence is a failure.

Save receipts in `target/orch-verify/`. Each command has a log, elapsed time, and
exit result in `receipts.json`. A failure or timeout fails the job. Rust uses
three build jobs. Formal walks use four workers and a cap of at most five
minutes per walk. The fast suite has its own two-minute cap per walk. Native
builds and the full set of short walks have a larger total cap. The release
S9/S10 TLAPS proofs run in sequence with four workers and a twenty-minute
cap each; this is separate from the model-walk cap.

## Pin model tools

The text files in `verification/orchestrator/tla/` came from the approved v2
model work in the design workspace. No generated states, logs, or npm modules
are checked in. The npm lock pins `tla-ts` 0.0.8 with registry integrity hashes.
The setup script fetches TLC from the `v1.8.0` release URL and checks SHA-256:

```text
e1f11b75c4b5bc4478ba0ac5cb6ae28e40a01b0016bc33e0aa97da22dfdbcd7f
```

The jar manifest gives source commit
`d8fb231de4e4b5d02c1b2ed1ac759c086f694021` in
<https://github.com/tlaplus/tlaplus/tree/d8fb231de4e4b5d02c1b2ed1ac759c086f694021>.
Verify the source revision in the fetched jar with:

```sh
unzip -p verification/orchestrator/tools/tla2tools.jar META-INF/MANIFEST.MF
```

The release URL itself can change; the mandatory content hash prevents accepting
a replaced artifact.
For offline setup, set `TLA_JAR` to a local copy with that hash. The file stays
ignored. TLAPS and Kani remain explicit host tools, not vendored binaries.

The fast model suite is a reduced-bound regression gate. It includes each
module, witnesses, mutants, and refinement checks. It does not replace the full
bounds in DESIGN. Some N+1 runs need more than five minutes. The nightly gate
reports that as a failure, not an exhaustive pass. The fast gate checks `ResultS3Proof`, a partial protocol theorem. The separate
release gate generates a byte-linked source wrapper from the vendored
Delegation module and checks `proof-wrapper/FullDelegationS3Proof.tla` with
fresh obligations. The corrected full source-linked S3 theorem passed144/144
obligations; the release lane checks it again from current source. The
projection alone does not close AC-M4. The `orchestrator-fast` check can pass during the week. The separate required
`orchestrator-verify` acceptance check needs that pass and runs the release
gate, so it stays red until release criteria pass. Kani checks only the bounds of each named harness, not all Rust states. The
fast tier runs only `s1_restart_bound3`, `s3_stop_bound3`, and `s5a_narrow`,
with three jobs and a five-minute cap per scope. These checks are optional evidence; they do not fail the required fast gate. The outer cap is sixteen minutes. AC-I4 permits Kani or proptest. The core proptest suite checks S1, S3, S5a,
and S6 after each accepted step over bounded generated sequences; it runs in
`cargo test -p zeron-orch`. Selected Kani scopes add proof evidence, with their
fixture limits stated in the proof report. Unfinished extra Kani scopes are
optional and do not add a release blocker. Use `python3 scripts/orch-kani.py
status` to list all scopes and source-linked receipts, or `run-all` to request
an explicit run. Each proof has a five-minute cap. Changed source or changed
logs invalidate receipts. `ORCH_KANI_EVIDENCE_DIR` can retain them outside the
checkout.

## Merge runtime traces

```sh
tools/orch-trace path/to/device-a.jsonl path/to/device-b.jsonl
```

The merge builds a directed graph. Per-device sequence order and exact message
send/effect-to-receive ids add edges. A stable topological sort adds no clock
order. Missing sequence rows, repeated sequence rows, ambiguous remote origins,
and cycles fail. A local command may have no send row. Remote trace rows retain their Driver
key in `cmd` and use the stable wire id in `msg`. A receive with a
`remote-recv:` command requires its producer trace. Each remote send is traced
after the durable append. Retries use the first producer edge. The append-to-
trace crash window can still leave a missing send trace and fails capture.

The Rust adapter replays each device partition against `step()`, compares
state hashes and effects, and checks invariants. The separate `TraceLedger`
TLA module replays the receipt/retry projection: each fresh key is new, each
duplicate has a receipt, duplicates keep the post hash and emit no effects.
This is executable TLA checking of one durable mechanism. It is **not** full
ZeronImpl conformance. It cannot check host journal, doc replicas, full outbox
progress, or the refinement mapping from an opaque post hash. The prior
`OrchTrace_*` export checks schema only. Do not count that as conformance.

The integration fixture exports its reference run and all FP1–FP6 fault runs
when `ORCH_TRACE_OUT` is set. The gate checks every exported JSONL, separately,
so recordings with the same device id do not collide. Multi-device fixtures
put device files in one subdirectory under `orch-trace/`; the gate merges that
group together. New integration suites
must export every trace to this directory. Full integration trace coverage
still needs an inventory as more suites add mechanisms.

## Run nightly checks

Use a local checkout and an idle machine. The script takes a local lock. It
runs N+1 module checks and the one-million-trace Composition simulation. It
never launches a hosted job or changes the live profile.

```sh
scripts/orch-nightly.sh
```

For a local schedule, add this command to the user's crontab with the absolute
checkout path, for example `0 2 * * * /path/to/orch/scripts/orch-nightly.sh`.
Set `PATH` for Rust, Node, Java, and TLAPS there. Installing a schedule is a
separate user action. A stale lock must be checked before it is removed.

## Record a week

Use an isolated v2 profile. Export a SQLite snapshot outside `~/.zeron`, then
capture it with an explicit path. Keep trace contents local: args can include
user text. Artifacts stay under ignored `target/`.

```sh
python3 scripts/orch-dogfood.py capture --usage dogfood --db /path/to/snapshot.sqlite
python3 scripts/orch-dogfood.py status
```

For durable week evidence, set `ORCH_DOGFOOD_DIR` to an isolated directory
outside the checkout, and set the same GitHub repository variable for the
local runner. GitHub checkout cleans ignored files, which would erase a week
stored under its `target/`. The default `target/orch-dogfood` is for local
trial captures only. No external directory is created until capture runs.

Use `--usage dogfood` only for actual use. Synthetic captures stay `trial`.
Each capture stores trace and mechanism rows, a hash, a validation log, and UTC capture
metadata. Trace rows keep their clock-free schema. Status requires seven
consecutive dates with new validated sequence activity and at least seven full
days between captures. Repeated unchanged snapshots add no active date.
Status rejects mutable conformance flags until a source-linked full checker exists.
Status therefore blocks release now. Do not set the flag on from these partial
checks. Before release, finish full conformance and required CI,
validate every capture, and complete the week. No week is claimed by this work.

See [the build map](orchestrator-v2/IMPLEMENTATION.md) for current source ownership,
proof limits, runtime scope, and R43 transfer rules.
