use zeron_orch::native::{NativeEvent as N, NativeStatus};
use zeron_orch::questions::QuestionEvent as Q;
use zeron_orch::*;
fn apply(s: &mut OrchState, e: impl Into<Event>) -> Vec<Effect> {
    let (next, fx) = step(s, e.into()).unwrap();
    *s = next;
    fx
}
fn r(e: RunEvent) -> Event {
    Event::Run(e)
}
fn d(e: DelEvent) -> Event {
    Event::Del(e)
}
fn q(e: Q) -> Event {
    Event::Question(e)
}
fn n(e: N) -> Event {
    Event::Native(e)
}
fn setup(wait: bool) -> OrchState {
    let mut s = OrchState::new(Config::default());
    apply(
        &mut s,
        d(DelEvent::OpenRoot {
            t: ThreadId(1),
            access: 2,
            reconfigure: true,
        }),
    );
    apply(
        &mut s,
        r(RunEvent::PersonSend {
            t: ThreadId(1),
            mode: Mode::Queue,
            msg: 1,
        }),
    );
    apply(&mut s, r(RunEvent::StartFromQueue { t: ThreadId(1) }));
    apply(
        &mut s,
        d(DelEvent::Delegate {
            t: ThreadId(1),
            child: ThreadId(2),
            mode: if wait {
                DelegateMode::Wait
            } else {
                DelegateMode::Async
            },
            access: 2,
            reconfigure: true,
        }),
    );
    apply(&mut s, r(RunEvent::StartFromQueue { t: ThreadId(2) }));
    s
}
fn raise(s: &mut OrchState) {
    apply(
        s,
        q(Q::Raise {
            child: ThreadId(2),
            qid: 5,
        }),
    );
    let m = *s
        .outbox
        .iter()
        .find(|m| matches!(m, Message::QuestionUp { .. }))
        .unwrap();
    apply(
        s,
        q(Q::Deliver {
            message: m,
            how: How::Ack,
        }),
    );
}
fn end(s: &mut OrchState, t: ThreadId, outcome: Outcome) {
    let th = s.thread(t).unwrap();
    let ev = ProviderEvent {
        node: Node::Root,
        run: th.rord(),
        attempt: th.attempt.n,
        outcome,
    };
    apply(
        s,
        r(RunEvent::ProviderTerminal {
            t,
            run: ev.run,
            attempt: ev.attempt,
            outcome,
        }),
    );
    apply(s, r(RunEvent::IngestEnd { t, ev }));
}
#[test]
fn person_wins_inflight_agent_answer() {
    let mut s = setup(true);
    raise(&mut s);
    apply(&mut s, d(DelEvent::WaitReturn { t: ThreadId(1) }));
    apply(
        &mut s,
        q(Q::AgentAnswer {
            agent: ThreadId(1),
            child: ThreadId(2),
            qid: 5,
            epoch: 1,
            answer: 11,
        }),
    );
    let answer = *s
        .outbox
        .iter()
        .find(|m| matches!(m, Message::AnswerDown { .. }))
        .unwrap();
    let fx = apply(
        &mut s,
        q(Q::PersonAnswer {
            child: ThreadId(2),
            qid: 5,
            epoch: 1,
            answer: 22,
        }),
    );
    assert_eq!(
        fx.iter()
            .filter(|e| matches!(e, Effect::RespondInput { answer: 22, .. }))
            .count(),
        1
    );
    let fx = apply(
        &mut s,
        q(Q::Deliver {
            message: answer,
            how: How::Keep,
        }),
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::RespondInput { .. })));
    assert!(
        apply(
            &mut s,
            q(Q::Deliver {
                message: answer,
                how: How::Ack
            })
        )
        .is_empty()
    );
}
#[test]
fn failed_run_catches_wait_only_question() {
    let mut s = setup(true);
    raise(&mut s);
    end(&mut s, ThreadId(1), Outcome::Failed);
    let question = &s.questions[&(ThreadId(2), 5)];
    assert_eq!(question.holder, None);
    assert!(question.waiting.is_empty());
    assert_eq!(question.epoch, 2);
}
#[test]
fn unanswered_seen_question_releases_past_root() {
    let mut s = setup(true);
    raise(&mut s);
    apply(&mut s, d(DelEvent::WaitReturn { t: ThreadId(1) }));
    end(&mut s, ThreadId(1), Outcome::Completed);
    let question = &s.questions[&(ThreadId(2), 5)];
    assert_eq!(question.epoch, 2);
    assert_eq!(question.holder, None);
    let m = *s
        .outbox
        .iter()
        .find(|m| {
            matches!(
                m,
                Message::QuestionUp {
                    epoch: 2,
                    to: None,
                    ..
                }
            )
        })
        .unwrap();
    let fx = apply(
        &mut s,
        q(Q::Deliver {
            message: m,
            how: How::Ack,
        }),
    );
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::NotifyQuestion { epoch: 2, .. }))
    );
    let fx = apply(
        &mut s,
        q(Q::PersonAnswer {
            child: ThreadId(2),
            qid: 5,
            epoch: 1,
            answer: 1,
        }),
    );
    assert!(fx.is_empty());
    assert!(s.questions[&(ThreadId(2), 5)].pending);
}
#[test]
fn background_survives_end_and_late_result_has_durable_destination() {
    let mut s = setup(false);
    apply(
        &mut s,
        n(N::Start {
            parent: ThreadId(1),
            key: 7,
            background: true,
        }),
    );
    apply(
        &mut s,
        n(N::Start {
            parent: ThreadId(1),
            key: 8,
            background: false,
        }),
    );
    end(&mut s, ThreadId(1), Outcome::Completed);
    assert_eq!(s.natives[&(ThreadId(1), 7)].status, NativeStatus::Running);
    assert_eq!(s.natives[&(ThreadId(1), 8)].status, NativeStatus::Orphaned);
    let before = s.thread(ThreadId(1)).unwrap().runs.clone();
    let fx = apply(
        &mut s,
        n(N::End {
            parent: ThreadId(1),
            key: 7,
            failed: false,
        }),
    );
    assert!(fx.contains(&Effect::DeliverNative {
        parent: ThreadId(1),
        key: 7,
        generation: 0
    }));
    assert_eq!(s.thread(ThreadId(1)).unwrap().runs, before);
    let wire = serde_json::to_vec(&s).unwrap();
    s = serde_json::from_slice(&wire).unwrap();
    let fx = apply(
        &mut s,
        n(N::Recover {
            parent: ThreadId(1),
            key: 7,
        }),
    );
    assert!(fx.contains(&Effect::DeliverNative {
        parent: ThreadId(1),
        key: 7,
        generation: 1
    }));
    apply(
        &mut s,
        n(N::Delivered {
            parent: ThreadId(1),
            key: 7,
            generation: 0,
        }),
    );
    assert_eq!(s.natives[&(ThreadId(1), 7)].delivery, Delivery::Claimed);
    apply(
        &mut s,
        n(N::Delivered {
            parent: ThreadId(1),
            key: 7,
            generation: 1,
        }),
    );
    assert_eq!(s.natives[&(ThreadId(1), 7)].delivery, Delivery::Delivered);
}
#[test]
fn retry_preserves_background_and_does_not_restart_native() {
    let mut s = setup(false);
    apply(
        &mut s,
        n(N::Start {
            parent: ThreadId(1),
            key: 7,
            background: true,
        }),
    );
    apply(
        &mut s,
        n(N::Start {
            parent: ThreadId(1),
            key: 8,
            background: false,
        }),
    );
    let ev = ProviderEvent {
        node: Node::Root,
        run: 1,
        attempt: 1,
        outcome: Outcome::Crash,
    };
    apply(
        &mut s,
        r(RunEvent::ProviderTerminal {
            t: ThreadId(1),
            run: 1,
            attempt: 1,
            outcome: Outcome::Crash,
        }),
    );
    apply(&mut s, r(RunEvent::RetryAttempt { t: ThreadId(1), ev }));
    assert_eq!(s.natives[&(ThreadId(1), 7)].status, NativeStatus::Running);
    assert_eq!(s.natives[&(ThreadId(1), 8)].status, NativeStatus::Orphaned);
    assert!(
        apply(
            &mut s,
            n(N::Start {
                parent: ThreadId(1),
                key: 7,
                background: true
            })
        )
        .is_empty()
    );
    assert_eq!(s.natives[&(ThreadId(1), 7)].attempt.n, 1);
}
#[test]
fn late_question_skips_failed_and_stopped_parent() {
    for stopped in [false, true] {
        let mut s = setup(false);
        apply(
            &mut s,
            q(Q::Raise {
                child: ThreadId(2),
                qid: 5,
            }),
        );
        if stopped {
            apply(&mut s, d(DelEvent::Stop { t: ThreadId(1) }));
            end(&mut s, ThreadId(1), Outcome::Interrupted);
        } else {
            end(&mut s, ThreadId(1), Outcome::Failed);
        }
        let m = *s
            .outbox
            .iter()
            .find(|m| {
                matches!(
                    m,
                    Message::QuestionUp {
                        to: Some(ThreadId(1)),
                        ..
                    }
                )
            })
            .unwrap();
        apply(
            &mut s,
            q(Q::Deliver {
                message: m,
                how: How::Ack,
            }),
        );
        assert_eq!(s.questions[&(ThreadId(2), 5)].holder, None);
        assert!(
            s.outbox
                .iter()
                .any(|m| matches!(m, Message::QuestionUp { to: None, .. }))
        );
    }
}
#[test]
fn stop_releases_idle_holder_without_run_end() {
    let mut s = setup(false);
    end(&mut s, ThreadId(1), Outcome::Completed);
    raise(&mut s);
    assert_eq!(s.questions[&(ThreadId(2), 5)].holder, Some(ThreadId(1)));
    apply(&mut s, d(DelEvent::Stop { t: ThreadId(1) }));
    assert_eq!(s.questions[&(ThreadId(2), 5)].holder, None);
    assert_eq!(s.questions[&(ThreadId(2), 5)].epoch, 2);
}
#[test]
fn terminal_wait_and_superseded_attempt_cannot_observe_question() {
    let mut s = setup(true);
    raise(&mut s);
    apply(&mut s, d(DelEvent::InterruptChat { t: ThreadId(1) }));
    end(&mut s, ThreadId(1), Outcome::Interrupted);
    assert!(step(&s, d(DelEvent::WaitReturn { t: ThreadId(1) })).is_err());
    for outcome in [Outcome::Crash, Outcome::Transient] {
        let mut s = setup(true);
        let ev = ProviderEvent {
            node: Node::Root,
            run: 1,
            attempt: 1,
            outcome,
        };
        apply(
            &mut s,
            r(RunEvent::ProviderTerminal {
                t: ThreadId(1),
                run: 1,
                attempt: 1,
                outcome,
            }),
        );
        apply(&mut s, r(RunEvent::RetryAttempt { t: ThreadId(1), ev }));
        assert_eq!(s.thread(ThreadId(1)).unwrap().wait_on, None);
    }
    let mut s = setup(true);
    apply(
        &mut s,
        r(RunEvent::RestartActive {
            t: ThreadId(1),
            mode: Mode::Restart,
            msg: 8,
        }),
    );
    assert_eq!(s.thread(ThreadId(1)).unwrap().wait_on, None);
}
#[test]
fn future_epoch_answer_waits_for_host_release() {
    let mut s = setup(false);
    raise(&mut s);
    let fx = apply(
        &mut s,
        q(Q::ImportAnswer {
            child: ThreadId(2),
            qid: 5,
            epoch: 2,
            via: ThreadId(1),
            answer: 41,
        }),
    );
    assert!(fx.is_empty());
    assert!(s.questions[&(ThreadId(2), 5)].pending);
    apply(
        &mut s,
        q(Q::ImportAnswer {
            child: ThreadId(2),
            qid: 5,
            epoch: 2,
            via: ThreadId(1),
            answer: 42,
        }),
    );
    let fx = apply(
        &mut s,
        q(Q::ImportRelease {
            child: ThreadId(2),
            qid: 5,
            epoch: 2,
            via: ThreadId(1),
        }),
    );
    assert!(fx.contains(&Effect::RespondInput {
        child: ThreadId(2),
        qid: 5,
        epoch: 2,
        answer: 41
    }));
    assert!(!s.questions[&(ThreadId(2), 5)].pending);
}

#[test]
fn host_closure_retracts_a_newer_shadow_epoch() {
    let mut s = setup(true);
    raise(&mut s);
    apply(&mut s, d(DelEvent::WaitReturn { t: ThreadId(1) }));
    end(&mut s, ThreadId(1), Outcome::Completed);
    assert_eq!(s.questions[&(ThreadId(2), 5)].epoch, 2);
    apply(
        &mut s,
        q(Q::ImportClosed {
            child: ThreadId(2),
            qid: 5,
            epoch: 1,
            to: ThreadId(1),
        }),
    );
    let question = &s.questions[&(ThreadId(2), 5)];
    assert!(!question.pending);
    assert!(question.views.is_empty());
    assert_eq!(question.epoch, 2);
}
