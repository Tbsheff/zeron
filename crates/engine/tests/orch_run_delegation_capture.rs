use std::sync::Arc;

use zeron_engine::orchestrator::{Applied, Driver};
use zeron_orch::{
    Config, DelEvent, DelegateMode, Event, How, Node, OutboxEvent, Outcome, ProviderEvent, Reject,
    RunEvent, ThreadId,
};
use zeron_sync::DocsStore;

struct DriverFixture {
    _dir: tempfile::TempDir,
    driver: Driver,
    next_key: u32,
}

impl DriverFixture {
    fn new(device: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let driver = Driver::open_with_capture(store, device, Config::default(), true).unwrap();
        for id in 1..=8 {
            driver
                .put_meta(
                    &format!("thread:{id}"),
                    &format!("driver-component-chat-{id}"),
                )
                .unwrap();
        }
        Self {
            _dir: dir,
            driver,
            next_key: 0,
        }
    }

    fn apply(&mut self, event: Event, rejection: Option<Reject>) {
        self.next_key += 1;
        let key = format!("driver-component-{}", self.next_key);
        let result = self.driver.apply(&key, event).unwrap();
        assert!(!result.duplicate);
        match (result.applied, rejection) {
            (Applied::Accepted { .. }, None) => {}
            (Applied::Rejected { reason, .. }, Some(expected)) => assert_eq!(reason, expected),
            (_, expected) => panic!("driver component acceptance differs from {expected:?}"),
        }
    }

    fn run(&mut self, event: RunEvent) {
        self.apply(Event::Run(event), None);
    }
    fn del(&mut self, event: DelEvent) {
        self.apply(Event::Del(event), None);
    }
    fn out(&mut self, event: OutboxEvent) {
        self.apply(Event::Outbox(event), None);
    }
    fn reject(&mut self, event: Event, reason: Reject) {
        self.apply(event, Some(reason));
    }

    fn open(&mut self, t: ThreadId) {
        self.del(DelEvent::OpenRoot {
            t,
            access: 3,
            reconfigure: false,
        });
    }

    fn start(&mut self, t: ThreadId, msg: u64) {
        self.run(RunEvent::PersonSend {
            t,
            mode: zeron_orch::Mode::Queue,
            msg,
        });
        self.run(RunEvent::StartFromQueue { t });
    }

    fn current(&self, t: ThreadId, outcome: Outcome) -> ProviderEvent {
        let state = self.driver.state().unwrap();
        let th = state.thread(t).unwrap();
        ProviderEvent {
            node: Node::Root,
            run: th.rord(),
            attempt: th.attempt.n,
            outcome,
        }
    }

    fn terminal(&mut self, t: ThreadId, outcome: Outcome) -> ProviderEvent {
        let ev = self.current(t, outcome);
        self.run(RunEvent::ProviderTerminal {
            t,
            run: ev.run,
            attempt: ev.attempt,
            outcome,
        });
        ev
    }

    fn end(&mut self, t: ThreadId, outcome: Outcome) {
        let ev = self.terminal(t, outcome);
        self.run(RunEvent::IngestEnd { t, ev });
    }

    fn settle_child(&mut self, child: ThreadId) {
        self.run(RunEvent::StartFromQueue { t: child });
        self.end(child, Outcome::Completed);
        let run = self.driver.state().unwrap().task(child).unwrap().run;
        self.out(OutboxEvent::DoNotice {
            child,
            run,
            how: How::Ack,
        });
    }

    fn export(&self, test_name: &str) {
        let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") else {
            return;
        };
        let dir = std::path::PathBuf::from(root)
            .join(format!("driver-run-delegation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = format!("{}.jsonl", self.driver.device());
        let rows = self.driver.mechanism_lines().unwrap();
        assert!(!rows.is_empty());
        std::fs::write(dir.join(&file), rows.join("\n") + "\n").unwrap();
        std::fs::write(
            dir.join("capture.json"),
            serde_json::to_vec(&serde_json::json!({
                "test_name":test_name,"devices":[self.driver.device()],"filenames":[file],
                "capture":"present","scope":"driver-component","synthetic":true
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn driver_component_captures_run_attempts_limits_and_rejections() {
    let mut f = DriverFixture::new("driver-run-component");
    let t = ThreadId(1);
    f.open(t);
    f.reject(Event::Run(RunEvent::Steer { t, msg: 1 }), Reject::NotLive);
    f.start(t, 1);
    f.reject(
        Event::Run(RunEvent::PersonSend {
            t,
            mode: zeron_orch::Mode::Steer,
            msg: 2,
        }),
        Reject::SteerApplies,
    );
    f.run(RunEvent::Steer { t, msg: 3 });
    f.run(RunEvent::ChildStart { t });
    f.reject(Event::Run(RunEvent::ChildStart { t }), Reject::ChildOpen);
    f.run(RunEvent::ChildEnd { t });
    let child = f
        .driver
        .state()
        .unwrap()
        .thread(t)
        .unwrap()
        .inbox
        .iter()
        .copied()
        .find(|e| e.node == Node::Child)
        .unwrap();
    f.run(RunEvent::IngestChild { t, ev: child });
    f.run(RunEvent::RestartActive {
        t,
        mode: zeron_orch::Mode::Restart,
        msg: 4,
    });
    let stale = f
        .driver
        .state()
        .unwrap()
        .thread(t)
        .unwrap()
        .inbox
        .iter()
        .copied()
        .find(|e| e.node == Node::Root)
        .unwrap();
    f.run(RunEvent::IngestStale { t, ev: stale });
    f.reject(
        Event::Run(RunEvent::ProviderTerminal {
            t,
            run: 99,
            attempt: 1,
            outcome: Outcome::Completed,
        }),
        Reject::NotCurrent,
    );
    let transient = f.terminal(t, Outcome::Transient);
    f.reject(
        Event::Run(RunEvent::IngestEnd { t, ev: transient }),
        Reject::UseRetry,
    );
    f.run(RunEvent::RetryAttempt { t, ev: transient });
    f.run(RunEvent::RequestInterrupt { t });
    f.end(t, Outcome::Interrupted);
    f.start(t, 5);
    f.run(RunEvent::ServerEnqueue { t, msg: 6 });
    f.end(t, Outcome::UsageLimit);
    f.reject(
        Event::Run(RunEvent::UsageLimitRecovery { t }),
        Reject::ResetNotDue,
    );
    f.reject(
        Event::Run(RunEvent::ResetPasses { t, run: 99 }),
        Reject::TimerNotArmed,
    );
    let run = f.driver.state().unwrap().thread(t).unwrap().rord();
    f.run(RunEvent::ResetPasses { t, run });
    f.run(RunEvent::UsageLimitRecovery { t });
    f.end(t, Outcome::Completed);
    f.run(RunEvent::StartFromQueue { t });
    f.del(DelEvent::InterruptChat { t });
    f.end(t, Outcome::Interrupted);
    f.out(OutboxEvent::CrashEngine);
    f.export("driver_component_captures_run_attempts_limits_and_rejections");
}

#[test]
fn driver_component_captures_followup_identity_wakes_stops_and_rejections() {
    let mut f = DriverFixture::new("driver-delegation-component");
    let p = ThreadId(1);
    let c = ThreadId(2);
    f.open(p);
    f.start(p, 1);
    f.del(DelEvent::Delegate {
        t: p,
        child: c,
        mode: DelegateMode::Wait,
        access: 3,
        reconfigure: false,
    });
    f.reject(
        Event::Del(DelEvent::Followup {
            parent: p,
            child: c,
            access: 3,
        }),
        Reject::Waiting,
    );
    f.settle_child(c);
    f.reject(Event::Del(DelEvent::WaitTimeout { t: p }), Reject::Settled);
    f.del(DelEvent::WaitReturn { t: p });
    f.reject(
        Event::Del(DelEvent::WaitReturn { t: p }),
        Reject::NotWaiting,
    );
    f.del(DelEvent::Followup {
        parent: p,
        child: c,
        access: 2,
    });
    f.reject(
        Event::Del(DelEvent::ReadResult { p, c, run: 1 }),
        Reject::StaleRun,
    );
    f.settle_child(c);
    f.del(DelEvent::ReadTaskResult {
        p,
        c,
        run: 1,
        task_run: 2,
    });
    f.reject(
        Event::Del(DelEvent::ReadTaskResult {
            p,
            c,
            run: 1,
            task_run: 2,
        }),
        Reject::AlreadyHandled,
    );
    f.out(OutboxEvent::DoOffer {
        parent: p,
        run: 1,
        generation: 1,
        how: How::Ack,
    });
    f.reject(
        Event::Del(DelEvent::Followup {
            parent: p,
            child: c,
            access: 3,
        }),
        Reject::AccessWider,
    );
    f.del(DelEvent::Followup {
        parent: p,
        child: c,
        access: 1,
    });
    f.settle_child(c);
    f.out(OutboxEvent::DoOffer {
        parent: p,
        run: 1,
        generation: 2,
        how: How::Ack,
    });
    f.reject(
        Event::Outbox(OutboxEvent::DoOffer {
            parent: p,
            run: 1,
            generation: 2,
            how: How::Ack,
        }),
        Reject::NotInOutbox,
    );
    f.end(p, Outcome::Completed);
    f.del(DelEvent::StartFromQueueWake { t: p });
    assert_eq!(f.driver.state().unwrap().task(c).unwrap().run, 3);
    assert_eq!(f.driver.state().unwrap().thread(p).unwrap().rord(), 2);
    f.del(DelEvent::Followup {
        parent: p,
        child: c,
        access: 1,
    });
    f.settle_child(c);
    f.del(DelEvent::ReadTaskResult {
        p,
        c,
        run: 2,
        task_run: 4,
    });
    let task = f.driver.state().unwrap().task(c).unwrap();
    assert_eq!(task.run, 4);
    assert_eq!(task.cohort, 2);
    for (id, close) in [(3, "timeout"), (4, "abandon"), (5, "terminal")] {
        f.del(DelEvent::Delegate {
            t: p,
            child: ThreadId(id),
            mode: DelegateMode::Wait,
            access: 3,
            reconfigure: false,
        });
        match close {
            "timeout" => f.del(DelEvent::WaitTimeout { t: p }),
            "abandon" => f.del(DelEvent::WaitAbandon { t: p }),
            _ => {}
        }
    }
    f.reject(
        Event::Del(DelEvent::LateWaitAck { t: p }),
        Reject::NoLateWait,
    );
    f.del(DelEvent::InterruptChat { t: p });
    f.end(p, Outcome::Interrupted);
    f.del(DelEvent::LateWaitAck { t: p });
    f.start(p, 2);
    f.del(DelEvent::NarrowAccess { t: p, access: 2 });
    f.reject(
        Event::Del(DelEvent::NarrowAccess { t: p, access: 2 }),
        Reject::NotNarrower,
    );
    f.del(DelEvent::Stop { t: p });
    f.out(OutboxEvent::DoStop {
        child: ThreadId(3),
        stop: 1,
        how: How::Ack,
    });
    f.out(OutboxEvent::DoStop {
        child: ThreadId(4),
        stop: 1,
        how: How::Keep,
    });
    f.out(OutboxEvent::DoStop {
        child: ThreadId(4),
        stop: 1,
        how: How::Ack,
    });
    f.reject(
        Event::Outbox(OutboxEvent::DoStop {
            child: ThreadId(4),
            stop: 1,
            how: How::Ack,
        }),
        Reject::NotInOutbox,
    );
    let q = ThreadId(7);
    let d = ThreadId(8);
    f.open(q);
    f.start(q, 7);
    f.reject(
        Event::Del(DelEvent::Delegate {
            t: q,
            child: d,
            mode: DelegateMode::Async,
            access: 4,
            reconfigure: false,
        }),
        Reject::AccessWider,
    );
    f.del(DelEvent::Delegate {
        t: q,
        child: d,
        mode: DelegateMode::Async,
        access: 3,
        reconfigure: false,
    });
    f.settle_child(d);
    f.del(DelEvent::ReadResult { p: q, c: d, run: 1 });
    f.out(OutboxEvent::CrashEngine);
    f.export("driver_component_captures_followup_identity_wakes_stops_and_rejections");
}
