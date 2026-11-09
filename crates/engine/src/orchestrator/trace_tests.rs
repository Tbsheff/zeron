use serde_json::json;

use super::FaultPoint;
use super::faults_tests::{DEV, run_with};
use super::trace::{self, TraceLine, parse_lines};

fn lines() -> Vec<TraceLine> {
    parse_lines(&run_with(None).lines.join("\n")).expect("parse")
}

fn rejects(mut t: Vec<TraceLine>, f: impl FnOnce(&mut Vec<TraceLine>)) -> String {
    f(&mut t);
    trace::check(&t).expect_err("tampered trace must fail").why
}

#[test]
fn trace_rows_follow_the_design_schema() {
    let t = lines();
    let first = &t[0];
    assert_eq!((first.seq, first.act.as_str()), (0, "Init"));
    for (i, l) in t.iter().enumerate() {
        assert_eq!(l.v, 1);
        assert_eq!(l.dev, DEV);
        assert_eq!(l.seq, u64::try_from(i).expect("seq"));
        assert!(l.post.starts_with("sha256:") && l.post.len() == 71);
        assert!(trace::ACTION_NAMES.contains(&l.act.as_str()), "{}", l.act);
        let v = serde_json::to_value(l).expect("json");
        for k in [
            "v", "dev", "seq", "act", "args", "chat", "msg", "cmd", "post",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert!(v.get("time").is_none() && v.get("ts").is_none());
    }
    let notice = t.iter().find(|l| l.act == "DoNotice").expect("notice row");
    assert_eq!(notice.args, json!({"child": 1, "run": 1, "how": "Ack"}));
    assert_eq!(notice.chat.as_deref(), Some("1"));
    let msg = notice.msg.as_ref().expect("msg");
    assert_eq!((msg.id.as_str(), msg.dir.as_str()), ("notice:1:1", "recv"));
    assert_eq!(notice.cmd.as_deref(), Some("notice:1:1"));
}

#[test]
fn checker_accepts_every_slice2_trace() {
    let out = std::env::var_os("ORCH_TRACE_OUT").map(std::path::PathBuf::from);
    let reference = lines();
    let r = trace::check(&reference).expect("reference");
    assert_eq!(r.steps, reference.len());
    if let Some(dir) = &out {
        trace::export_tla(dir, "reference", &reference).expect("export");
        trace::export_jsonl(dir, "reference", &run_with(None).lines).expect("jsonl");
    }
    for at in FaultPoint::ALL {
        for skip in [0, 3, 7] {
            let got = run_with(Some((at, skip)));
            let t = parse_lines(&got.lines.join("\n")).expect("parse");
            trace::check(&t).unwrap_or_else(|e| panic!("{at:?} {skip}: {e}"));
            if let Some(dir) = &out {
                trace::export_tla(dir, &format!("{at:?}_{skip}"), &t).expect("export");
                trace::export_jsonl(dir, &format!("{at:?}_{skip}"), &got.lines).expect("jsonl");
            }
        }
    }
}

#[test]
fn checker_fails_on_invalid_steps() {
    let t = lines();
    let why = rejects(t.clone(), |t| t[5].post = t[4].post.clone());
    assert!(why.contains("post"), "{why}");
    let why = rejects(t.clone(), |t| t.swap(6, 7));
    assert!(why.contains("seq"), "{why}");
    let why = rejects(t.clone(), |t| {
        let mut extra = t[3].clone();
        extra.seq = t.len() as u64;
        t.push(extra);
    });
    assert!(why.contains("twice"), "{why}");
    let why = rejects(t.clone(), |t| t[3].fx.clear());
    assert!(why.contains("effects"), "{why}");
    let why = rejects(t.clone(), |t| t[5].args = json!({"t": 0}));
    assert!(!why.is_empty());
    let why = rejects(t.clone(), |t| {
        t[4].ok = true;
        t[4].reject = None;
    });
    assert!(why.contains("rejects"), "{why}");
    let why = rejects(t.clone(), |t| t[2].dup = true);
    assert!(why.contains("unseen"), "{why}");
    let why = rejects(t, |t| t[9].act = "DoOffer".into());
    assert!(!why.is_empty());
}

#[test]
fn exporter_writes_jsonl_and_a_tla_module() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = run_with(None);
    let jsonl = trace::export_jsonl(dir.path(), DEV, &run.lines).expect("jsonl");
    assert_eq!(jsonl, dir.path().join("orch-trace").join("dev-a.jsonl"));
    let back = parse_lines(&std::fs::read_to_string(&jsonl).expect("read")).expect("parse");
    trace::check(&back).expect("exported trace checks");
    let tla = trace::export_tla(dir.path(), DEV, &back).expect("tla");
    let body = std::fs::read_to_string(&tla).expect("read tla");
    assert!(body.starts_with("---- MODULE OrchTrace_dev_a ----"));
    assert!(body.contains("ASSUME TraceWellFormed"));
    assert_eq!(body.matches("[seq |-> ").count(), back.len());
    assert!(tla.with_extension("cfg").exists());
}

#[test]
fn extension_actions_use_unique_round_trip_trace_names() {
    use zeron_orch::{Event, RunStatus, ThreadId};
    use zeron_orch::{native::NativeEvent, questions::QuestionEvent, remote::RemoteEvent};
    let t = ThreadId(0);
    let cases = [
        Event::Del(zeron_orch::DelEvent::Followup {
            parent: t,
            child: ThreadId(1),
            access: 1,
        }),
        Event::Del(zeron_orch::DelEvent::ReadTaskResult {
            p: t,
            c: ThreadId(1),
            run: 1,
            task_run: 2,
        }),
        Event::Question(QuestionEvent::Raise { child: t, qid: 1 }),
        Event::Question(QuestionEvent::ImportUp {
            child: t,
            qid: 1,
            epoch: 1,
            to: t,
        }),
        Event::Question(QuestionEvent::ImportAnswer {
            child: t,
            qid: 1,
            epoch: 1,
            via: t,
            answer: 1,
        }),
        Event::Question(QuestionEvent::ImportClosed {
            child: t,
            qid: 1,
            epoch: 1,
            to: t,
        }),
        Event::Question(QuestionEvent::ImportRelease {
            child: t,
            qid: 1,
            epoch: 1,
            via: t,
        }),
        Event::Question(QuestionEvent::Start { t }),
        Event::Question(QuestionEvent::Deliver {
            message: zeron_orch::Message::QuestionUp {
                child: t,
                qid: 1,
                epoch: 1,
                to: None,
            },
            how: zeron_orch::How::Ack,
        }),
        Event::Question(QuestionEvent::PersonAnswer {
            child: t,
            qid: 1,
            epoch: 1,
            answer: 1,
        }),
        Event::Question(QuestionEvent::AgentAnswer {
            agent: t,
            child: t,
            qid: 1,
            epoch: 1,
            answer: 1,
        }),
        Event::Native(NativeEvent::Start {
            parent: t,
            key: 1,
            background: false,
        }),
        Event::Native(NativeEvent::End {
            parent: t,
            key: 1,
            failed: false,
        }),
        Event::Native(NativeEvent::Claim { parent: t, key: 1 }),
        Event::Native(NativeEvent::Delivered {
            parent: t,
            key: 1,
            generation: 0,
        }),
        Event::Native(NativeEvent::Recover { parent: t, key: 1 }),
        Event::Remote(RemoteEvent::RemoteImport {
            parent: t,
            child: ThreadId(1),
            ancestor: None,
            depth: 1,
            access: 1,
            cohort: 1,
        }),
        Event::Remote(RemoteEvent::RemoteNotice {
            child: t,
            run: 1,
            status: RunStatus::Completed,
        }),
        Event::Remote(RemoteEvent::RemoteStop { child: t, stop: 1 }),
        Event::Remote(RemoteEvent::RemoteFence { t }),
        Event::Remote(RemoteEvent::RemoteFollowup {
            child: t,
            run: 2,
            cohort: 2,
            access: 1,
        }),
        Event::Remote(RemoteEvent::RemoteClamp {
            child: t,
            access: 1,
        }),
        Event::Remote(RemoteEvent::RemoteCancel { child: t }),
        Event::Remote(RemoteEvent::RemoteSent {
            message: zeron_orch::Message::Notice { child: t, run: 1 },
        }),
    ];
    fn declared(source: &str, name: &str) -> std::collections::BTreeSet<String> {
        source
            .split(&format!("pub enum {name} {{"))
            .nth(1)
            .unwrap()
            .split("\n}")
            .next()
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("    "))
            .filter(|line| line.chars().next().is_some_and(char::is_uppercase))
            .map(|line| {
                line.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    for (kind, source, name) in [
        (
            0,
            include_str!("../../../orch/src/questions.rs"),
            "QuestionEvent",
        ),
        (
            1,
            include_str!("../../../orch/src/native.rs"),
            "NativeEvent",
        ),
        (
            2,
            include_str!("../../../orch/src/remote.rs"),
            "RemoteEvent",
        ),
    ] {
        let tags: std::collections::BTreeSet<String> = cases
            .iter()
            .filter(|event| match event {
                Event::Question(_) => kind == 0,
                Event::Native(_) => kind == 1,
                Event::Remote(_) => kind == 2,
                _ => false,
            })
            .map(|event| {
                serde_json::to_value(event).unwrap()["act"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            tags,
            declared(source, name),
            "every {name} variant needs a trace round-trip fixture"
        );
    }
    let state = zeron_orch::OrchState::new(super::faults_tests::config());
    for event in cases {
        let line = trace::line(trace::Step {
            dev: "fixture",
            seq: 1,
            key: "test",
            cmd: None,
            event: &event,
            dup: false,
            reject: None,
            fx: Vec::new(),
            post: &state,
        })
        .unwrap();
        assert_eq!(line.act, event.name());
        assert!(trace::ACTION_NAMES.contains(&line.act.as_str()));
        assert_eq!(trace::join_event(&line.act, &line.args).unwrap(), event);
    }
}

#[test]
fn relay_edges_keep_the_driver_receipt_key() {
    use zeron_orch::remote::RemoteEvent;
    use zeron_orch::{Event, ThreadId};
    let event = Event::Remote(RemoteEvent::RemoteFence { t: ThreadId(0) });
    let state = zeron_orch::OrchState::new(super::faults_tests::config());
    for (key, id, dir, command) in [
        (
            "remote-send:wire:1",
            "wire:1",
            "send",
            Some("remote-send:wire:1"),
        ),
        (
            "remote-recv:wire:1",
            "wire:1",
            "recv",
            Some("remote-recv:wire:1"),
        ),
        ("remote-sent:wire:1", "remote-sent:wire:1", "recv", None),
    ] {
        let line = trace::line(trace::Step {
            dev: "fixture",
            seq: 1,
            key,
            cmd: None,
            event: &event,
            dup: false,
            reject: None,
            fx: Vec::new(),
            post: &state,
        })
        .unwrap();
        let msg = line.msg.unwrap();
        assert_eq!((msg.id.as_str(), msg.dir.as_str()), (id, dir));
        assert_eq!(line.cmd.as_deref(), command);
    }
}
