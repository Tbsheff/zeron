mod common;

use common::{A, B, C, H, R, del, outbox, run};
use zeron_orch::{
    CohortSt, Config, DelEvent, DelegateMode, Delivery, Disp, Effect, Hold, How, Message, Mode,
    Msg, Node, OutboxEvent, Outcome, ProviderEvent, Reason, Reject, RunEvent, RunKind, RunStatus,
    Start, TaskSt, ThreadId, WakeRule,
};

#[test]
fn rl1_person_send_queues_releases_hold_and_respects_cap() {
    let mut h = H::with(Config {
        max_queue: 1,
        ..Config::default()
    });
    h.s.threads.get_mut(&R).expect("root").held = Hold::Fail;
    h.send(R);
    assert_eq!(h.th(R).held, Hold::None);
    assert_eq!(
        h.no(run(RunEvent::PersonSend {
            t: R,
            mode: Mode::Queue,
            msg: 9
        })),
        Reject::QueueFull
    );
}

#[test]
fn rl2_start_from_queue_needs_idle_unheld_non_wake_head() {
    let mut h = H::new();
    assert_eq!(
        h.no(run(RunEvent::StartFromQueue { t: R })),
        Reject::QueueEmpty
    );
    h.send(R);
    let fx = h.start(R);
    assert!(matches!(
        fx[0],
        Effect::StartRun {
            start: Start::Queued(Msg::Person(_)),
            ..
        }
    ));
    assert_eq!(h.th(R).attempt.n, 1);
    h.send(R);
    assert_eq!(
        h.no(run(RunEvent::StartFromQueue { t: R })),
        Reject::RunLive
    );
    h.end(R, Outcome::Failed);
    assert_eq!(h.th(R).held, Hold::Fail);
    assert_eq!(h.no(run(RunEvent::StartFromQueue { t: R })), Reject::Held);
}

#[test]
fn rl3_steer_joins_active_attempt_until_terminal_or_stop() {
    let mut h = H::new();
    h.live(R);
    assert_eq!(
        h.no(run(RunEvent::PersonSend {
            t: R,
            mode: Mode::Steer,
            msg: 5
        })),
        Reject::SteerApplies
    );
    let fx = h.ok(run(RunEvent::Steer { t: R, msg: 5 }));
    assert!(matches!(fx[0], Effect::SteerRun { msg: 5, .. }));
    h.emit(R, Outcome::Completed);
    assert_eq!(h.no(run(RunEvent::Steer { t: R, msg: 6 })), Reject::TooLate);
    h.ok(run(RunEvent::PersonSend {
        t: R,
        mode: Mode::Steer,
        msg: 6,
    }));
    assert_eq!(h.th(R).queue.len(), 1);
}

#[test]
fn rl4_restart_active_supersedes_attempt_and_old_terminal_is_stale() {
    let mut h = H::new();
    h.live(R);
    let old = h.current(R, Outcome::Interrupted);
    let fx = h.ok(run(RunEvent::RestartActive {
        t: R,
        mode: Mode::Restart,
        msg: 7,
    }));
    assert!(matches!(fx[0], Effect::InterruptAttempt { .. }));
    assert!(matches!(
        fx[1],
        Effect::StartAttempt {
            reason: Reason::SteeringRestart,
            msg: Some(7),
            ..
        }
    ));
    assert_eq!(h.th(R).attempt.n, 2);
    assert_eq!(
        h.no(run(RunEvent::IngestEnd { t: R, ev: old })),
        Reject::NotCurrent
    );
    h.ok(run(RunEvent::IngestStale { t: R, ev: old }));
    assert!(h.th(R).is_live());
    let mode = Mode::Queue;
    assert_eq!(
        h.no(run(RunEvent::RestartActive { t: R, mode, msg: 8 })),
        Reject::NotRestartMode
    );
}

#[test]
fn rl5_retry_attempt_uses_budget_then_fails_the_run() {
    let mut h = H::with(Config {
        max_retry: 1,
        ..Config::default()
    });
    h.live(R);
    let ev = h.emit(R, Outcome::Transient);
    assert_eq!(
        h.no(run(RunEvent::IngestEnd { t: R, ev })),
        Reject::UseRetry
    );
    let fx = h.ok(run(RunEvent::RetryAttempt { t: R, ev }));
    assert!(matches!(
        fx[0],
        Effect::StartAttempt {
            reason: Reason::Retry,
            ..
        }
    ));
    let ev = h.emit(R, Outcome::Crash);
    assert_eq!(
        h.no(run(RunEvent::RetryAttempt { t: R, ev })),
        Reject::RetryBudget
    );
    h.ok(run(RunEvent::IngestEnd { t: R, ev }));
    assert_eq!(h.th(R).last_out(), Some(RunStatus::Failed));
}

#[test]
fn rl5_r7_no_retry_after_stop_request() {
    let mut h = H::new();
    h.live(R);
    h.ok(run(RunEvent::RequestInterrupt { t: R }));
    let ev = h.emit(R, Outcome::Transient);
    assert_eq!(
        h.no(run(RunEvent::RetryAttempt { t: R, ev })),
        Reject::InterruptRequested
    );
    h.ok(run(RunEvent::IngestEnd { t: R, ev }));
    assert_eq!(h.th(R).held, Hold::Stop);
}

#[test]
fn rl6_rl7_interrupt_is_a_request_and_only_provider_confirms() {
    let mut h = H::new();
    h.live(R);
    let ev = h.current(R, Outcome::Interrupted);
    let ptx = run(RunEvent::ProviderTerminal {
        t: R,
        run: ev.run,
        attempt: ev.attempt,
        outcome: ev.outcome,
    });
    assert_eq!(h.no(ptx), Reject::InterruptNotRequested);
    let fx = h.ok(run(RunEvent::RequestInterrupt { t: R }));
    assert!(matches!(fx[0], Effect::InterruptRun { .. }));
    assert!(h.th(R).is_live());
    h.end(R, Outcome::Interrupted);
    assert_eq!(h.th(R).last_out(), Some(RunStatus::Interrupted));
}

#[test]
fn s2_child_node_never_ends_the_run() {
    let mut h = H::new();
    h.live(R);
    h.ok(run(RunEvent::ChildStart { t: R }));
    assert_eq!(h.no(run(RunEvent::ChildStart { t: R })), Reject::ChildOpen);
    h.ok(run(RunEvent::ChildEnd { t: R }));
    let ev = ProviderEvent {
        node: Node::Child,
        run: 1,
        attempt: 1,
        outcome: Outcome::Completed,
    };
    assert_eq!(h.no(run(RunEvent::IngestEnd { t: R, ev })), Reject::NotRoot);
    h.ok(run(RunEvent::IngestChild { t: R, ev }));
    assert!(h.th(R).is_live());
}

#[test]
fn rl8_root_complete_waits_for_open_wait() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    let ev = h.emit(R, Outcome::Completed);
    assert_eq!(h.no(run(RunEvent::IngestEnd { t: R, ev })), Reject::Waiting);
    h.ok(del(DelEvent::WaitAbandon { t: R }));
    h.ok(run(RunEvent::IngestEnd { t: R, ev }));
}

#[test]
fn rl9_usage_limit_holds_and_arms_timer_rl11_resume_releases_hold_d6() {
    let mut h = H::new();
    h.live(R);
    h.send(R);
    let fx = h.end(R, Outcome::UsageLimit);
    assert!(matches!(fx[0], Effect::ArmTimer { .. }));
    assert_eq!(h.th(R).held, Hold::Limit);
    assert_eq!(
        h.no(run(RunEvent::UsageLimitRecovery { t: R })),
        Reject::ResetNotDue
    );
    assert_eq!(
        h.no(run(RunEvent::ResetPasses { t: R, run: 2 })),
        Reject::TimerNotArmed
    );
    h.ok(run(RunEvent::ResetPasses { t: R, run: 1 }));
    assert_eq!(
        h.no(run(RunEvent::ResetPasses { t: R, run: 1 })),
        Reject::TimerAlreadyFired
    );
    let fx = h.ok(run(RunEvent::UsageLimitRecovery { t: R }));
    assert!(matches!(
        fx[0],
        Effect::StartRun {
            start: Start::Resume,
            ..
        }
    ));
    assert_eq!(h.th(R).held, Hold::None);
    h.end(R, Outcome::Completed);
    h.start(R);
    assert_eq!(h.th(R).latest_kind(), Some(RunKind::Person));
}

#[test]
fn r24_stop_disarms_usage_limit_resume() {
    let mut h = H::new();
    h.live(R);
    h.end(R, Outcome::UsageLimit);
    h.ok(run(RunEvent::RequestInterrupt { t: R }));
    assert_eq!(h.th(R).ul.armed, None);
    assert_eq!(
        h.no(run(RunEvent::ResetPasses { t: R, run: 1 })),
        Reject::TimerNotArmed
    );
    assert_eq!(
        h.no(run(RunEvent::UsageLimitRecovery { t: R })),
        Reject::UsageLimitNotArmed
    );
}

#[test]
fn r24_usage_limit_after_stop_keeps_stop_hold_and_arms_nothing() {
    let mut h = H::new();
    h.live(R);
    h.ok(del(DelEvent::Stop { t: R }));
    let fx = h.end(R, Outcome::UsageLimit);
    assert!(fx.iter().all(|e| !matches!(e, Effect::ArmTimer { .. })));
    assert_eq!(h.th(R).held, Hold::Stop);
}

#[test]
fn t1_delegate_creates_task_and_wait_blocks_second_delegate() {
    let mut h = H::new();
    assert_eq!(
        h.no(del(DelEvent::Delegate {
            t: R,
            child: A,
            mode: DelegateMode::Async,
            access: 0,
            reconfigure: true
        })),
        Reject::NotLive
    );
    h.live(R);
    let fx = h.delegate(R, A, DelegateMode::Wait);
    assert!(matches!(
        fx[0],
        Effect::CreateChild {
            parent: R,
            child: A,
            depth: 1,
            mode: DelegateMode::Wait,
            ..
        }
    ));
    assert_eq!(h.task(A).wake, WakeRule::SettledOnly);
    assert_eq!(h.th(A).queue.front(), Some(&Msg::Task));
    let e = del(DelEvent::Delegate {
        t: R,
        child: B,
        mode: DelegateMode::Async,
        access: 0,
        reconfigure: true,
    });
    assert_eq!(h.no(e), Reject::Waiting);
    h.ok(del(DelEvent::WaitAbandon { t: R }));
    assert_eq!(
        h.no(del(DelEvent::Delegate {
            t: R,
            child: A,
            mode: DelegateMode::Async,
            access: 0,
            reconfigure: true
        })),
        Reject::ThreadExists
    );
    assert_eq!(
        h.no(del(DelEvent::Delegate {
            t: R,
            child: B,
            mode: DelegateMode::Async,
            access: 3,
            reconfigure: true
        })),
        Reject::AccessWider
    );
}

#[test]
fn t1_r3_depth_cap_two_allows_grandchild_only() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.start(A);
    h.delegate(A, B, DelegateMode::Async);
    h.start(B);
    let e = del(DelEvent::Delegate {
        t: B,
        child: C,
        mode: DelegateMode::Async,
        access: 0,
        reconfigure: true,
    });
    assert_eq!(h.no(e), Reject::DepthCap);
}

#[test]
fn t1_r10_fanout_counts_unsettled_tasks() {
    let mut h = H::with(Config {
        max_fanout: 1,
        ..Config::default()
    });
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    let e = del(DelEvent::Delegate {
        t: R,
        child: B,
        mode: DelegateMode::Async,
        access: 0,
        reconfigure: true,
    });
    assert_eq!(h.no(e), Reject::FanoutCap);
    h.finish_child(A);
    h.ok(e);
}

#[test]
fn s4_d7_delegate_after_stop_refused_locally_only() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.start(A);
    h.ok(del(DelEvent::Stop { t: R }));
    let e = del(DelEvent::Delegate {
        t: R,
        child: C,
        mode: DelegateMode::Async,
        access: 0,
        reconfigure: true,
    });
    assert_eq!(h.no(e), Reject::InterruptRequested);
    assert!(h.s.outbox.contains(&Message::Stop { child: A, stop: 1 }));
    h.delegate(A, B, DelegateMode::Async);
    h.ok(outbox(OutboxEvent::DoStop {
        child: A,
        stop: 1,
        how: How::Ack,
    }));
    assert!(h.th(A).int_req);
    assert!(h.s.outbox.contains(&Message::Stop { child: B, stop: 1 }));
}

#[test]
fn t2_settle_plans_wake_t2_prime_duplicate_notice_is_noop() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.start(A);
    let fx = h.end(A, Outcome::Completed);
    assert!(fx.contains(&Effect::Send(Message::Notice { child: A, run: 1 })));
    let fx = h.notice(A, How::Keep);
    assert!(fx.contains(&Effect::Send(Message::Offer {
        parent: R,
        run: 1,
        generation: 1
    })));
    assert_eq!(h.task(A).delivery, Delivery::Claimed);
    let before = h.s.clone();
    let fx = h.notice(A, How::Ack);
    assert!(fx.is_empty());
    assert_eq!(h.s.threads, before.threads);
    assert_eq!(
        h.no(outbox(OutboxEvent::DoNotice {
            child: A,
            run: 1,
            how: How::Ack
        })),
        Reject::NotInOutbox
    );
}

#[test]
fn t3_f3_two_results_join_one_wake() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.delegate(R, B, DelegateMode::Async);
    h.finish_child(A);
    let fx = h.finish_child(B);
    assert_eq!(
        fx,
        vec![Effect::Send(Message::Offer {
            parent: R,
            run: 1,
            generation: 1
        })]
    );
    h.deliver_offer(R);
    let fx = h.end(R, Outcome::Completed);
    assert!(fx.is_empty());
    let fx = h.ok(del(DelEvent::StartFromQueueWake { t: R }));
    let Effect::StartRun { delivered, .. } = &fx[0] else {
        panic!("no start")
    };
    assert_eq!(delivered.len(), 2);
    assert_eq!(h.task(A).pushes, 1);
    assert_eq!(h.task(B).delivery, Delivery::Delivered);
}

#[test]
fn t3_prime_stopped_cohort_disposes_results() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.start(A);
    h.end(A, Outcome::Completed);
    h.ok(del(DelEvent::Stop { t: R }));
    assert_eq!(h.task(A).delivery, Delivery::Disposed);
    h.notice(A, How::Ack);
    assert_eq!(h.task(A).delivery, Delivery::Disposed);
    assert!(h.offers().is_empty());
}

#[test]
fn t4_prime_stale_offer_generation_is_noop() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.finish_child(A);
    h.s.outbox.insert(Message::Offer {
        parent: R,
        run: 1,
        generation: 9,
    });
    let before = h.s.threads.clone();
    h.ok(outbox(OutboxEvent::DoOffer {
        parent: R,
        run: 1,
        generation: 9,
        how: How::Ack,
    }));
    assert_eq!(h.s.threads, before);
}

#[test]
fn t6_s4_wake_waits_behind_live_run_and_reoffers_pending_siblings() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.delegate(R, B, DelegateMode::Async);
    h.finish_child(A);
    h.deliver_offer(R);
    assert_eq!(
        h.no(del(DelEvent::StartFromQueueWake { t: R })),
        Reject::RunLive
    );
    h.end(R, Outcome::Completed);
    h.finish_child(B);
    assert_eq!(h.task(B).delivery, Delivery::Claimed);
    h.ok(del(DelEvent::StartFromQueueWake { t: R }));
    assert_eq!(h.task(A).delivery, Delivery::Delivered);
    assert_eq!(h.task(B).delivery, Delivery::Delivered);
}

#[test]
fn t3_rl10_sweep_joins_settled_only_result_into_queued_wake() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.delegate(R, B, DelegateMode::Wait);
    h.finish_child(A);
    h.deliver_offer(R);
    h.finish_child(B);
    assert_eq!(h.task(B).delivery, Delivery::Pending);
    h.ok(del(DelEvent::WaitAbandon { t: R }));
    let fx = h.end(R, Outcome::Completed);
    assert!(fx.is_empty());
    assert_eq!(h.task(B).delivery, Delivery::Claimed);
    let fx = h.ok(del(DelEvent::StartFromQueueWake { t: R }));
    let Effect::StartRun { delivered, .. } = &fx[0] else {
        panic!("no start")
    };
    assert_eq!(delivered.len(), 2);
}

#[test]
fn r1_r9_sweep_on_interrupted_end_delivers_waited_result() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    h.ok(del(DelEvent::InterruptChat { t: R }));
    h.finish_child(A);
    assert_eq!(h.task(A).delivery, Delivery::Pending);
    let fx = h.end(R, Outcome::Interrupted);
    assert!(fx.contains(&Effect::Send(Message::Offer {
        parent: R,
        run: 1,
        generation: 1
    })));
    assert_eq!(h.task(A).delivery, Delivery::Claimed);
    assert_eq!(h.th(R).late_wait, Some(A));
}

#[test]
fn t7_read_result_acks_only_from_live_reading_run_and_cancels_wake() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    assert_eq!(
        h.no(del(DelEvent::ReadResult { p: R, c: A, run: 1 })),
        Reject::NotSettled
    );
    h.finish_child(A);
    h.deliver_offer(R);
    assert_eq!(
        h.no(del(DelEvent::ReadResult { p: R, c: A, run: 0 })),
        Reject::StaleRun
    );
    assert_eq!(
        h.no(del(DelEvent::ReadResult { p: A, c: A, run: 1 })),
        Reject::NotLive
    );
    h.ok(del(DelEvent::ReadResult { p: R, c: A, run: 1 }));
    assert_eq!(h.task(A).delivery, Delivery::Acknowledged);
    assert_eq!(h.th(R).cohort(1).map(|c| c.st), Some(CohortSt::None));
    assert!(h.th(R).queue.is_empty());
    assert_eq!(
        h.no(del(DelEvent::ReadResult { p: R, c: A, run: 1 })),
        Reject::AlreadyHandled
    );
}

#[test]
fn t8_wait_return_delivers_inside_the_run() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    assert_eq!(h.no(del(DelEvent::WaitReturn { t: R })), Reject::NotSettled);
    h.finish_child(A);
    assert_eq!(h.task(A).delivery, Delivery::Pending);
    let fx = h.ok(del(DelEvent::WaitReturn { t: R }));
    assert!(matches!(
        fx[0],
        Effect::CompleteWait {
            outcome: zeron_orch::WaitOutcome::Result,
            ..
        }
    ));
    assert_eq!(h.task(A).delivery, Delivery::Acknowledged);
    assert_eq!(h.task(A).pushes, 1);
}

#[test]
fn t9_wait_timeout_upgrades_wake_to_always() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    let fx = h.ok(del(DelEvent::WaitTimeout { t: R }));
    assert!(matches!(
        fx[0],
        Effect::CompleteWait {
            outcome: zeron_orch::WaitOutcome::TimedOut,
            ..
        }
    ));
    assert_eq!(h.task(A).wake, WakeRule::Always);
    h.finish_child(A);
    assert_eq!(h.task(A).delivery, Delivery::Claimed);
    assert_eq!(
        h.no(del(DelEvent::WaitTimeout { t: R })),
        Reject::NotWaiting
    );
}

#[test]
fn t11_r2_late_wait_ack_never_acknowledges() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    h.end(R, Outcome::Failed);
    assert_eq!(h.th(R).late_wait, Some(A));
    h.finish_child(A);
    h.ok(del(DelEvent::LateWaitAck { t: R }));
    assert_eq!(h.task(A).delivery, Delivery::Claimed);
    assert_eq!(h.task(A).pushes, 0);
    assert_eq!(
        h.no(del(DelEvent::LateWaitAck { t: R })),
        Reject::NoLateWait
    );
}

#[test]
fn r21_failure_hold_lets_wake_pass_but_stop_hold_does_not() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.end(R, Outcome::Failed);
    h.finish_child(A);
    h.deliver_offer(R);
    assert_eq!(h.th(R).held, Hold::Fail);
    h.ok(del(DelEvent::StartFromQueueWake { t: R }));
    assert_eq!(h.task(A).delivery, Delivery::Delivered);

    let mut h = H::new();
    h.live(R);
    h.delegate(R, B, DelegateMode::Async);
    h.finish_child(B);
    h.deliver_offer(R);
    h.ok(del(DelEvent::InterruptChat { t: R }));
    h.end(R, Outcome::Interrupted);
    h.s.threads.get_mut(&R).expect("root").held = Hold::Stop;
    assert_eq!(
        h.no(del(DelEvent::StartFromQueueWake { t: R })),
        Reject::Held
    );
}

#[test]
fn r4_d3_narrow_access_clamps_descendants_at_once() {
    let mut h = H::new();
    h.live(R);
    h.ok(del(DelEvent::Delegate {
        t: R,
        child: A,
        mode: DelegateMode::Async,
        access: 2,
        reconfigure: true,
    }));
    h.start(A);
    h.ok(del(DelEvent::Delegate {
        t: A,
        child: B,
        mode: DelegateMode::Async,
        access: 2,
        reconfigure: false,
    }));
    h.start(B);
    assert_eq!(
        h.no(del(DelEvent::NarrowAccess { t: R, access: 2 })),
        Reject::NotNarrower
    );
    let fx = h.ok(del(DelEvent::NarrowAccess { t: R, access: 1 }));
    assert_eq!(h.th(A).access, 1);
    assert_eq!(h.th(B).access, 1);
    assert!(fx.contains(&Effect::Reconfigure { t: R, access: 1 }));
    assert!(fx.contains(&Effect::Reconfigure { t: A, access: 1 }));
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::StartAttempt {
            reason: Reason::AccessRestart,
            access: 1,
            ..
        }
    )));
    assert_eq!(h.th(B).attempt.reason, Reason::AccessRestart);
}

#[test]
fn r5_stop_cascades_three_levels_through_durable_outbox() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.start(A);
    h.delegate(A, B, DelegateMode::Async);
    h.start(B);
    let fx = h.ok(del(DelEvent::Stop { t: R }));
    assert!(fx.contains(&Effect::Send(Message::Stop { child: A, stop: 1 })));
    assert!(h.th(A).is_live());
    let fx = h.ok(outbox(OutboxEvent::DoStop {
        child: A,
        stop: 1,
        how: How::Keep,
    }));
    assert!(fx.contains(&Effect::Send(Message::Stop { child: B, stop: 1 })));
    assert!(h.th(A).int_req);
    h.ok(outbox(OutboxEvent::DoStop {
        child: B,
        stop: 1,
        how: How::Ack,
    }));
    assert!(h.th(B).int_req);
    assert_eq!(h.th(B).held, Hold::Stop);
    assert!(h.th(A).runs.iter().all(|r| r.cohort.disp == Disp::Stopped));
}

#[test]
fn s10a_stop_receipt_makes_replay_a_noop_after_resume() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Async);
    h.ok(del(DelEvent::Stop { t: R }));
    h.ok(outbox(OutboxEvent::DoStop {
        child: A,
        stop: 1,
        how: How::Keep,
    }));
    h.send(A);
    assert_eq!(h.th(A).held, Hold::None);
    let before = h.s.threads.clone();
    h.ok(outbox(OutboxEvent::DoStop {
        child: A,
        stop: 1,
        how: How::Ack,
    }));
    assert_eq!(h.s.threads, before);
}

#[test]
fn crash_engine_drops_in_memory_waits_only() {
    let mut h = H::new();
    h.live(R);
    h.delegate(R, A, DelegateMode::Wait);
    h.ok(outbox(OutboxEvent::CrashEngine));
    assert_eq!(h.th(R).wait_on, None);
    assert_eq!(h.task(A).st, TaskSt::Running);
    assert!(h.th(R).is_live());
}

#[test]
fn stale_wake_is_refused_as_an_assertion() {
    let mut h = H::new();
    h.live(R);
    h.end(R, Outcome::Completed);
    h.s.threads
        .get_mut(&R)
        .expect("root")
        .queue
        .push_back(Msg::Wake {
            run: 1,
            generation: 3,
        });
    assert_eq!(
        h.no(del(DelEvent::StartFromQueueWake { t: R })),
        Reject::StaleWake
    );
    assert_eq!(
        h.no(run(RunEvent::StartFromQueue { t: R })),
        Reject::HeadIsWake
    );
}

#[test]
fn unknown_threads_and_corrupt_links_reject_without_panic() {
    let h = H::new();
    let ghost = ThreadId(42);
    assert_eq!(
        h.no(run(RunEvent::StartFromQueue { t: ghost })),
        Reject::UnknownThread
    );
    assert_eq!(
        h.no(del(DelEvent::Stop { t: ghost })),
        Reject::UnknownThread
    );
    assert_eq!(
        h.no(outbox(OutboxEvent::DoNotice {
            child: ghost,
            run: 1,
            how: How::Ack
        })),
        Reject::NotInOutbox
    );
    assert_eq!(
        h.no(del(DelEvent::OpenRoot {
            t: R,
            access: 0,
            reconfigure: false
        })),
        Reject::ThreadExists
    );
}

#[test]
fn ingest_rejects_events_not_in_inbox_or_not_current() {
    let mut h = H::new();
    h.live(R);
    let ev = h.current(R, Outcome::Completed);
    assert_eq!(
        h.no(run(RunEvent::IngestEnd { t: R, ev })),
        Reject::NotInInbox
    );
    h.emit(R, Outcome::Completed);
    assert_eq!(
        h.no(run(RunEvent::IngestStale { t: R, ev })),
        Reject::Current
    );
    assert_eq!(
        h.no(run(RunEvent::RetryAttempt { t: R, ev })),
        Reject::NotRetryable
    );
    assert_eq!(
        h.no(run(RunEvent::ProviderTerminal {
            t: R,
            run: 1,
            attempt: 1,
            outcome: Outcome::Failed
        })),
        Reject::AlreadyEmitted
    );
    assert_eq!(
        h.no(run(RunEvent::ProviderTerminal {
            t: R,
            run: 2,
            attempt: 1,
            outcome: Outcome::Failed
        })),
        Reject::NotCurrent
    );
}
