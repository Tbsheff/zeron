use serde_json::{Map, Value, json};
use zeron_orch::{
    CohortSt, Delivery, Disp, Hold, Message, Msg, Node, OrchState, Outcome, Reason, RunKind,
    RunStatus, TaskSt, Thread, ThreadId, WakeRule,
};

use super::fixture::{Fixture, Module};

pub fn abstract_state(fx: &Fixture, s: &OrchState) -> Map<String, Value> {
    let mut out = Map::new();
    for var in &fx.vars {
        if let Some(v) = abstract_var(fx, s, var) {
            out.insert(var.clone(), canon(v));
        }
    }
    out
}

fn abstract_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    if let Some(v) = base_var(fx, s, var) {
        return Some(v);
    }
    match fx.module {
        Module::RunLifecycle => rl_var(fx, s, var),
        Module::Delegation => del_var(fx, s, var),
    }
}

fn per_thread(fx: &Fixture, s: &OrchState, f: impl Fn(Option<&Thread>) -> Value) -> Value {
    let mut m = Map::new();
    for (name, id) in fx.threads.iter() {
        m.insert(name.clone(), f(s.threads.get(id)));
    }
    Value::Object(m)
}

fn per_child(fx: &Fixture, s: &OrchState, f: impl Fn(Option<&Thread>) -> Value) -> Value {
    let mut m = Map::new();
    for (name, id) in fx.threads.iter().skip(1) {
        m.insert(name.clone(), f(s.threads.get(id)));
    }
    Value::Object(m)
}

fn base_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    let v = match var {
        "queue" => per_thread(fx, s, |t| {
            t.map_or(json!([]), |t| t.queue.iter().map(msg).collect())
        }),
        "held" => per_thread(fx, s, |t| json!(hold(t.map_or(Hold::None, |t| t.held)))),
        "liveSet" => per_thread(fx, s, |t| set(t.map_or(Vec::new(), live_set))),
        "rord" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.rord()))),
        "intReq" => per_thread(fx, s, |t| json!(t.is_some_and(|t| t.int_req))),
        _ => return None,
    };
    Some(v)
}

fn rl_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    let v = match var {
        "att" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.attempt.n))),
        "why" => per_thread(fx, s, |t| {
            json!(reason(t.map_or(Reason::Initial, |t| t.attempt.reason)))
        }),
        "emitted" => per_thread(fx, s, |t| json!(t.is_some_and(|t| t.attempt.emitted))),
        "kidOpen" => per_thread(fx, s, |t| json!(t.is_some_and(|t| t.attempt.kid_open))),
        "pev" => per_thread(fx, s, |t| {
            set(t.map_or(Vec::new(), |t| t.inbox.iter().map(event).collect()))
        }),
        "retries" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.attempt.retries))),
        "steers" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.attempt.steers.min(2)))),
        "lastOut" => per_thread(fx, s, |t| {
            json!(t.and_then(Thread::last_out).map_or("none", status))
        }),
        "rkind" => per_thread(fx, s, |t| {
            json!(t.and_then(Thread::latest_kind).map_or("none", kind))
        }),
        "ulArmed" => per_thread(fx, s, |t| json!(t.and_then(|t| t.ul.armed).unwrap_or(0))),
        "ulDue" => per_thread(fx, s, |t| json!(t.is_some_and(|t| t.ul.due))),
        _ => return None,
    };
    Some(v)
}

fn del_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    if let Some(v) = del_lineage_var(fx, s, var) {
        return Some(v);
    }
    if let Some(v) = del_task_var(fx, s, var) {
        return Some(v);
    }
    del_cohort_var(fx, s, var)
}

fn del_lineage_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    let name = |id: Option<ThreadId>| json!(fx.name_of(id));
    let v = match var {
        "created" => per_thread(fx, s, |t| json!(t.is_some())),
        "parent" => per_thread(fx, s, |t| name(t.and_then(|t| t.parent))),
        "depth" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.depth))),
        "access" => per_thread(fx, s, |t| json!(t.map_or(0, |t| t.access))),
        "rtask" => per_thread(fx, s, |t| json!(t.is_some_and(Thread::is_task_run))),
        "waitOn" => per_thread(fx, s, |t| name(t.and_then(|t| t.wait_on))),
        "zWait" => per_thread(fx, s, |t| name(t.and_then(|t| t.late_wait))),
        "outbox" => set(s.outbox.iter().map(|m| message(fx, *m)).collect()),
        "rcpt" => set(s
            .stop_receipts
            .iter()
            .map(|(c, n)| json!([fx.name_of(Some(*c)), n]))
            .collect()),
        _ => return None,
    };
    Some(v)
}

fn del_task_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    let v = match var {
        "tSt" => per_child(fx, s, |t| {
            json!(t.and_then(|t| t.task).map_or("none", |k| task_st(k.st)))
        }),
        "tWake" => per_child(fx, s, |t| {
            json!(t.and_then(|t| t.task).map_or("always", |k| wake(k.wake)))
        }),
        "tDel" => per_child(fx, s, |t| {
            json!(
                t.and_then(|t| t.task)
                    .map_or("pending", |k| delivery(k.delivery))
            )
        }),
        "tCoh" => per_child(fx, s, |t| {
            json!(t.and_then(|t| t.task).map_or(0, |k| k.cohort))
        }),
        "pushes" => per_child(fx, s, |t| {
            json!(t.and_then(|t| t.task).map_or(0, |k| k.pushes))
        }),
        _ => return None,
    };
    Some(v)
}

fn del_cohort_var(fx: &Fixture, s: &OrchState, var: &str) -> Option<Value> {
    let runs = fx.max_runs();
    let cohorts = |t: Option<&Thread>, f: &dyn Fn(&zeron_orch::Cohort) -> Value| -> Value {
        let default = zeron_orch::Cohort::default();
        (1..=runs)
            .map(|r| f(t.and_then(|t| t.cohort(r)).unwrap_or(&default)))
            .collect()
    };
    let v = match var {
        "cDisp" => per_thread(fx, s, |t| cohorts(t, &|c| json!(disp(c.disp)))),
        "cSt" => per_thread(fx, s, |t| cohorts(t, &|c| json!(cohort_st(c.st)))),
        "cGen" => per_thread(fx, s, |t| cohorts(t, &|c| json!(c.generation))),
        "cDel" => per_thread(fx, s, |t| {
            cohorts(t, &|c| {
                set(c.del.iter().map(|d| json!(fx.name_of(Some(*d)))).collect())
            })
        }),
        _ => return None,
    };
    Some(v)
}

fn set(items: Vec<Value>) -> Value {
    json!({ "set": items })
}

pub fn canon(v: Value) -> Value {
    match v {
        Value::Object(m) if m.len() == 1 && m.contains_key("set") => {
            let mut items: Vec<Value> = match m.into_iter().next() {
                Some((_, Value::Array(a))) => a.into_iter().map(canon).collect(),
                _ => Vec::new(),
            };
            items.sort_by_key(|x| x.to_string());
            json!({ "set": items })
        }
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, canon(v))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(canon).collect()),
        other => other,
    }
}

fn live_set(t: &Thread) -> Vec<Value> {
    t.runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status == RunStatus::Live)
        .map(|(i, _)| json!(i + 1))
        .collect()
}

fn msg(m: &Msg) -> Value {
    match m {
        Msg::Person(_) => json!({"k": "person", "r": 0, "g": 0}),
        Msg::Server(_) => json!({"k": "server", "r": 0, "g": 0}),
        Msg::Task => json!({"k": "task", "r": 0, "g": 0}),
        Msg::Wake { run, generation } => json!({"k": "wake", "r": run, "g": generation}),
    }
}

fn message(fx: &Fixture, m: Message) -> Value {
    match m {
        Message::Notice { child, run } => {
            json!({"k": "notice", "t": fx.name_of(Some(child)), "r": run, "g": 0})
        }
        Message::Offer {
            parent,
            run,
            generation,
        } => {
            json!({"k": "offer", "t": fx.name_of(Some(parent)), "r": run, "g": generation})
        }
        Message::Stop { child, stop } => {
            json!({"k": "stop", "t": fx.name_of(Some(child)), "r": stop, "g": 0})
        }
    }
}

fn event(e: &zeron_orch::ProviderEvent) -> Value {
    let n = if e.node == Node::Root {
        "root"
    } else {
        "child"
    };
    json!({"n": n, "r": e.run, "a": e.attempt, "o": outcome(e.outcome)})
}

pub fn outcome(o: Outcome) -> &'static str {
    match o {
        Outcome::Completed => "completed",
        Outcome::Failed => "failed",
        Outcome::Interrupted => "interrupted",
        Outcome::UsageLimit => "usage_limit",
        Outcome::Transient => "transient",
        Outcome::Crash => "crash",
    }
}

fn hold(h: Hold) -> &'static str {
    match h {
        Hold::None => "none",
        Hold::Stop => "stop",
        Hold::Fail => "fail",
        Hold::Limit => "limit",
    }
}

fn reason(r: Reason) -> &'static str {
    match r {
        Reason::Initial => "initial",
        Reason::SteeringRestart => "steering_restart",
        Reason::Retry => "retry",
        Reason::ProviderRecovery => "provider_recovery",
        Reason::AccessRestart => "access_restart",
    }
}

fn status(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Live => "live",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
        RunStatus::UsageLimit => "usage_limit",
    }
}

fn kind(k: RunKind) -> &'static str {
    match k {
        RunKind::Person => "person",
        RunKind::Server => "server",
        RunKind::Task => "task",
        RunKind::Wake => "wake",
        RunKind::Resume => "resume",
    }
}

fn task_st(s: TaskSt) -> &'static str {
    match s {
        TaskSt::Running => "running",
        TaskSt::Settled => "settled",
    }
}

fn wake(w: WakeRule) -> &'static str {
    match w {
        WakeRule::Always => "always",
        WakeRule::SettledOnly => "settled_only",
    }
}

fn delivery(d: Delivery) -> &'static str {
    match d {
        Delivery::Pending => "pending",
        Delivery::Claimed => "claimed",
        Delivery::Delivered => "delivered",
        Delivery::Acknowledged => "acknowledged",
        Delivery::Disposed => "disposed",
    }
}

fn disp(d: Disp) -> &'static str {
    match d {
        Disp::Open => "open",
        Disp::Stopped => "stopped",
    }
}

fn cohort_st(s: CohortSt) -> &'static str {
    match s {
        CohortSt::None => "none",
        CohortSt::Offered => "offered",
        CohortSt::Queued => "queued",
    }
}
