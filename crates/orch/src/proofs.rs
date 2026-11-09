use crate::event::{
    DelEvent, DelegateMode, Effect, Event, How, Mode, OutboxEvent, Reject, RunEvent,
};
use crate::ids::ThreadId;
use crate::invariants::{check_invariants, s1, s3, s5a, s6, well_formed};
use crate::state::{
    CohortSt, Config, Delivery, Disp, Hold, Message, Msg, Node, OrchState, Outcome, ProviderEvent,
    Run, RunStatus, Start, Task, TaskSt, Thread, WakeRule,
};
use crate::step::apply_event;
use crate::{delegation, outbox, run};

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

const IDS: [ThreadId; 3] = [R, A, B];

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
    let ok = apply_event(&mut s, e).is_ok();
    kani::cover!(ok);
    if ok {
        assert!(holds(&s));
    }
}

fn holds_after(
    s: &mut OrchState,
    act: impl FnOnce(&mut OrchState, &mut Vec<Effect>) -> Result<(), Reject>,
    holds: fn(&OrchState) -> bool,
) {
    let mut fx = Vec::new();
    let ok = act(s, &mut fx).is_ok();
    kani::cover!(ok);
    if ok {
        assert!(holds(s));
    }
}

fn cases(n: u8, mut f: impl FnMut(u8)) {
    let k: u8 = kani::any();
    kani::assume(k < n);
    if k == 0 {
        f(0);
    } else if k == 1 {
        f(1);
    } else if k == 2 {
        f(2);
    } else {
        f(3);
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

fn kin(id: ThreadId, parent: Option<ThreadId>, depth: u8) -> Thread {
    let mut th = Thread::new(id, parent, depth, level(), false);
    th.runs.push(Run {
        start: Start::Queued(Msg::Task),
        status: RunStatus::Live,
        cohort: Default::default(),
    });
    th.attempt.n = 1;
    th.attempt.emitted = true;
    if parent.is_some() {
        th.task = Some(Task {
            st: pick([TaskSt::Running, TaskSt::Settled]),
            wake: WakeRule::Always,
            delivery: Delivery::Pending,
            cohort: 1,
            run: 1,
            pushes: 0,
        });
    }
    th
}

fn lineage(shape: u8) -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
    });
    s.threads.insert(R, kin(R, None, 0));
    if shape >= 1 {
        s.threads.insert(A, kin(A, Some(R), 1));
    }
    if shape == 2 {
        s.threads.insert(B, kin(B, Some(R), 1));
    }
    if shape == 3 {
        s.threads.insert(B, kin(B, Some(A), 2));
    }
    s.outbox.insert(Message::Notice { child: A, run: 1 });
    s.outbox.insert(Message::Notice { child: B, run: 1 });
    s
}

fn on_lineage(
    act: impl Fn(&mut OrchState, ThreadId, &mut Vec<Effect>) -> Result<(), Reject>,
    holds: fn(&OrchState) -> bool,
) {
    cases(4, |shape| {
        let mut s = lineage(shape);
        kani::assume(holds(&s));
        cases(3, |i| {
            let t = IDS[usize::from(i)];
            holds_after(&mut s, |s, fx| act(s, t, fx), holds);
        })
    });
}

fn delegate_to(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    delegation::kani_delegate(
        s,
        t,
        pick([A, B, ThreadId(3)]),
        pick([DelegateMode::Async, DelegateMode::Wait]),
        level(),
        kani::any(),
        fx,
    )
}

fn narrow_at(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    delegation::kani_narrow_access(s, t, level(), fx)
}

fn notice_of(s: &mut OrchState, c: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    outbox::kani_do_notice(s, c, 1, how(), fx)
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

#[kani::proof]
#[kani::unwind(5)]
fn s6_narrow() {
    on_lineage(narrow_at, s6_holds);
}

#[kani::proof]
#[kani::unwind(5)]
fn s6_delegate() {
    on_lineage(delegate_to, s6_holds);
}

#[kani::proof]
#[kani::unwind(5)]
fn s5a_delegate() {
    on_lineage(delegate_to, s5a_holds);
}

#[kani::proof]
#[kani::unwind(5)]
fn s5a_narrow() {
    on_lineage(narrow_at, s5a_holds);
}

#[kani::proof]
#[kani::unwind(5)]
fn s5a_notice() {
    on_lineage(notice_of, s5a_holds);
}

const HEADS: [Option<Msg>; 4] = [
    None,
    Some(Msg::Person(1)),
    Some(Msg::Server(2)),
    Some(Msg::Wake {
        run: 1,
        generation: 1,
    }),
];

fn any_head() -> Option<Msg> {
    if kani::any() {
        Some(pick([
            Msg::Person(1),
            Msg::Server(2),
            Msg::Wake {
                run: 1,
                generation: 1,
            },
        ]))
    } else {
        None
    }
}

fn lone_thread() -> OrchState {
    lone_runs(u8::try_from(small(2)).unwrap_or(0), any_head())
}

fn lone_runs(n: u8, head: Option<Msg>) -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 1,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
    });
    let mut th = Thread::new(R, None, 0, 1, true);
    for _ in 0..n {
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
    if let Some(m) = head {
        th.queue.push_back(m);
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

fn inbox_event(s: &OrchState, t: ThreadId) -> ProviderEvent {
    s.threads
        .get(&t)
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
#[kani::proof]
#[kani::unwind(4)]
fn s1_ingest_end() {
    cases(3, |n| {
        let mut s = lone_runs(n, any_head());
        assume_good(&s);
        let ev = inbox_event(&s, T);
        holds_after(&mut s, |s, fx| run::kani_ingest_end(s, T, ev, fx), s1_holds);
    });
}
harness!(
    s1_retry_attempt,
    4,
    lone_thread(),
    |s: &OrchState| Event::Run(RunEvent::RetryAttempt {
        t: T,
        ev: inbox_event(s, T)
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
#[kani::proof]
#[kani::unwind(4)]
fn s1_start_from_queue_wake() {
    cases(3, |n| {
        cases(4, |q| {
            let mut s = lone_runs(n, HEADS[usize::from(q)]);
            kani::assume(s1_holds(&s));
            holds_after(
                &mut s,
                |s, fx| delegation::kani_start_from_queue_wake(s, T, fx),
                s1_holds,
            );
        })
    });
}

const WAKES: [Option<Msg>; 3] = [
    None,
    Some(Msg::Wake {
        run: 1,
        generation: 0,
    }),
    Some(Msg::Wake {
        run: 1,
        generation: 1,
    }),
];

fn any_wake() -> Option<Msg> {
    if kani::any() {
        Some(Msg::Wake {
            run: 1,
            generation: small(1),
        })
    } else {
        None
    }
}

const P: ThreadId = ThreadId(1);
const K: ThreadId = ThreadId(0);

fn delivery_state(wake: Option<Msg>) -> OrchState {
    let mut s = OrchState::new(Config {
        max_depth: 2,
        max_fanout: 2,
        max_queue: 2,
        max_retry: 1,
        run_window: 8,
    });
    let mut root = Thread::new(P, None, 0, 1, true);
    one_run(&mut root, Start::Queued(Msg::Person(1)));
    if let Some(co) = root.cohort_mut(1) {
        co.disp = pick([Disp::Open, Disp::Stopped]);
        co.st = pick([CohortSt::None, CohortSt::Offered, CohortSt::Queued]);
        co.generation = small(1);
        if kani::any() {
            co.del.insert(K);
        }
    }
    if let Some(m) = wake {
        root.queue.push_back(m);
    }
    root.held = pick([Hold::None, Hold::Fail]);
    root.wait_on = if kani::any() { Some(K) } else { None };
    root.late_wait = if kani::any() { Some(K) } else { None };
    s.threads.insert(P, root);
    s.threads.insert(K, child(K, P, 1));
    if kani::any() {
        s.outbox.insert(pick([
            Message::Notice { child: K, run: 1 },
            Message::Offer {
                parent: P,
                run: 1,
                generation: small(1),
            },
            Message::Stop { child: K, stop: 1 },
        ]));
    }
    s
}

fn delivery_with_end() -> OrchState {
    delivery_end(any_wake())
}

fn delivery_end(wake: Option<Msg>) -> OrchState {
    let mut s = delivery_state(wake);
    if kani::any() {
        if let Some(root) = s.threads.get_mut(&P) {
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
    del(DelEvent::WaitReturn { t: P }),
    s3_holds
);
harness!(
    s3_wait_timeout,
    5,
    delivery_with_end(),
    del(DelEvent::WaitTimeout { t: P }),
    s3_holds
);
harness!(
    s3_late_wait_ack,
    5,
    delivery_with_end(),
    del(DelEvent::LateWaitAck { t: P }),
    s3_holds
);
fn on_delivery(act: impl FnOnce(&mut OrchState, &mut Vec<Effect>) -> Result<(), Reject>) {
    let mut s = delivery_with_end();
    kani::assume(s3_holds(&s));
    holds_after(&mut s, act, s3_holds);
}

#[kani::proof]
#[kani::unwind(5)]
fn s3_stop() {
    on_delivery(|s, fx| delegation::kani_stop(s, P, fx));
}

#[kani::proof]
#[kani::unwind(5)]
fn s3_start_from_queue_wake() {
    let mut s = delivery_end(None);
    kani::assume(s3_holds(&s));
    cases(3, |w| {
        if let (Some(m), Ok(th)) = (WAKES[usize::from(w)], s.thread_mut(P)) {
            th.queue.push_back(m);
        }
        holds_after(
            &mut s,
            |s, fx| delegation::kani_start_from_queue_wake(s, P, fx),
            s3_holds,
        );
    });
}
harness!(
    s3_read_result,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Del(DelEvent::ReadResult {
        p: P,
        c: K,
        run: small(1)
    }),
    s3_holds
);
#[kani::proof]
#[kani::unwind(5)]
fn s3_do_notice() {
    on_delivery(|s, fx| outbox::kani_do_notice(s, K, 1, how(), fx));
}
harness!(
    s3_do_offer,
    5,
    delivery_with_end(),
    |_: &OrchState| Event::Outbox(OutboxEvent::DoOffer {
        parent: P,
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
        child: K,
        stop: 1,
        how: how()
    }),
    s3_holds
);
#[kani::proof]
#[kani::unwind(5)]
fn s3_ingest_end() {
    let mut s = delivery_with_end();
    kani::assume(s3_holds(&s));
    let ev = inbox_event(&s, P);
    holds_after(&mut s, |s, fx| run::kani_ingest_end(s, P, ev, fx), s3_holds);
}
