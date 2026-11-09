use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use zeron_orch::{Config, Effect, Event, OrchState, Reject, check_invariants, step, well_formed};

use super::DriverError;

pub const TRACE_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceMsg {
    pub id: String,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEffect {
    pub id: String,
    pub effect: Effect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceLine {
    pub v: u8,
    pub dev: String,
    pub seq: u64,
    pub act: String,
    pub args: Value,
    pub chat: Option<String>,
    pub msg: Option<TraceMsg>,
    pub cmd: Option<String>,
    pub ok: bool,
    pub dup: bool,
    pub reject: Option<Reject>,
    pub fx: Vec<TraceEffect>,
    pub post: String,
}

pub fn state_hash(s: &OrchState) -> Result<String, DriverError> {
    let bytes = serde_json::to_vec(s)?;
    let digest = Sha256::digest(&bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

fn split_event(e: &Event) -> Result<(String, Value), DriverError> {
    let v = serde_json::to_value(e)?;
    let act = v
        .get("act")
        .and_then(Value::as_str)
        .ok_or_else(|| DriverError::Corrupt(format!("event without act: {v}")))?
        .to_owned();
    let args = v.get("args").cloned().unwrap_or(Value::Object(Map::new()));
    Ok((act, args))
}

pub fn join_event(act: &str, args: &Value) -> Result<Event, DriverError> {
    let empty = args.as_object().is_some_and(Map::is_empty);
    let v = if empty {
        json!({ "act": act })
    } else {
        json!({ "act": act, "args": args })
    };
    match serde_json::from_value(v.clone()) {
        Ok(e) => Ok(e),
        Err(_) if empty => Ok(serde_json::from_value(json!({ "act": act, "args": {} }))?),
        Err(err) => Err(err.into()),
    }
}

fn chat_of(args: &Value) -> Option<String> {
    ["t", "p", "child", "parent"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_u64))
        .map(|n| n.to_string())
}

pub struct Step<'a> {
    pub dev: &'a str,
    pub seq: u64,
    pub key: &'a str,
    pub cmd: Option<&'a str>,
    pub event: &'a Event,
    pub dup: bool,
    pub reject: Option<Reject>,
    pub fx: Vec<TraceEffect>,
    pub post: &'a OrchState,
}

pub fn line(st: Step<'_>) -> Result<TraceLine, DriverError> {
    let (act, args) = split_event(st.event)?;
    Ok(TraceLine {
        v: TRACE_VERSION,
        dev: st.dev.to_owned(),
        seq: st.seq,
        chat: chat_of(&args),
        act,
        args,
        msg: Some(TraceMsg {
            id: st.key.to_owned(),
            dir: "recv".into(),
        }),
        cmd: st.cmd.map(str::to_owned),
        ok: st.reject.is_none(),
        dup: st.dup,
        reject: st.reject,
        fx: st.fx,
        post: state_hash(st.post)?,
    })
}

pub fn init_line(dev: &str, s: &OrchState) -> Result<TraceLine, DriverError> {
    Ok(TraceLine {
        v: TRACE_VERSION,
        dev: dev.to_owned(),
        seq: 0,
        act: "Init".into(),
        args: json!({ "config": s.config }),
        chat: None,
        msg: None,
        cmd: None,
        ok: true,
        dup: false,
        reject: None,
        fx: Vec::new(),
        post: state_hash(s)?,
    })
}

pub fn parse_lines(text: &str) -> Result<Vec<TraceLine>, DriverError> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(DriverError::from))
        .collect()
}

pub fn export_jsonl(dir: &Path, dev: &str, lines: &[String]) -> Result<PathBuf, DriverError> {
    let out = dir.join("orch-trace");
    std::fs::create_dir_all(&out)?;
    let path = out.join(format!("{dev}.jsonl"));
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(&path, body)?;
    Ok(path)
}

fn tla_ident(dev: &str) -> String {
    dev.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn tla_value(v: &Value) -> String {
    match v {
        Value::Null => "\"null\"".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(tla_value).collect();
            format!("<<{}>>", items.join(", "))
        }
        Value::Object(m) if m.is_empty() => "<<>>".into(),
        Value::Object(m) => {
            let fields: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{} |-> {}", tla_field(k), tla_value(v)))
                .collect();
            format!("[{}]", fields.join(", "))
        }
    }
}

fn tla_field(k: &str) -> String {
    let ok = k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && k.chars().any(|c| c.is_ascii_alphabetic());
    if ok {
        k.to_owned()
    } else {
        format!("f_{}", tla_ident(k))
    }
}

fn tla_step(l: &TraceLine) -> String {
    let fx: Vec<String> = l.fx.iter().map(|f| format!("{:?}", f.id)).collect();
    format!(
        "[seq |-> {}, act |-> {:?}, args |-> {}, ok |-> {}, dup |-> {}, fx |-> <<{}>>, post |-> {:?}]",
        l.seq,
        l.act,
        tla_value(&l.args),
        if l.ok { "TRUE" } else { "FALSE" },
        if l.dup { "TRUE" } else { "FALSE" },
        fx.join(", "),
        l.post,
    )
}

pub const ACTION_NAMES: &[&str] = &[
    "Init",
    "PersonSend",
    "Steer",
    "RestartActive",
    "RequestInterrupt",
    "ServerEnqueue",
    "ProviderTerminal",
    "ChildStart",
    "ChildEnd",
    "ResetPasses",
    "StartFromQueue",
    "UsageLimitRecovery",
    "IngestEnd",
    "RetryAttempt",
    "IngestChild",
    "IngestStale",
    "OpenRoot",
    "Stop",
    "NarrowAccess",
    "InterruptChat",
    "Delegate",
    "ReadResult",
    "WaitReturn",
    "WaitTimeout",
    "WaitAbandon",
    "LateWaitAck",
    "StartFromQueueWake",
    "DoNotice",
    "DoOffer",
    "DoStop",
    "CrashEngine",
];

pub fn tla_module(dev: &str, lines: &[TraceLine]) -> (String, String) {
    let name = format!("OrchTrace_{}", tla_ident(dev));
    let steps: Vec<String> = lines.iter().map(|l| format!("  {}", tla_step(l))).collect();
    let acts: Vec<String> = ACTION_NAMES.iter().map(|a| format!("{a:?}")).collect();
    let body = format!(
        "---- MODULE {name} ----\nEXTENDS Naturals, Sequences\n\nActions == {{{acts}}}\n\nTrace == <<\n{steps}\n>>\n\nTraceWellFormed ==\n  /\\ Len(Trace) > 0\n  /\\ Trace[1].act = \"Init\"\n  /\\ \\A i \\in 1..Len(Trace) : Trace[i].seq = i - 1 /\\ Trace[i].act \\in Actions\n  /\\ \\A i \\in 1..Len(Trace) : (Trace[i].dup \\/ ~Trace[i].ok) => Trace[i].fx = <<>>\n  /\\ \\A i \\in 2..Len(Trace) : (Trace[i].dup \\/ ~Trace[i].ok) => Trace[i].post = Trace[i - 1].post\n\nASSUME TraceWellFormed\n\nVARIABLE i\nInit == i = 1\nNext == i < Len(Trace) /\\ i' = i + 1\nSpec == Init /\\ [][Next]_i\n====\n",
        acts = acts.join(", "),
        steps = steps.join(",\n"),
    );
    (name, body)
}

pub fn export_tla(dir: &Path, dev: &str, lines: &[TraceLine]) -> Result<PathBuf, DriverError> {
    let out = dir.join("orch-trace");
    std::fs::create_dir_all(&out)?;
    let (name, body) = tla_module(dev, lines);
    std::fs::write(
        out.join(format!("{name}.cfg")),
        "SPECIFICATION Spec\nCHECK_DEADLOCK FALSE\n",
    )?;
    let path = out.join(format!("{name}.tla"));
    std::fs::write(&path, body)?;
    Ok(path)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub steps: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub duplicates: usize,
    pub acts: BTreeMap<String, usize>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("trace seq {seq}: {why}")]
pub struct TraceError {
    pub seq: u64,
    pub why: String,
}

fn bad(seq: u64, why: impl Into<String>) -> TraceError {
    TraceError {
        seq,
        why: why.into(),
    }
}

struct Replay {
    dev: String,
    state: OrchState,
    keys: BTreeSet<String>,
    next_seq: u64,
    report: CheckReport,
}

impl Replay {
    fn start(first: &TraceLine) -> Result<Self, TraceError> {
        let seq = first.seq;
        if first.act != "Init" || seq != 0 {
            return Err(bad(seq, "trace must start with Init at seq 0"));
        }
        let config: Config = first
            .args
            .get("config")
            .cloned()
            .ok_or_else(|| bad(seq, "Init without config"))
            .and_then(|c| serde_json::from_value(c).map_err(|e| bad(seq, e.to_string())))?;
        let state = OrchState::new(config);
        let mut r = Replay {
            dev: first.dev.clone(),
            state,
            keys: BTreeSet::new(),
            next_seq: 1,
            report: CheckReport::default(),
        };
        r.expect_post(first)?;
        r.count(first);
        Ok(r)
    }

    fn count(&mut self, l: &TraceLine) {
        self.report.steps += 1;
        *self.report.acts.entry(l.act.clone()).or_default() += 1;
    }

    fn expect_post(&self, l: &TraceLine) -> Result<(), TraceError> {
        let h = state_hash(&self.state).map_err(|e| bad(l.seq, e.to_string()))?;
        if h != l.post {
            return Err(bad(l.seq, format!("post {} but replay gives {h}", l.post)));
        }
        well_formed(&self.state).map_err(|m| bad(l.seq, format!("malformed {m:?}")))?;
        check_invariants(&self.state).map_err(|v| bad(l.seq, format!("violates {v:?}")))
    }

    fn header(&self, l: &TraceLine) -> Result<String, TraceError> {
        if l.v != TRACE_VERSION {
            return Err(bad(l.seq, format!("version {}", l.v)));
        }
        if l.dev != self.dev {
            return Err(bad(
                l.seq,
                format!("device {} in a trace of {}", l.dev, self.dev),
            ));
        }
        if l.seq != self.next_seq {
            return Err(bad(l.seq, format!("expected seq {}", self.next_seq)));
        }
        let key = l.msg.as_ref().map(|m| m.id.clone());
        key.ok_or_else(|| bad(l.seq, "step without msg id"))
    }

    fn duplicate(&mut self, l: &TraceLine, key: &str) -> Result<(), TraceError> {
        if !self.keys.contains(key) {
            return Err(bad(l.seq, format!("duplicate of unseen {key}")));
        }
        if !l.fx.is_empty() {
            return Err(bad(l.seq, "duplicate with effects"));
        }
        self.report.duplicates += 1;
        Ok(())
    }

    fn fresh(&mut self, l: &TraceLine, key: &str) -> Result<(), TraceError> {
        if !self.keys.insert(key.to_owned()) {
            return Err(bad(l.seq, format!("{key} applied twice without a receipt")));
        }
        let event = join_event(&l.act, &l.args).map_err(|e| bad(l.seq, e.to_string()))?;
        match step(&self.state, event) {
            Ok((next, fx)) => {
                self.expect_effects(l, key, &fx)?;
                if !l.ok {
                    return Err(bad(l.seq, "trace says rejected; step accepts"));
                }
                self.state = next;
                self.report.accepted += 1;
            }
            Err(r) => {
                if l.ok || l.reject != Some(r) || !l.fx.is_empty() {
                    return Err(bad(
                        l.seq,
                        format!("step rejects with {r:?}; trace says {:?}", l.reject),
                    ));
                }
                self.report.rejected += 1;
            }
        }
        Ok(())
    }

    fn expect_effects(&self, l: &TraceLine, key: &str, fx: &[Effect]) -> Result<(), TraceError> {
        let want: Vec<TraceEffect> = fx
            .iter()
            .enumerate()
            .map(|(i, e)| TraceEffect {
                id: super::driver::effect_id(key, i),
                effect: e.clone(),
            })
            .collect();
        if want != l.fx {
            return Err(bad(l.seq, "effects differ from replay"));
        }
        Ok(())
    }

    fn feed(&mut self, l: &TraceLine) -> Result<(), TraceError> {
        let key = self.header(l)?;
        if l.dup {
            self.duplicate(l, &key)?;
        } else {
            self.fresh(l, &key)?;
        }
        self.expect_post(l)?;
        self.count(l);
        self.next_seq += 1;
        Ok(())
    }
}

pub fn check(lines: &[TraceLine]) -> Result<CheckReport, TraceError> {
    let (first, rest) = lines.split_first().ok_or_else(|| bad(0, "empty trace"))?;
    let mut r = Replay::start(first)?;
    for l in rest {
        r.feed(l)?;
    }
    Ok(r.report)
}
