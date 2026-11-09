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
    }
    for at in FaultPoint::ALL {
        for skip in [0, 3, 7] {
            let got = run_with(Some((at, skip)));
            let t = parse_lines(&got.lines.join("\n")).expect("parse");
            trace::check(&t).unwrap_or_else(|e| panic!("{at:?} {skip}: {e}"));
            if let Some(dir) = &out {
                trace::export_tla(dir, &format!("{at:?}_{skip}"), &t).expect("export");
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
