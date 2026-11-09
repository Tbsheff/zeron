#![allow(dead_code)]

use zeron_orch::{
    Config, DelEvent, DelegateMode, Effect, Event, How, Message, Mode, Node, OrchState,
    OutboxEvent, Outcome, ProviderEvent, Reject, RunEvent, Task, Thread, ThreadId,
    check_invariants, step, well_formed,
};

pub const R: ThreadId = ThreadId(0);
pub const A: ThreadId = ThreadId(1);
pub const B: ThreadId = ThreadId(2);
pub const C: ThreadId = ThreadId(3);

pub fn run(e: RunEvent) -> Event {
    Event::Run(e)
}

pub fn del(e: DelEvent) -> Event {
    Event::Del(e)
}

pub fn outbox(e: OutboxEvent) -> Event {
    Event::Outbox(e)
}

pub struct H {
    pub s: OrchState,
    msg: u64,
}

impl H {
    pub fn new() -> H {
        H::with(Config::default())
    }

    pub fn with(cfg: Config) -> H {
        let mut h = H {
            s: OrchState::new(cfg),
            msg: 0,
        };
        h.ok(del(DelEvent::OpenRoot {
            t: R,
            access: 2,
            reconfigure: true,
        }));
        h
    }

    pub fn ok(&mut self, e: Event) -> Vec<Effect> {
        let (next, fx) = step(&self.s, e).unwrap_or_else(|r| panic!("{e:?} rejected: {r:?}"));
        check_invariants(&next).unwrap_or_else(|v| panic!("{e:?} broke {v:?}"));
        well_formed(&next).unwrap_or_else(|m| panic!("{e:?} malformed {m:?}"));
        self.s = next;
        fx
    }

    pub fn no(&self, e: Event) -> Reject {
        match step(&self.s, e) {
            Err(r) => r,
            Ok(_) => panic!("{e:?} accepted"),
        }
    }

    pub fn th(&self, t: ThreadId) -> &Thread {
        &self.s.threads[&t]
    }

    pub fn task(&self, c: ThreadId) -> Task {
        self.s.threads[&c].task.expect("task")
    }

    pub fn next_msg(&mut self) -> u64 {
        self.msg += 1;
        self.msg
    }

    pub fn send(&mut self, t: ThreadId) -> Vec<Effect> {
        let msg = self.next_msg();
        self.ok(run(RunEvent::PersonSend {
            t,
            mode: Mode::Queue,
            msg,
        }))
    }

    pub fn start(&mut self, t: ThreadId) -> Vec<Effect> {
        self.ok(run(RunEvent::StartFromQueue { t }))
    }

    pub fn live(&mut self, t: ThreadId) {
        self.send(t);
        self.start(t);
    }

    pub fn current(&self, t: ThreadId, outcome: Outcome) -> ProviderEvent {
        let th = self.th(t);
        ProviderEvent {
            node: Node::Root,
            run: th.rord(),
            attempt: th.attempt.n,
            outcome,
        }
    }

    pub fn emit(&mut self, t: ThreadId, outcome: Outcome) -> ProviderEvent {
        let ev = self.current(t, outcome);
        self.ok(run(RunEvent::ProviderTerminal {
            t,
            run: ev.run,
            attempt: ev.attempt,
            outcome,
        }));
        ev
    }

    pub fn end(&mut self, t: ThreadId, outcome: Outcome) -> Vec<Effect> {
        let ev = self.emit(t, outcome);
        self.ok(run(RunEvent::IngestEnd { t, ev }))
    }

    pub fn delegate(&mut self, t: ThreadId, c: ThreadId, mode: DelegateMode) -> Vec<Effect> {
        let access = self.th(t).access;
        self.ok(del(DelEvent::Delegate {
            t,
            child: c,
            mode,
            access,
            reconfigure: true,
        }))
    }

    pub fn notice(&mut self, c: ThreadId, how: How) -> Vec<Effect> {
        self.ok(outbox(OutboxEvent::DoNotice {
            child: c,
            run: 1,
            how,
        }))
    }

    pub fn finish_child(&mut self, c: ThreadId) -> Vec<Effect> {
        self.start(c);
        self.end(c, Outcome::Completed);
        self.notice(c, How::Ack)
    }

    pub fn offers(&self) -> Vec<Message> {
        self.s
            .outbox
            .iter()
            .copied()
            .filter(|m| matches!(m, Message::Offer { .. }))
            .collect()
    }

    pub fn deliver_offer(&mut self, p: ThreadId) -> Vec<Effect> {
        let Some(Message::Offer {
            parent,
            run,
            generation,
        }) = self
            .offers()
            .into_iter()
            .find(|m| matches!(m, Message::Offer { parent, .. } if *parent == p))
        else {
            panic!("no offer for {p:?}");
        };
        self.ok(outbox(OutboxEvent::DoOffer {
            parent,
            run,
            generation,
            how: How::Ack,
        }))
    }
}
