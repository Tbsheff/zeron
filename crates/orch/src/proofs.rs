use crate::event::{DelEvent, DelegateMode, Event, How, Mode, OutboxEvent, RunEvent};
use crate::ids::ThreadId;
use crate::invariants::{check_invariants, s1, s3, s5a, s6, well_formed};
use crate::state::{
    CohortSt, Config, Delivery, Disp, Hold, Message, Msg, Node, OrchState, Outcome, ProviderEvent,
    Run, RunStatus, Start, Task, TaskSt, Thread, WakeRule,
};
use crate::step::apply_event;

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

fn preserved(mut s: OrchState, e: Event, holds: fn(&OrchState) -> bool) {
    if apply_event(&mut s, e).is_ok() {
        assert!(holds(&s));
    }
}

fn s1_holds(s: &OrchState) -> bool {
    s.threads.values().all(|th| s1(th).is_ok())
}

fn s3_holds(s: &OrchState) -> bool {
    s.threads
        .values()
        .all(|th| th.task.is_none_or(|k| s3(th.id, k).is_ok()))
}

fn s5a_holds(s: &OrchState) -> bool {
    s.threads.values().all(|th| s5a(s, th).is_ok())
}

fn s6_holds(s: &OrchState) -> bool {
    s.threads.values().all(|th| s6(s, th).is_ok())
}

fn lineage() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
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

fn delegate_event() -> Event {
    Event::Del(DelEvent::Delegate {
        t: tid(),
        child: pick([A, B, ThreadId(3)]),
        mode: pick([DelegateMode::Async, DelegateMode::Wait]),
        access: level(),
        reconfigure: kani::any(),
    })
}

fn narrow_event() -> Event {
    Event::Del(DelEvent::NarrowAccess {
        t: tid(),
        access: level(),
    })
}

fn notice_event() -> Event {
    Event::Outbox(OutboxEvent::DoNotice {
        child: pick([A, B]),
        run: 1,
        how: how(),
    })
}

fn with_notice() -> OrchState {
    let mut s = lineage();
    if kani::any() {
        s.outbox.insert(Message::Notice {
            child: pick([A, B]),
            run: 1,
        });
    }
    s
}

macro_rules! harness {
    ($name:ident, $unwind:literal, $state:expr, $event:expr, $holds:expr) => {
        #[kani::proof]
        #[kani::unwind($unwind)]
        fn $name() {
            let s = $state;
            assume_good(&s);
            let pick_event = $event;
            let e = pick_event(&s);
            preserved(s, e, $holds);
        }
    };
}

harness!(
    s6_narrow,
    5,
    lineage(),
    |_: &OrchState| narrow_event(),
    s6_holds
);
harness!(
    s6_delegate,
    5,
    lineage(),
    |_: &OrchState| delegate_event(),
    s6_holds
);
harness!(
    s5a_delegate,
    5,
    with_notice(),
    |_: &OrchState| delegate_event(),
    s5a_holds
);
harness!(
    s5a_narrow,
    5,
    with_notice(),
    |_: &OrchState| narrow_event(),
    s5a_holds
);
harness!(
    s5a_notice,
    5,
    with_notice(),
    |_: &OrchState| notice_event(),
    s5a_holds
);

fn lone_thread() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
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

fn inbox_event(s: &OrchState) -> ProviderEvent {
    s.threads
        .get(&R)
        .and_then(|th| th.inbox.first().copied())
        .unwrap_or(ProviderEvent {
            node: Node::Root,
            run: 1,
            attempt: 1,
            outcome: Outcome::Completed,
        })
}

fn lone_run(e: RunEvent) -> impl Fn(&OrchState) -> Event {
    move |_| Event::Run(e)
}

const T: ThreadId = R;

harness!(
    s1_start_from_queue,
    4,
    lone_thread(),
    lone_run(RunEvent::StartFromQueue { t: T }),
    s1_holds
);
harness!(
    s1_usage_limit_recovery,
    4,
    lone_thread(),
    lone_run(RunEvent::UsageLimitRecovery { t: T }),
    s1_holds
);
harness!(
    s1_request_interrupt,
    4,
    lone_thread(),
    lone_run(RunEvent::RequestInterrupt { t: T }),
    s1_holds
);
harness!(
    s1_restart_active,
    4,
    lone_thread(),
    lone_run(RunEvent::RestartActive {
        t: T,
        mode: Mode::Restart,
        msg: 1
    }),
    s1_holds
);
harness!(
    s1_ingest_end,
    4,
    lone_thread(),
    |s: &OrchState| Event::Run(RunEvent::IngestEnd {
        t: T,
        ev: inbox_event(s)
    }),
    s1_holds
);
harness!(
    s1_retry_attempt,
    4,
    lone_thread(),
    |s: &OrchState| Event::Run(RunEvent::RetryAttempt {
        t: T,
        ev: inbox_event(s)
    }),
    s1_holds
);
harness!(
    s1_provider_terminal,
    4,
    lone_thread(),
    |_: &OrchState| Event::Run(RunEvent::ProviderTerminal {
        t: T,
        run: small(2),
        attempt: small(2),
        outcome: outcome(),
    }),
    s1_holds
);
harness!(
    s1_reset_passes,
    4,
    lone_thread(),
    |_: &OrchState| Event::Run(RunEvent::ResetPasses {
        t: T,
        run: small(2)
    }),
    s1_holds
);
harness!(
    s1_start_from_queue_wake,
    4,
    lone_thread(),
    |_: &OrchState| Event::Del(DelEvent::StartFromQueueWake { t: T }),
    s1_holds
);

fn delivery_state() -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 2,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
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

fn delivery_with_end() -> OrchState {
    let mut s = delivery_state();
    if kani::any() {
        if let Some(root) = s.threads.get_mut(&R) {
            root.inbox.insert(ProviderEvent {
                node: Node::Root,
                run: 1,
                attempt: 1,
                outcome: pick([Outcome::Completed, Outcome::Interrupted]),
            });
            root.attempt.emitted = true;
        }
    }
    s
}

fn del(e: DelEvent) -> impl Fn(&OrchState) -> Event {
    move |_| Event::Del(e)
}

harness!(
    s3_wait_return,
    5,
    delivery_with_end(),
    del(DelEvent::WaitReturn { t: R }),
    s3_holds
);
harness!(
    s3_wait_timeout,
    5,
    delivery_with_end(),
    del(DelEvent::WaitTimeout { t: R }),
    s3_holds
);
harness!(
    s3_late_wait_ack,
    5,
    delivery_with_end(),
    del(DelEvent::LateWaitAck { t: R }),
    s3_holds
);
harness!(
    s3_stop,
    5,
    delivery_with_end(),
    del(DelEvent::Stop { t: R }),
    s3_holds
);
harness!(
    s3_start_from_queue_wake,
    5,
    delivery_with_end(),
    del(DelEvent::StartFromQueueWake { t: R }),
    s3_holds
);
harness!(
    s3_read_result,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Del(DelEvent::ReadResult {
        p: R,
        c: A,
        run: small(1)
    }),
    s3_holds
);
harness!(
    s3_do_notice,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Outbox(OutboxEvent::DoNotice {
        child: A,
        run: 1,
        how: how()
    }),
    s3_holds
);
harness!(
    s3_do_offer,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Outbox(OutboxEvent::DoOffer {
        parent: R,
        run: 1,
        generation: small(1),
        how: how(),
    }),
    s3_holds
);
harness!(
    s3_do_stop,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Outbox(OutboxEvent::DoStop {
        child: A,
        stop: 1,
        how: how()
    }),
    s3_holds
);
harness!(
    s3_ingest_end,
    5,
    delivery_with_end(),
    |s: &OrchState| Event::Run(RunEvent::IngestEnd {
        t: R,
        ev: inbox_event(s)
    }),
    s3_holds
);
