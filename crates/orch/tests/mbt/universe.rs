use serde_json::{Value, json};

use super::fixture::{Action, Fixture, Module};

const DEL_ONE: [&str; 13] = [
    "PersonSend",
    "Stop",
    "NarrowAccess",
    "InterruptChat",
    "WaitReturn",
    "WaitTimeout",
    "WaitAbandon",
    "LateWaitAck",
    "StartFromQueue",
    "StartFromQueueWake",
    "RootComplete",
    "RootFail",
    "ConfirmInterrupt",
];
const RL_ONE: [&str; 8] = [
    "Steer",
    "RequestInterrupt",
    "ServerEnqueue",
    "ChildStart",
    "ChildEnd",
    "ResetPasses",
    "StartFromQueue",
    "UsageLimitRecovery",
];
const MODES: [&str; 4] = ["steer", "restart", "queue", "now"];
const OUTCOMES: [&str; 6] = [
    "completed",
    "failed",
    "interrupted",
    "usage_limit",
    "transient",
    "crash",
];
const INGEST: [&str; 4] = ["IngestEnd", "RetryAttempt", "IngestChild", "IngestStale"];

fn act(name: &str, params: Vec<Value>) -> Action {
    Action {
        name: name.to_string(),
        params,
    }
}

pub fn candidates(fx: &Fixture) -> Vec<Action> {
    let mut out = fx.actions.clone();
    let extra = match fx.module {
        Module::RunLifecycle => rl_extra(fx),
        Module::Delegation => del_extra(fx),
    };
    for a in extra {
        if !out.iter().any(|b| b.name == a.name && b.params == a.params) {
            out.push(a);
        }
    }
    out
}

fn names(fx: &Fixture) -> Vec<String> {
    fx.threads.iter().map(|(n, _)| n.clone()).collect()
}

fn rl_extra(fx: &Fixture) -> Vec<Action> {
    let mut out = Vec::new();
    for t in names(fx) {
        out.extend(RL_ONE.iter().map(|n| act(n, vec![json!(t)])));
        for m in MODES {
            out.push(act("PersonSend", vec![json!(t), json!(m)]));
            out.push(act("RestartActive", vec![json!(t), json!(m)]));
        }
        out.extend(
            OUTCOMES
                .iter()
                .map(|o| act("ProviderTerminal", vec![json!(t), json!(o)])),
        );
    }
    let events: Vec<Vec<Value>> = fx
        .actions
        .iter()
        .filter(|a| INGEST.contains(&a.name.as_str()))
        .map(|a| a.params.clone())
        .collect();
    for p in events {
        out.extend(INGEST.iter().map(|n| act(n, p.clone())));
    }
    out
}

fn del_extra(fx: &Fixture) -> Vec<Action> {
    let all = names(fx);
    let mut out = Vec::new();
    for t in &all {
        out.extend(DEL_ONE.iter().map(|n| act(n, vec![json!(t)])));
        for c in &all {
            out.push(act("ReadResult", vec![json!(t), json!(c)]));
            for m in ["wait", "async"] {
                out.extend(
                    (0..=1).map(|a| act("Delegate", vec![json!(t), json!(c), json!(m), json!(a)])),
                );
            }
        }
    }
    out
}
