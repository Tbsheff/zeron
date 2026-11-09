use super::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct Provider {
    seq: AtomicU64,
    calls: AtomicUsize,
    fail: bool,
    append: bool,
    durable: Option<Arc<DocsStore>>,
}

#[async_trait::async_trait]
impl RunEngine for Provider {
    async fn start(&self, _: &Pending) -> Result<String, EngineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(store) = &self.durable {
            store.transact(|tx| {
                tx.execute("UPDATE synthetic_provider SET calls=calls+1,seq=seq+1", [])?;
                Ok::<_, DriverError>(())
            }).unwrap();
        }
        if self.append { self.seq.fetch_add(1, Ordering::SeqCst); }
        if self.fail { Err(EngineError::Other("private provider error".into())) }
        else { Ok("provider-return-7".into()) }
    }
    async fn steer(&self, _: &Pending) -> Result<Ran, EngineError> { Ok(Ran::Nothing) }
    async fn stop_attempt(&self, _: &str) -> bool { false }
    async fn interrupt_run(&self, _: &str) -> bool { false }
    fn owns_run(&self, _: &str) -> bool {
        self.durable.as_ref().is_some_and(|store| store.transact(|tx| {
            Ok::<_, DriverError>(tx.query_row("SELECT mode=1 FROM synthetic_provider", [], |r| r.get::<_, bool>(0))?)
        }).unwrap())
    }
    fn last_done(&self, _: &str) -> Option<(u64, DoneStatus, Option<zeron_proto::FailureClass>)> {
        self.durable.as_ref().and_then(|store| store.transact(|tx| {
            let (seq, mode): (u64, u8) = tx.query_row("SELECT seq,mode FROM synthetic_provider", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok::<_, DriverError>((mode == 2).then_some((seq, DoneStatus::Completed, None)))
        }).unwrap())
    }
    fn journal_seq(&self, _: &str) -> u64 {
        self.durable.as_ref().map_or_else(|| self.seq.load(Ordering::SeqCst), |store| store.transact(|tx| {
            Ok::<_, DriverError>(tx.query_row("SELECT seq FROM synthetic_provider", [], |r| r.get(0))?)
        }).unwrap())
    }
    fn effective_access(&self, _: &str) -> Option<u8> { Some(1) }
    fn create_child(&self, _: &str, _: &ChildSpec) -> Result<(), EngineError> { Ok(()) }
    fn result_text(&self, _: &str) -> Option<String> { None }
    fn reports_mcp_results(&self, _: &str) -> bool { true }
    fn task_results(&self, _: &str, _: &str, _: Vec<zeron_doc::TaskResultPart>) -> Result<(), EngineError> { Ok(()) }
    fn kick_drain(&self, _: &str) {}
}

fn fixture(fail: bool, append: bool, capture: bool) -> (tempfile::TempDir, OrchWire, Provider, Pending, AttemptId) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let driver = Driver::open_with_capture(store.clone(), "mechanism-test", Config::default(), capture).unwrap();
    let wire = OrchWire::open(store, "mechanism-test", OrchOptions::default()).unwrap();
    let mut wire = Arc::try_unwrap(wire).ok().unwrap();
    wire.driver = driver;
    let t = wire.thread("synthetic-chat").unwrap();
    wire.driver.apply_command("fixture-queue", None, Event::Run(RunEvent::PersonSend { t, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("fixture-start", None, Event::Run(RunEvent::StartFromQueue { t })).unwrap();
    let at = wire.driver.state().unwrap().thread(t).unwrap().attempt_id();
    let p = Pending { chat: "synthetic-chat".into(), message_id: "synthetic-msg".into(),
        via: Via::Resume { prompt: "private prompt".into(), worktree: None } };
    let engine = Provider { seq: AtomicU64::new(4), calls: AtomicUsize::new(0), fail, append, durable: None };
    (dir, wire, engine, p, at)
}

fn export(wire: &OrchWire, name: &str) {
    if let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") {
        let dir = std::path::PathBuf::from(root).join(format!("provider-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rows = wire.driver.mechanism_lines().unwrap();
        std::fs::write(dir.join("mechanism-test.jsonl"), rows.join("\n") + "\n").unwrap();
        std::fs::write(dir.join("capture.json"), serde_json::to_vec(&serde_json::json!({
            "test_name":std::thread::current().name().expect("test thread name"),
            "devices":["mechanism-test"], "filenames":["mechanism-test.jsonl"], "capture":"present"
        })).unwrap()).unwrap();
    }
}

fn facts(wire: &OrchWire) -> Vec<serde_json::Value> {
    let line = wire.driver.mechanism_lines().unwrap().pop().unwrap();
    let row: serde_json::Value = serde_json::from_str(&line).unwrap();
    row["snapshot"]["facts"].as_array().unwrap().clone()
}

#[tokio::test]
async fn provider_mechanism_records_success_and_replay_without_second_start() {
    let (_dir, wire, engine, p, at) = fixture(false, true, true);
    assert!(matches!(wire.start(&engine, "fixture-start#0", at, p.clone(), false).await.unwrap(), Ran::Started(_)));
    wire.start(&engine, "fixture-start#0", at, p.clone(), true).await.unwrap();
    engine.seq.store(8, Ordering::SeqCst);
    wire.start(&engine, "fixture-start#0", at, p, true).await.unwrap();
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    export(&wire, "capture");
    let facts = facts(&wire);
    let started = facts.iter().find(|f| f["data"]["outcome"] == "engine_start_ok").unwrap();
    assert_eq!(started["data"]["provider_return_id"], "provider-return-7");
    assert_eq!(started["data"]["effective_access"], 1);
    assert_eq!(started["data"]["journal_seq_before"], 4);
    assert_eq!(started["data"]["journal_seq_after"], 5);
    assert_eq!(facts.iter().filter(|f| f["kind"] == "journal_observed").count(), 1);
    assert_eq!(facts.iter().filter(|f| f["data"]["outcome"] == "already_done").count(), 2);
}

#[tokio::test]
async fn provider_mechanism_records_failure_without_error_or_invented_journal() {
    let (_dir, wire, engine, p, at) = fixture(true, false, true);
    assert!(wire.start(&engine, "fixture-start#0", at, p, false).await.is_err());
    export(&wire, "capture");
    let facts = facts(&wire);
    let failed = facts.iter().find(|f| f["data"]["outcome"] == "failed").unwrap();
    assert!(failed["data"]["provider_return_id"].is_null());
    assert!(failed["data"]["effective_access"].is_null());
    assert_eq!(failed["data"]["journal_change"], "unchanged");
    assert!(!facts.iter().any(|f| f["kind"] == "journal_observed"));
    let encoded = serde_json::to_string(&facts).unwrap();
    assert!(!encoded.contains("private provider error"));
    assert!(!encoded.contains("private prompt"));
}

#[tokio::test]
async fn provider_mechanism_disabled_preserves_start_without_capture_rows() {
    let (_dir, wire, engine, p, at) = fixture(false, true, false);
    let before = wire.driver.mechanism_lines().unwrap().len();
    assert!(matches!(wire.start(&engine, "fixture-start#0", at, p, false).await.unwrap(), Ran::Started(_)));
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(wire.driver.mechanism_lines().unwrap().len(), before);
}


fn fail_meta(store: &DocsStore, prefix: &str) {
    assert!(matches!(prefix, "jseq:" | "start-intent:" | "fx:"));
    store.transact(|tx| {
        tx.execute_batch(&format!("CREATE TRIGGER synthetic_meta_failure BEFORE INSERT ON orch_meta WHEN NEW.key LIKE '{prefix}%' BEGIN SELECT RAISE(FAIL,'synthetic metadata write fault'); END;"))?;
        Ok::<_, DriverError>(())
    }).unwrap();
}

fn clear_meta_fault(store: &DocsStore) {
    store.transact(|tx| {
        tx.execute_batch("DROP TRIGGER synthetic_meta_failure")?;
        Ok::<_, DriverError>(())
    }).unwrap();
}

#[tokio::test]
async fn provider_start_meta_failure_prevents_external_call() {
    for prefix in ["jseq:", "start-intent:"] {
        let (dir, wire, engine, p, at) = fixture(false, true, false);
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        fail_meta(&store, prefix);
        assert!(wire.start(&engine, "fixture-start#0", at, p, false).await.is_err());
        assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
        assert!(wire.driver.meta("start-intent:fixture-start#0").unwrap().is_none());
        clear_meta_fault(&store);
        drop(wire);
        let reopened = OrchWire::open(store, "mechanism-test", OrchOptions::default()).unwrap();
        reopened.recover(&engine).await.unwrap();
        assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn provider_start_marker_failure_and_fp2_recover_durable_done_once() {
    for mode in [1, 2] {
    let (dir, wire, mut engine, p, _) = fixture(false, true, false);
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.transact(|tx| {
        tx.execute_batch(&format!("CREATE TABLE synthetic_provider(seq INTEGER,calls INTEGER,mode INTEGER); INSERT INTO synthetic_provider VALUES(4,0,{mode});"))?;
        Ok::<_, DriverError>(())
    }).unwrap();
    engine.durable = Some(store.clone());
    wire.stash(1, &p).unwrap();
    fail_meta(&store, "fx:");
    wire.driver.crash_at(super::super::driver::FaultPoint::Fp2EffectRunNotMarked, 0);
    let result = wire.run_effects(&engine, None).await;
    assert!(result.iter().any(Result::is_err));
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert!(wire.driver.meta("fx:fixture-start#0").unwrap().is_none());
    assert!(wire.driver.meta("ran:synthetic-msg").unwrap().is_some());
    assert!(wire.driver.pending().unwrap().iter().any(|r| r.effect_id == "fixture-start#0"));
    clear_meta_fault(&store);
    drop(wire);
    drop(engine);
    let reopened = OrchWire::open(store.clone(), "mechanism-test", OrchOptions::default()).unwrap();
    let engine = Provider { seq: AtomicU64::new(0), calls: AtomicUsize::new(0), fail: false, append: false, durable: Some(store.clone()) };
    reopened.recover(&engine).await.unwrap();
    reopened.recover(&engine).await.unwrap();
    let calls = store.transact(|tx| Ok::<_, DriverError>(tx.query_row("SELECT calls FROM synthetic_provider", [], |r| r.get::<_, u64>(0))?)).unwrap();
    assert_eq!(calls, 1, "same attempt cannot dispatch after a lost success mark");
    let th = reopened.thread_state("synthetic-chat").unwrap();
    assert_eq!(th.attempt.n, 1);
    assert_eq!(th.runs.last().unwrap().status, if mode == 2 { zeron_orch::RunStatus::Completed } else { zeron_orch::RunStatus::Live });
    assert!(reopened.driver.pending().unwrap().iter().all(|r| !matches!(r.effect, Effect::StartRun { .. } | Effect::StartAttempt { .. })));
    }
}

#[tokio::test]
async fn provider_start_intent_without_journal_replaces_attempt_instead_of_replay() {
    let (dir, wire, engine, p, at) = fixture(false, false, false);
    wire.stash(1, &p).unwrap();
    wire.driver.put_meta("jseq:synthetic-chat", "1:1:4").unwrap();
    wire.driver.put_meta("start-intent:fixture-start#0", "1:1:4").unwrap();
    drop(wire);
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let reopened = OrchWire::open(store, "mechanism-test", OrchOptions::default()).unwrap();
    reopened.recover(&engine).await.unwrap();
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    let th = reopened.thread_state("synthetic-chat").unwrap();
    assert!(th.attempt.n > at.n || !th.is_live());
    assert!(reopened.driver.pending().unwrap().iter().filter_map(|r| match &r.effect { Effect::StartAttempt { attempt, .. } => Some(attempt.n), _ => None }).all(|n| n > at.n));
}

#[tokio::test]
async fn complete_failure_releases_claim_and_retries_without_second_start() {
    let (dir, wire, engine, p, _) = fixture(false, true, false);
    wire.stash(1, &p).unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    store.transact(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_complete BEFORE UPDATE OF status ON orch_outbox WHEN NEW.status='done' BEGIN SELECT RAISE(FAIL,'complete fault'); END;")?;
        Ok::<_, DriverError>(())
    }).unwrap();
    assert!(wire.run_effects(&engine, None).await.iter().any(Result::is_err));
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert!(!lock(&wire.claimed).contains("fixture-start#0"));
    store.transact(|tx| {
        tx.execute_batch("DROP TRIGGER fail_complete")?;
        Ok::<_, DriverError>(())
    }).unwrap();
    assert!(wire.run_effects(&engine, None).await.iter().all(Result::is_ok));
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert!(!wire.driver.pending().unwrap().iter().any(|r| r.effect_id == "fixture-start#0"));
}
