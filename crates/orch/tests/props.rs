use proptest::prelude::*;
use zeron_orch::{
    Config, DelEvent, DelegateMode, Event, How, Message, Mode, Node, OrchState, OutboxEvent,
    Outcome, ProviderEvent, RunEvent, Thread, ThreadId, check_invariants, step, well_formed,
};

const OUTCOMES: [Outcome; 6] = [
    Outcome::Completed,
    Outcome::Failed,
    Outcome::Interrupted,
    Outcome::UsageLimit,
    Outcome::Transient,
    Outcome::Crash,
];
const MODES: [Mode; 4] = [Mode::Steer, Mode::Restart, Mode::Queue, Mode::Now];
const MAX_IDS: u32 = 5;

fn init() -> OrchState {
    let cfg = Config {
        max_depth: 2,
        max_fanout: 2,
        max_queue: 3,
        max_retry: 1,
        run_window: 2,
    };
    let open = Event::Del(DelEvent::OpenRoot {
        t: ThreadId(0),
        access: 2,
        reconfigure: true,
    });
    step(&OrchState::new(cfg), open).expect("open root").0
}

fn thread_events(th: &Thread, out: &mut Vec<Event>) {
    let t = th.id;
    for mode in MODES {
        out.push(Event::Run(RunEvent::PersonSend { t, mode, msg: 1 }));
        out.push(Event::Run(RunEvent::RestartActive { t, mode, msg: 2 }));
    }
    for e in [
        RunEvent::Steer { t, msg: 3 },
        RunEvent::RequestInterrupt { t },
        RunEvent::ServerEnqueue { t, msg: 4 },
        RunEvent::StartFromQueue { t },
        RunEvent::UsageLimitRecovery { t },
        RunEvent::ChildStart { t },
        RunEvent::ChildEnd { t },
        RunEvent::ResetPasses {
            t,
            run: th.ul.armed.unwrap_or(th.rord()),
        },
    ] {
        out.push(Event::Run(e));
    }
    for outcome in OUTCOMES {
        out.push(Event::Run(RunEvent::ProviderTerminal {
            t,
            run: th.rord(),
            attempt: th.attempt.n,
            outcome,
        }));
    }
    for ev in &th.inbox {
        ingest_events(t, *ev, out);
    }
}

fn ingest_events(t: ThreadId, ev: ProviderEvent, out: &mut Vec<Event>) {
    for e in [
        RunEvent::IngestEnd { t, ev },
        RunEvent::RetryAttempt { t, ev },
        RunEvent::IngestChild { t, ev },
        RunEvent::IngestStale { t, ev },
    ] {
        out.push(Event::Run(e));
    }
}

fn del_events(s: &OrchState, th: &Thread, out: &mut Vec<Event>) {
    let t = th.id;
    for e in [
        DelEvent::Stop { t },
        DelEvent::NarrowAccess {
            t,
            access: th.access.saturating_sub(1),
        },
        DelEvent::InterruptChat { t },
        DelEvent::WaitReturn { t },
        DelEvent::WaitTimeout { t },
        DelEvent::WaitAbandon { t },
        DelEvent::LateWaitAck { t },
        DelEvent::StartFromQueueWake { t },
    ] {
        out.push(Event::Del(e));
    }
    let child = (0..MAX_IDS)
        .map(ThreadId)
        .find(|c| !s.threads.contains_key(c))
        .unwrap_or(ThreadId(MAX_IDS));
    for (mode, access, reconfigure) in [
        (DelegateMode::Async, 1, true),
        (DelegateMode::Wait, 2, false),
        (DelegateMode::Async, 3, true),
    ] {
        out.push(Event::Del(DelEvent::Delegate {
            t,
            child,
            mode,
            access,
            reconfigure,
        }));
    }
    for c in s.kids(t) {
        out.push(Event::Del(DelEvent::ReadResult {
            p: t,
            c,
            run: th.rord(),
        }));
    }
}

fn outbox_events(s: &OrchState, out: &mut Vec<Event>) {
    for m in &s.outbox {
        for how in [How::Ack, How::Keep] {
            let e = match *m {
                Message::Notice { child, run } => OutboxEvent::DoNotice { child, run, how },
                Message::Offer {
                    parent,
                    run,
                    generation,
                } => OutboxEvent::DoOffer {
                    parent,
                    run,
                    generation,
                    how,
                },
                Message::Stop { child, stop } => OutboxEvent::DoStop { child, stop, how },
                Message::QuestionUp { .. }
                | Message::AnswerDown { .. }
                | Message::QuestionClosed { .. }
                | Message::QuestionRelease { .. } => {
                    out.push(Event::Question(
                        zeron_orch::questions::QuestionEvent::Deliver { message: *m, how },
                    ));
                    continue;
                }
            };
            out.push(Event::Outbox(e));
        }
    }
    out.push(Event::Outbox(OutboxEvent::CrashEngine));
}

fn candidates(s: &OrchState) -> Vec<Event> {
    let mut out = Vec::new();
    for th in s.threads.values() {
        thread_events(th, &mut out);
        del_events(s, th, &mut out);
    }
    outbox_events(s, &mut out);
    out
}

fn noise(pick: u16, n: u64) -> Event {
    let t = ThreadId(u32::try_from(n % 7).unwrap_or(0));
    let small = u32::try_from((n >> 8) % 4).unwrap_or(0);
    let ev = ProviderEvent {
        node: if n & 1 == 0 { Node::Root } else { Node::Child },
        run: small,
        attempt: small,
        outcome: OUTCOMES[usize::from(pick) % 6],
    };
    match pick % 6 {
        0 => Event::Run(RunEvent::IngestEnd { t, ev }),
        1 => Event::Del(DelEvent::ReadResult {
            p: t,
            c: ThreadId(small),
            run: small,
        }),
        2 => Event::Outbox(OutboxEvent::DoOffer {
            parent: t,
            run: small,
            generation: small,
            how: How::Ack,
        }),
        3 => Event::Outbox(OutboxEvent::DoNotice {
            child: t,
            run: small,
            how: How::Keep,
        }),
        4 => Event::Del(DelEvent::Delegate {
            t,
            child: ThreadId(small),
            mode: DelegateMode::Wait,
            access: 0,
            reconfigure: false,
        }),
        _ => Event::Run(RunEvent::ProviderTerminal {
            t,
            run: small,
            attempt: small,
            outcome: ev.outcome,
        }),
    }
}

const RARE: [&str; 4] = ["Stop", "RequestInterrupt", "CrashEngine", "NarrowAccess"];

fn by_name(enabled: &[Event], pick: u16) -> Vec<Event> {
    let mut names: Vec<&str> = enabled.iter().map(Event::name).collect();
    names.sort_unstable();
    names.dedup();
    if !pick.is_multiple_of(7) {
        names.retain(|n| !RARE.contains(n));
    }
    let Some(name) = names.get(usize::from(pick / 10) % names.len().max(1)) else {
        return enabled.to_vec();
    };
    enabled
        .iter()
        .copied()
        .filter(|e| e.name() == *name)
        .collect()
}

fn choose(s: &OrchState, pick: u16, n: u64) -> Event {
    if pick.is_multiple_of(10) {
        return noise(pick / 10, n);
    }
    let all = candidates(s);
    let enabled: Vec<Event> = all
        .iter()
        .copied()
        .filter(|e| step(s, *e).is_ok())
        .collect();
    let pool = if pick % 10 == 1 || enabled.is_empty() {
        all
    } else {
        by_name(&enabled, pick)
    };
    pool[usize::try_from(n % u64::try_from(pool.len()).unwrap_or(1)).unwrap_or(0)]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn invariants_hold_after_every_accepted_step(script in prop::collection::vec(any::<(u16, u64)>(), 1..40)) {
        let mut s = init();
        let mut accepted = 0usize;
        for (pick, n) in script {
            let e = choose(&s, pick, n);
            let first = step(&s, e);
            prop_assert_eq!(&first, &step(&s, e), "step is not deterministic for {:?}", e);
            if let Ok((next, _)) = first {
                prop_assert_eq!(check_invariants(&next), Ok(()), "after {:?}", e);
                prop_assert_eq!(well_formed(&next), Ok(()), "after {:?}", e);
                s = next;
                accepted += 1;
            }
        }
        prop_assert!(accepted <= 40);
    }
}

#[test]
fn rejected_events_leave_the_state_unchanged() {
    let s = init();
    let before = s.clone();
    let e = Event::Run(RunEvent::StartFromQueue { t: ThreadId(0) });
    assert!(step(&s, e).is_err());
    assert_eq!(s, before);
}
