use crate::event::{DelEvent, DelegateMode, Event, How, Mode, OutboxEvent, RunEvent};
use crate::ids::ThreadId;
use crate::invariants::{Violation, all_violations, check_invariants, well_formed};
use crate::state::{
    CohortSt, Config, Delivery, Disp, Hold, Message, Msg, Node, OrchState, Outcome, ProviderEvent,
    Run, RunStatus, Start, Task, TaskSt, Thread, WakeRule,
};
use crate::step::step;

const R: ThreadId = ThreadId(0);
const A: ThreadId = ThreadId(1);
const B: ThreadId = ThreadId(2);

fn small(max: u32) -> u32 {
    let v: u32 = kani::any();
    kani::assume(v <= max);
    v
}

fn level() -> u8 {
    let v: u8 = kani::any();
    kani::assume(v <= 2);
    v
}

fn pick<T: Copy, const K: usize>(items: [T; K]) -> T {
    let i: usize = kani::any();
    kani::assume(i < K);
    items[i]
}

fn tid() -> ThreadId {
    pick([R, A, B])
}

fn outcome() -> Outcome {
    pick([
        Outcome::Completed,
        Outcome::Failed,
        Outcome::Interrupted,
        Outcome::UsageLimit,
        Outcome::Transient,
        Outcome::Crash,
    ])
}

fn status() -> RunStatus {
    pick([
        RunStatus::Live,
        RunStatus::Completed,
        RunStatus::Failed,
        RunStatus::Interrupted,
        RunStatus::UsageLimit,
    ])
}

fn how() -> How {
    pick([How::Ack, How::Keep])
}

fn one_run(th: &mut Thread, start: Start) {
    th.runs.push(Run {
        start,
        status: status(),
        cohort: Default::default(),
    });
    th.attempt.n = 1;
}

fn task(cohort: u32) -> Task {
    Task {
        st: pick([TaskSt::Running, TaskSt::Settled]),
        wake: pick([WakeRule::Always, WakeRule::SettledOnly]),
        delivery: pick([
            Delivery::Pending,
            Delivery::Claimed,
            Delivery::Delivered,
            Delivery::Acknowledged,
            Delivery::Disposed,
        ]),
        cohort,
        run: 1,
        pushes: pick([0, 1, 2]),
    }
}

fn child(id: ThreadId, parent: ThreadId, depth: u8) -> Thread {
    let mut c = Thread::new(id, Some(parent), depth, level(), kani::any());
    c.task = Some(task(1));
    if kani::any() {
        one_run(&mut c, Start::Queued(Msg::Task));
    }
    c
}

fn assume_good(s: &OrchState) {
    kani::assume(well_formed(s).is_ok());
    kani::assume(check_invariants(s).is_ok());
}

fn preserved(s: &OrchState, e: Event, broken: fn(&Violation) -> bool) {
    if let Ok((next, _)) = step(s, e) {
        assert!(!all_violations(&next).iter().any(broken));
    }
}

fn lineage() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
    });
    let mut root = Thread::new(R, None, 0, level(), kani::any());
    one_run(&mut root, Start::Queued(Msg::Person(1)));
    s.threads.insert(R, root);
    if kani::any() {
        s.threads.insert(A, child(A, R, 1));
        if kani::any() {
            let deep: bool = kani::any();
            let (p, d) = if deep { (A, 2) } else { (R, 1) };
            s.threads.insert(B, child(B, p, d));
        }
    }
    s
}

fn lineage_event() -> Event {
    let mode = pick([DelegateMode::Async, DelegateMode::Wait]);
    let child = pick([A, B, ThreadId(3)]);
    Event::Del(pick([
        DelEvent::NarrowAccess {
            t: tid(),
            access: level(),
        },
        DelEvent::Delegate {
            t: tid(),
            child,
            mode,
            access: level(),
            reconfigure: kani::any(),
        },
    ]))
}

#[kani::proof]
#[kani::unwind(5)]
fn s6_access_narrow_under_narrow_and_delegate() {
    let s = lineage();
    assume_good(&s);
    preserved(&s, lineage_event(), |v| {
        matches!(v, Violation::S6AccessNarrow(_))
    });
}

#[kani::proof]
#[kani::unwind(5)]
fn s5a_depth_and_fanout_under_delegate_and_settle() {
    let mut s = lineage();
    if kani::any() {
        s.outbox.insert(Message::Notice {
            child: pick([A, B]),
            run: 1,
        });
    }
    assume_good(&s);
    let e = if kani::any() {
        lineage_event()
    } else {
        Event::Outbox(OutboxEvent::DoNotice {
            child: pick([A, B]),
            run: 1,
            how: how(),
        })
    };
    preserved(&s, e, |v| {
        matches!(v, Violation::S5aDepth(_) | Violation::S5aFanout(_))
    });
}

fn lone_thread() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
    });
    let mut th = Thread::new(R, None, 0, 1, true);
    for _ in 0..small(2) {
        th.runs.push(Run {
            start: Start::Queued(Msg::Person(1)),
            status: status(),
            cohort: Default::default(),
        });
    }
    th.attempt.n = small(2);
    th.attempt.emitted = kani::any();
    th.attempt.retries = small(1);
    th.held = pick([Hold::None, Hold::Stop, Hold::Fail, Hold::Limit]);
    th.int_req = kani::any();
    th.ul.armed = if kani::any() { Some(small(2)) } else { None };
    th.ul.due = kani::any();
    if kani::any() {
        th.queue.push_back(pick([
            Msg::Person(1),
            Msg::Server(2),
            Msg::Wake {
                run: 1,
                generation: 1,
            },
        ]));
    }
    if let Some(co) = th.cohort_mut(1) {
        co.st = pick([CohortSt::None, CohortSt::Queued]);
        co.generation = small(1);
    }
    if kani::any() {
        th.inbox.insert(ProviderEvent {
            node: Node::Root,
            run: small(2),
            attempt: small(2),
            outcome: outcome(),
        });
    }
    s.threads.insert(R, th);
    s
}

fn lone_event(s: &OrchState) -> Event {
    let t = R;
    let ev = s
        .threads
        .get(&R)
        .and_then(|th| th.inbox.iter().next().copied());
    let ev = ev.unwrap_or(ProviderEvent {
        node: Node::Root,
        run: 1,
        attempt: 1,
        outcome: Outcome::Completed,
    });
    match small(2) {
        0 => Event::Run(pick([
            RunEvent::StartFromQueue { t },
            RunEvent::UsageLimitRecovery { t },
            RunEvent::IngestEnd { t, ev },
            RunEvent::RetryAttempt { t, ev },
        ])),
        1 => Event::Run(pick([
            RunEvent::RestartActive {
                t,
                mode: Mode::Restart,
                msg: 1,
            },
            RunEvent::ProviderTerminal {
                t,
                run: small(2),
                attempt: small(2),
                outcome: outcome(),
            },
            RunEvent::RequestInterrupt { t },
            RunEvent::ResetPasses { t, run: small(2) },
        ])),
        _ => Event::Del(DelEvent::StartFromQueueWake { t }),
    }
}

#[kani::proof]
#[kani::unwind(5)]
fn s1_at_most_one_active_run() {
    let s = lone_thread();
    assume_good(&s);
    let e = lone_event(&s);
    preserved(&s, e, |v| matches!(v, Violation::S1AtMostOneActiveRun(_)));
}

fn delivery_state() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 2,
        max_queue: 2,
        max_retry: 1,
    });
    let mut root = Thread::new(R, None, 0, 1, true);
    one_run(&mut root, Start::Queued(Msg::Person(1)));
    if let Some(co) = root.cohort_mut(1) {
        co.disp = pick([Disp::Open, Disp::Stopped]);
        co.st = pick([CohortSt::None, CohortSt::Offered, CohortSt::Queued]);
        co.generation = small(1);
        if kani::any() {
            co.del.insert(A);
        }
    }
    if kani::any() {
        root.queue.push_back(Msg::Wake {
            run: 1,
            generation: small(1),
        });
    }
    root.held = pick([Hold::None, Hold::Fail]);
    root.wait_on = if kani::any() { Some(A) } else { None };
    root.late_wait = if kani::any() { Some(A) } else { None };
    s.threads.insert(R, root);
    s.threads.insert(A, child(A, R, 1));
    if kani::any() {
        s.outbox.insert(pick([
            Message::Notice { child: A, run: 1 },
            Message::Offer {
                parent: R,
                run: 1,
                generation: small(1),
            },
            Message::Stop { child: A, stop: 1 },
        ]));
    }
    s
}

fn delivery_event(ev: ProviderEvent) -> Event {
    match small(2) {
        0 => Event::Del(pick([
            DelEvent::ReadResult {
                p: R,
                c: A,
                run: small(1),
            },
            DelEvent::WaitReturn { t: R },
            DelEvent::WaitTimeout { t: R },
            DelEvent::LateWaitAck { t: R },
            DelEvent::StartFromQueueWake { t: R },
            DelEvent::Stop { t: R },
        ])),
        1 => Event::Outbox(pick([
            OutboxEvent::DoNotice {
                child: A,
                run: 1,
                how: how(),
            },
            OutboxEvent::DoOffer {
                parent: R,
                run: 1,
                generation: small(1),
                how: how(),
            },
            OutboxEvent::DoStop {
                child: A,
                stop: 1,
                how: how(),
            },
        ])),
        _ => Event::Run(RunEvent::IngestEnd { t: R, ev }),
    }
}

#[kani::proof]
#[kani::unwind(5)]
fn s3_exactly_once() {
    let mut s = delivery_state();
    let ev = ProviderEvent {
        node: Node::Root,
        run: 1,
        attempt: 1,
        outcome: pick([Outcome::Completed, Outcome::Interrupted]),
    };
    if kani::any() {
        if let Some(root) = s.threads.get_mut(&R) {
            root.inbox.insert(ev);
            root.attempt.emitted = true;
        }
    }
    assume_good(&s);
    let e = delivery_event(ev);
    preserved(&s, e, |v| matches!(v, Violation::S3ExactlyOnce(_)));
}
