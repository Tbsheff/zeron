use std::path::Path;
use std::sync::Arc;

use zeron_orch::{
    Config, DelEvent, DelegateMode, Event, How, Message, Mode, Node, OrchState, OutboxEvent,
    Outcome, ProviderEvent, RunEvent, ThreadId, check_invariants, well_formed,
};
use zeron_sync::DocsStore;

use super::trace::{self, CheckReport, parse_lines};
use super::{Applied, Driver, DriverError, FaultPoint, RecordingSink};

pub(super) const R: ThreadId = ThreadId(0);
pub(super) const A: ThreadId = ThreadId(1);
pub(super) const DEV: &str = "dev-a";

pub(super) fn config() -> Config {
    Config {
        run_window: 3,
        ..Config::default()
    }
}

fn ended(run: u32, outcome: Outcome) -> ProviderEvent {
    ProviderEvent {
        node: Node::Root,
        run,
        attempt: 1,
        outcome,
    }
}

fn root_turn(t: ThreadId, run: u32, rows: &mut Vec<(String, Event)>) {
    rows.push((
        format!("term:{}:{run}", t.0),
        Event::Run(RunEvent::ProviderTerminal {
            t,
            run,
            attempt: 1,
            outcome: Outcome::Completed,
        }),
    ));
    rows.push((
        format!("end:{}:{run}", t.0),
        Event::Run(RunEvent::IngestEnd {
            t,
            ev: ended(run, Outcome::Completed),
        }),
    ));
}

pub(super) fn script() -> Vec<(String, Event)> {
    let mut rows = vec![
        (
            "open:0".into(),
            Event::Del(DelEvent::OpenRoot {
                t: R,
                access: 2,
                reconfigure: true,
            }),
        ),
        (
            "person:0:1".into(),
            Event::Run(RunEvent::PersonSend {
                t: R,
                mode: Mode::Queue,
                msg: 1,
            }),
        ),
        (
            "start:0:1".into(),
            Event::Run(RunEvent::StartFromQueue { t: R }),
        ),
        (
            "start:0:again".into(),
            Event::Run(RunEvent::StartFromQueue { t: R }),
        ),
        (
            "delegate:0:1".into(),
            Event::Del(DelEvent::Delegate {
                t: R,
                child: A,
                mode: DelegateMode::Async,
                access: 1,
                reconfigure: true,
            }),
        ),
        (
            "task:1".into(),
            Event::Run(RunEvent::StartFromQueue { t: A }),
        ),
    ];
    root_turn(A, 1, &mut rows);
    rows.push((
        "notice:1:1".into(),
        Event::Outbox(OutboxEvent::DoNotice {
            child: A,
            run: 1,
            how: How::Ack,
        }),
    ));
    root_turn(R, 1, &mut rows);
    rows.push((
        "offer:0:1:1".into(),
        Event::Outbox(OutboxEvent::DoOffer {
            parent: R,
            run: 1,
            generation: 1,
            how: How::Ack,
        }),
    ));
    rows.push((
        "wake:0:2".into(),
        Event::Del(DelEvent::StartFromQueueWake { t: R }),
    ));
    rows.push(("stop:0".into(), Event::Del(DelEvent::Stop { t: R })));
    rows.push((
        "interrupted:0:2".into(),
        Event::Run(RunEvent::ProviderTerminal {
            t: R,
            run: 2,
            attempt: 1,
            outcome: Outcome::Interrupted,
        }),
    ));
    rows.push((
        "end:0:2".into(),
        Event::Run(RunEvent::IngestEnd {
            t: R,
            ev: ended(2, Outcome::Interrupted),
        }),
    ));
    rows.push((
        "dostop:1:1".into(),
        Event::Outbox(OutboxEvent::DoStop {
            child: A,
            stop: 1,
            how: How::Ack,
        }),
    ));
    rows
}

pub(super) fn open(dir: &Path) -> Driver {
    let store = Arc::new(DocsStore::open(dir).expect("open docs store"));
    Driver::open(store, DEV, config()).expect("open driver")
}

fn drive(
    d: &Driver,
    rows: &[(String, Event)],
    sink: &mut RecordingSink,
) -> Result<(), DriverError> {
    d.run_outbox(sink)?;
    for row in rows {
        d.drain(std::slice::from_ref(row))?;
        d.run_outbox(sink)?;
    }
    Ok(())
}

fn assert_good(s: &OrchState) {
    well_formed(s).expect("well formed after restart");
    check_invariants(s).expect("invariants after restart");
}

pub(super) struct Run {
    pub state: OrchState,
    pub sink: RecordingSink,
    pub report: CheckReport,
    pub restarts: usize,
    pub lines: Vec<String>,
}

pub(super) fn run_with(fault: Option<(FaultPoint, usize)>) -> Run {
    let dir = tempfile::tempdir().expect("tempdir");
    let rows = script();
    let mut sink = RecordingSink::default();
    let mut armed = fault;
    let mut restarts = 0;
    loop {
        let d = open(dir.path());
        assert_good(&d.state().expect("state"));
        if let Some((at, skip)) = armed.take() {
            d.crash_at(at, skip);
        }
        match drive(&d, &rows, &mut sink) {
            Ok(()) => {
                let lines = d.trace_lines().expect("trace");
                let parsed = parse_lines(&lines.join("\n")).expect("parse trace");
                let report = trace::check(&parsed).unwrap_or_else(|e| panic!("trace check: {e}"));
                assert!(d.pending().expect("pending").is_empty());
                return Run {
                    state: d.state().expect("state"),
                    sink,
                    report,
                    restarts,
                    lines,
                };
            }
            Err(DriverError::Crashed(at)) => {
                assert_eq!(Some(at), fault.map(|f| f.0));
                restarts += 1;
                assert!(restarts < 3, "crashed twice");
            }
            Err(e) => panic!("driver error: {e}"),
        }
    }
}

fn hits(reference: &Run, at: FaultPoint) -> usize {
    let applied = reference.report.accepted + reference.report.rejected;
    match at {
        FaultPoint::Fp2EffectRunNotMarked => reference.sink.applied.len(),
        _ => applied,
    }
}

fn assert_same_outcome(got: &Run, want: &Run, at: FaultPoint, skip: usize) {
    let ctx = format!("{at:?} skip {skip}");
    assert_eq!(got.restarts, 1, "{ctx}: one crash then recovery");
    assert_eq!(
        trace::state_hash(&got.state).ok(),
        trace::state_hash(&want.state).ok(),
        "{ctx}: final state"
    );
    assert_eq!(
        got.sink.applied, want.sink.applied,
        "{ctx}: effects exactly once by id"
    );
    assert_eq!(
        got.report.accepted, want.report.accepted,
        "{ctx}: accepted steps"
    );
    assert_eq!(
        got.report.rejected, want.report.rejected,
        "{ctx}: rejected steps"
    );
}

#[test]
fn reference_script_exercises_the_driver() {
    let r = run_with(None);
    assert_eq!(r.restarts, 0);
    assert_eq!(r.report.rejected, 1);
    assert_eq!(r.report.duplicates, 0);
    assert_eq!(r.report.accepted, script().len() - 1);
    assert_eq!(r.sink.redeliveries(), 0);
    assert!(
        r.sink.applied.len() >= 8,
        "effects: {:?}",
        r.sink.applied.keys()
    );
    assert!(
        r.sink
            .applied
            .values()
            .any(|e| matches!(e, zeron_orch::Effect::Send(Message::Stop { .. })))
    );
    assert_eq!(r.state.threads[&R].rord(), 2);
}

#[test]
fn fp1_step_committed_effect_not_run() {
    let want = run_with(None);
    let at = FaultPoint::Fp1StepCommittedEffectNotRun;
    for skip in 0..hits(&want, at) {
        let got = run_with(Some((at, skip)));
        assert_same_outcome(&got, &want, at, skip);
        assert_eq!(got.sink.redeliveries(), 0, "{at:?} {skip}");
        assert_eq!(
            got.report.duplicates, 1,
            "{at:?} {skip}: ledger re-drains the row"
        );
    }
}

#[test]
fn fp2_effect_run_outbox_not_marked() {
    let want = run_with(None);
    let at = FaultPoint::Fp2EffectRunNotMarked;
    for skip in 0..hits(&want, at) {
        let got = run_with(Some((at, skip)));
        assert_same_outcome(&got, &want, at, skip);
        assert_eq!(
            got.sink.redeliveries(),
            1,
            "{at:?} {skip}: replayed once, applied once"
        );
        assert_eq!(
            got.report.duplicates, 0,
            "{at:?} {skip}: the ledger row was marked"
        );
    }
}

#[test]
fn fp3_entry_received_no_receipt_rolls_back() {
    let want = run_with(None);
    let at = FaultPoint::Fp3ReceivedNoReceipt;
    for skip in 0..hits(&want, at) {
        let got = run_with(Some((at, skip)));
        assert_same_outcome(&got, &want, at, skip);
        assert_eq!(got.sink.redeliveries(), 0, "{at:?} {skip}");
        assert_eq!(
            got.report.duplicates, 0,
            "{at:?} {skip}: the step left no trace row"
        );
        assert_eq!(
            got.lines, want.lines,
            "{at:?} {skip}: same trace as no crash"
        );
    }
}

#[test]
fn fp4_receipt_committed_ledger_not_marked() {
    let want = run_with(None);
    let at = FaultPoint::Fp4ReceiptNotMarked;
    for skip in 0..hits(&want, at) {
        let got = run_with(Some((at, skip)));
        assert_same_outcome(&got, &want, at, skip);
        assert_eq!(got.sink.redeliveries(), 0, "{at:?} {skip}");
        assert_eq!(
            got.report.duplicates, 1,
            "{at:?} {skip}: re-drain is a stutter"
        );
    }
}

#[test]
fn s10a_duplicate_returns_stored_result_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = open(dir.path());
    let rows = script();
    for (key, ev) in &rows[..3] {
        d.apply(key, *ev).expect("apply");
    }
    let before = d.state().expect("state");
    let outbox = d.outbox().expect("outbox");
    let (key, ev) = &rows[2];
    let again = d.apply(key, *ev).expect("duplicate");
    assert!(again.duplicate);
    assert!(matches!(&again.applied, Applied::Accepted { effects, .. } if !effects.is_empty()));
    let other = Event::Run(RunEvent::ServerEnqueue { t: R, msg: 9 });
    let dup_other = d.apply(key, other).expect("same key, other event");
    assert_eq!(dup_other.applied, again.applied);
    assert_eq!(d.state().expect("state"), before);
    assert_eq!(d.outbox().expect("outbox"), outbox);
}

#[test]
fn rejected_event_gets_a_receipt_and_no_state_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = open(dir.path());
    let ev = Event::Run(RunEvent::StartFromQueue { t: R });
    let r = d.apply("bad", ev).expect("apply");
    assert!(matches!(
        r.applied,
        Applied::Rejected {
            reason: zeron_orch::Reject::UnknownThread,
            ..
        }
    ));
    assert_eq!(d.state().expect("state"), OrchState::new(config()));
    assert!(d.apply("bad", ev).expect("again").duplicate);
}

#[test]
fn update_rollback_restores_sqlite_and_redrain_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let saved = tempfile::tempdir().expect("saved");
    let rows = script();
    let mut sink = RecordingSink::default();
    let half = rows.len() / 2;
    {
        let d = open(dir.path());
        drive(&d, &rows[..half], &mut sink).expect("first half");
    }
    for entry in std::fs::read_dir(dir.path()).expect("ls") {
        let p = entry.expect("entry").path();
        std::fs::copy(&p, saved.path().join(p.file_name().expect("name"))).expect("copy");
    }
    {
        let d = open(dir.path());
        drive(&d, &rows, &mut sink).expect("whole script");
    }
    for entry in std::fs::read_dir(dir.path()).expect("ls") {
        std::fs::remove_file(entry.expect("entry").path()).expect("clear");
    }
    for entry in std::fs::read_dir(saved.path()).expect("ls") {
        let p = entry.expect("entry").path();
        std::fs::copy(&p, dir.path().join(p.file_name().expect("name"))).expect("restore");
    }
    let d = open(dir.path());
    assert!(d.state().expect("state").threads[&R].rord() < 2);
    drive(&d, &rows, &mut sink).expect("re-drain after rollback");
    let want = run_with(None);
    assert_eq!(d.state().expect("state"), want.state);
    assert_eq!(sink.applied, want.sink.applied);
    assert!(sink.redeliveries() > 0);
    let lines = parse_lines(&d.trace_lines().expect("trace").join("\n")).expect("parse");
    trace::check(&lines).expect("trace after rollback");
}
