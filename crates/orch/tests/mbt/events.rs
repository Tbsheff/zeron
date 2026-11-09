use serde_json::{Map, Value};
use zeron_orch::{
    DelEvent, DelegateMode, Event, How, Message, Mode, Node, OrchState, OutboxEvent, Outcome,
    ProviderEvent, RunEvent, ThreadId,
};

use super::fixture::{Action, Fixture, Module};

pub struct Ctx<'a> {
    pub fx: &'a Fixture,
    pub s: &'a OrchState,
    pub msg: u64,
}

impl Ctx<'_> {
    fn t(&self, a: &Action, i: usize) -> ThreadId {
        self.fx
            .id_of(a.params.get(i).and_then(Value::as_str).unwrap_or_default())
    }

    fn rord(&self, t: ThreadId) -> u32 {
        self.s.threads.get(&t).map_or(0, |t| t.rord())
    }

    fn att(&self, t: ThreadId) -> u32 {
        self.s.threads.get(&t).map_or(0, |t| t.attempt.n)
    }
}

pub fn events(ctx: &Ctx, a: &Action) -> Vec<Event> {
    match ctx.fx.module {
        Module::RunLifecycle => rl_events(ctx, a),
        Module::Delegation => del_events(ctx, a),
    }
}

fn rl_events(ctx: &Ctx, a: &Action) -> Vec<Event> {
    let t = ctx.t(a, 0);
    let p1 = || a.params.get(1).cloned().unwrap_or(Value::Null);
    let msg = ctx.msg;
    let ev = match a.name.as_str() {
        "PersonSend" => RunEvent::PersonSend {
            t,
            mode: mode(&p1()),
            msg,
        },
        "RestartActive" => RunEvent::RestartActive {
            t,
            mode: mode(&p1()),
            msg,
        },
        "Steer" => RunEvent::Steer { t, msg },
        "RequestInterrupt" => RunEvent::RequestInterrupt { t },
        "ServerEnqueue" => RunEvent::ServerEnqueue { t, msg },
        "ProviderTerminal" => RunEvent::ProviderTerminal {
            t,
            run: ctx.rord(t),
            attempt: ctx.att(t),
            outcome: outcome(&p1()),
        },
        "ChildStart" => RunEvent::ChildStart { t },
        "ChildEnd" => RunEvent::ChildEnd { t },
        "ResetPasses" => RunEvent::ResetPasses {
            t,
            run: armed(ctx, t),
        },
        "StartFromQueue" => RunEvent::StartFromQueue { t },
        "UsageLimitRecovery" => RunEvent::UsageLimitRecovery { t },
        _ => return ingest_events(t, a),
    };
    vec![Event::Run(ev)]
}

fn armed(ctx: &Ctx, t: ThreadId) -> u32 {
    ctx.s.threads.get(&t).and_then(|t| t.ul.armed).unwrap_or(0)
}

fn ingest_events(t: ThreadId, a: &Action) -> Vec<Event> {
    let ev = provider_event(a.params.get(1).and_then(Value::as_object));
    let e = match a.name.as_str() {
        "IngestEnd" => RunEvent::IngestEnd { t, ev },
        "RetryAttempt" => RunEvent::RetryAttempt { t, ev },
        "IngestChild" => RunEvent::IngestChild { t, ev },
        "IngestStale" => RunEvent::IngestStale { t, ev },
        other => panic!("unmapped RunLifecycle action {other}"),
    };
    vec![Event::Run(e)]
}

fn provider_event(m: Option<&Map<String, Value>>) -> ProviderEvent {
    let m = m.cloned().unwrap_or_default();
    let n = |k: &str| u32::try_from(m.get(k).and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    let node = if m.get("n").and_then(Value::as_str) == Some("child") {
        Node::Child
    } else {
        Node::Root
    };
    ProviderEvent {
        node,
        run: n("r"),
        attempt: n("a"),
        outcome: outcome(m.get("o").unwrap_or(&Value::Null)),
    }
}

fn mode(v: &Value) -> Mode {
    match v.as_str() {
        Some("steer") => Mode::Steer,
        Some("restart") => Mode::Restart,
        Some("now") => Mode::Now,
        _ => Mode::Queue,
    }
}

fn outcome(v: &Value) -> Outcome {
    match v.as_str() {
        Some("completed") => Outcome::Completed,
        Some("interrupted") => Outcome::Interrupted,
        Some("usage_limit") => Outcome::UsageLimit,
        Some("transient") => Outcome::Transient,
        Some("crash") => Outcome::Crash,
        _ => Outcome::Failed,
    }
}

fn del_events(ctx: &Ctx, a: &Action) -> Vec<Event> {
    let t = ctx.t(a, 0);
    let ev = match a.name.as_str() {
        "PersonSend" => Event::Run(RunEvent::PersonSend {
            t,
            mode: Mode::Queue,
            msg: ctx.msg,
        }),
        "StartFromQueue" => Event::Run(RunEvent::StartFromQueue { t }),
        "RootComplete" => return end_events(ctx, t, Outcome::Completed),
        "RootFail" => return end_events(ctx, t, Outcome::Failed),
        "ConfirmInterrupt" => return end_events(ctx, t, Outcome::Interrupted),
        "Delegate" => Event::Del(delegate(ctx, a, t)),
        "ReadResult" => Event::Del(DelEvent::ReadResult {
            p: t,
            c: ctx.t(a, 1),
            run: ctx.rord(t),
        }),
        _ => return del_simple(ctx, a, t),
    };
    vec![ev]
}

fn del_simple(ctx: &Ctx, a: &Action, t: ThreadId) -> Vec<Event> {
    let access = ctx
        .s
        .threads
        .get(&t)
        .map_or(0, |t| t.access)
        .saturating_sub(1);
    let ev = match a.name.as_str() {
        "Stop" => DelEvent::Stop { t },
        "NarrowAccess" => DelEvent::NarrowAccess { t, access },
        "InterruptChat" => DelEvent::InterruptChat { t },
        "WaitReturn" => DelEvent::WaitReturn { t },
        "WaitTimeout" => DelEvent::WaitTimeout { t },
        "WaitAbandon" => DelEvent::WaitAbandon { t },
        "LateWaitAck" => DelEvent::LateWaitAck { t },
        "StartFromQueueWake" => DelEvent::StartFromQueueWake { t },
        _ => return vec![Event::Outbox(outbox_event(ctx, a))],
    };
    vec![Event::Del(ev)]
}

fn delegate(ctx: &Ctx, a: &Action, t: ThreadId) -> DelEvent {
    let mode = if a.params.get(2).and_then(Value::as_str) == Some("wait") {
        DelegateMode::Wait
    } else {
        DelegateMode::Async
    };
    let access = u8::try_from(a.params.get(3).and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    DelEvent::Delegate {
        t,
        child: ctx.t(a, 1),
        mode,
        access,
        reconfigure: true,
    }
}

fn end_events(ctx: &Ctx, t: ThreadId, outcome: Outcome) -> Vec<Event> {
    let (run, attempt) = (ctx.rord(t), ctx.att(t));
    let ev = ProviderEvent {
        node: Node::Root,
        run,
        attempt,
        outcome,
    };
    vec![
        Event::Run(RunEvent::ProviderTerminal {
            t,
            run,
            attempt,
            outcome,
        }),
        Event::Run(RunEvent::IngestEnd { t, ev }),
    ]
}

fn outbox_event(ctx: &Ctx, a: &Action) -> OutboxEvent {
    if a.name == "CrashEngine" {
        return OutboxEvent::CrashEngine;
    }
    let m = spec_message(ctx.fx, a.params.first().and_then(Value::as_object));
    let how = if a.params.get(1).and_then(Value::as_str) == Some("ack") {
        How::Ack
    } else {
        How::Keep
    };
    match (a.name.as_str(), m) {
        ("DoNotice", Message::Notice { child, run }) => OutboxEvent::DoNotice { child, run, how },
        (
            "DoOffer",
            Message::Offer {
                parent,
                run,
                generation,
            },
        ) => OutboxEvent::DoOffer {
            parent,
            run,
            generation,
            how,
        },
        ("DoStop", Message::Stop { child, stop }) => OutboxEvent::DoStop { child, stop, how },
        (other, _) => panic!("unmapped Delegation action {other}"),
    }
}

fn spec_message(fx: &Fixture, m: Option<&Map<String, Value>>) -> Message {
    let m = m.cloned().unwrap_or_default();
    let n = |k: &str| u32::try_from(m.get(k).and_then(Value::as_u64).unwrap_or(0)).unwrap_or(0);
    let t = fx.id_of(m.get("t").and_then(Value::as_str).unwrap_or_default());
    match m.get("k").and_then(Value::as_str) {
        Some("notice") => Message::Notice {
            child: t,
            run: n("r"),
        },
        Some("offer") => Message::Offer {
            parent: t,
            run: n("r"),
            generation: n("g"),
        },
        _ => Message::Stop {
            child: t,
            stop: n("r"),
        },
    }
}

pub fn exempt(fx: &Fixture, spec: &Map<String, Value>, a: &Action) -> bool {
    let left = |k: &str| spec.get(k).and_then(Value::as_i64) == Some(0);
    let p = |i: usize| {
        a.params
            .get(i)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let runs_full = || spec["rord"][p(0)].as_i64() >= Some(fx.constant("MaxRuns"));
    match a.name.as_str() {
        "PersonSend" | "Steer" | "RestartActive" => left("personLeft"),
        "RequestInterrupt" | "Stop" => left("stopLeft"),
        "ServerEnqueue" => left("serverLeft"),
        "ProviderTerminal" => {
            (p(1) == "usage_limit" && left("ulLeft")) || (p(1) == "crash" && left("crashLeft"))
        }
        "StartFromQueue" | "StartFromQueueWake" | "UsageLimitRecovery" => runs_full(),
        "NarrowAccess" => left("narrowLeft"),
        "InterruptChat" => left("interruptLeft"),
        "WaitAbandon" => left("abandonLeft"),
        "RootFail" => left("failLeft"),
        "CrashEngine" => left("crashLeft"),
        "DoNotice" | "DoOffer" | "DoStop" => replay_exempt(fx, spec, a),
        _ => false,
    }
}

fn replay_exempt(fx: &Fixture, spec: &Map<String, Value>, a: &Action) -> bool {
    let how = a.params.get(1).and_then(Value::as_str).unwrap_or_default();
    let crash_left = spec.get("crashLeft").and_then(Value::as_i64).unwrap_or(0);
    match how {
        "crash" => !fx.flag("Crash") || crash_left == 0,
        "dup" => !fx.flag("DupDelivery") || a.name != "DoNotice",
        _ => false,
    }
}
