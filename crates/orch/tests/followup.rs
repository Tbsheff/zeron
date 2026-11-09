mod common;
use common::{A, H, R, del, outbox};
use zeron_orch::*;

fn ready() -> H {
    let mut h = H::new();
    h.live(R);
    h.ok(del(DelEvent::Delegate {
        t: R,
        child: A,
        mode: DelegateMode::Async,
        access: 2,
        reconfigure: false,
    }));
    h.start(A);
    h.end(A, Outcome::Completed);
    h.ok(outbox(OutboxEvent::DoNotice {
        child: A,
        run: 1,
        how: How::Keep,
    }));
    h.ok(del(DelEvent::ReadTaskResult {
        p: R,
        c: A,
        run: 1,
        task_run: 1,
    }));
    h
}

#[test]
fn two_tasks_keep_notices_and_acks_bound_to_the_child_run() {
    let mut h = ready();
    h.ok(del(DelEvent::Followup {
        parent: R,
        child: A,
        access: 2,
    }));
    assert_eq!(h.task(A).run, 2);
    h.start(A);
    h.ok(outbox(OutboxEvent::DoNotice {
        child: A,
        run: 1,
        how: How::Ack,
    }));
    assert_eq!(h.task(A).st, TaskSt::Running);
    h.end(A, Outcome::Completed);
    h.ok(outbox(OutboxEvent::DoNotice {
        child: A,
        run: 2,
        how: How::Ack,
    }));
    assert_eq!(
        h.no(del(DelEvent::ReadTaskResult {
            p: R,
            c: A,
            run: 1,
            task_run: 1
        })),
        Reject::StaleRun
    );
    assert_eq!(
        h.no(del(DelEvent::ReadResult { p: R, c: A, run: 1 })),
        Reject::StaleRun
    );
    assert_ne!(h.task(A).delivery, Delivery::Acknowledged);
    h.ok(del(DelEvent::ReadTaskResult {
        p: R,
        c: A,
        run: 1,
        task_run: 2,
    }));
    assert_eq!(h.task(A).delivery, Delivery::Acknowledged);
    h.ok(del(DelEvent::Followup {
        parent: R,
        child: A,
        access: 1,
    }));
    h.start(A);
    assert_eq!(h.task(A).run, 3);
}

#[test]
fn followups_enforce_handling_caps_access_and_stop() {
    let mut h = ready();
    assert_eq!(
        h.no(del(DelEvent::Followup {
            parent: R,
            child: A,
            access: 3
        })),
        Reject::AccessWider
    );
    h.s.config.max_fanout = 0;
    assert_eq!(
        h.no(del(DelEvent::Followup {
            parent: R,
            child: A,
            access: 2
        })),
        Reject::FanoutCap
    );
    h.s.config.max_fanout = 8;
    h.s.thread_mut(A).unwrap().held = Hold::Stop;
    assert_eq!(
        h.no(del(DelEvent::Followup {
            parent: R,
            child: A,
            access: 2
        })),
        Reject::Held
    );
    h.s.thread_mut(A).unwrap().held = Hold::None;
    h.ok(del(DelEvent::Stop { t: R }));
    assert_eq!(
        h.no(del(DelEvent::Followup {
            parent: R,
            child: A,
            access: 2
        })),
        Reject::InterruptRequested
    );
}
