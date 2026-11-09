use super::*;
use crate::orchestrator::FaultPoint;

struct StopEngine {
    interrupted: Mutex<Vec<String>>,
    store: Arc<DocsStore>,
    saves: std::sync::atomic::AtomicUsize,
    journal_done: Option<(u64, DoneStatus, Option<FailureClass>)>,
}
#[async_trait::async_trait]
impl RunEngine for StopEngine {
    async fn start(&self, _: &Pending) -> Result<String, EngineError> { panic!("unexpected provider start") }
    async fn steer(&self, _: &Pending) -> Result<Ran, EngineError> { panic!("unexpected steer") }
    async fn stop_attempt(&self, _: &str) -> bool { false }
    async fn interrupt_run(&self, chat: &str) -> bool { lock(&self.interrupted).push(chat.into()); false }
    fn owns_run(&self, _: &str) -> bool { false }
    fn last_done(&self, _: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)> { self.journal_done }
    fn journal_seq(&self, _: &str) -> u64 { self.journal_done.map_or(0, |done| done.0) }
    fn native_snapshot(&self, doc: &str) -> Result<Vec<u8>, EngineError> {
        self.store.load_snapshot(doc).map_err(|e| EngineError::Other(e.to_string()))?
            .ok_or_else(|| EngineError::Other("missing saved native doc".into()))
    }
    fn save_native_snapshot(&self, doc: &str, snapshot: &[u8]) -> Result<(), EngineError> {
        self.store.save_snapshot(doc, snapshot).map_err(|e| EngineError::Other(e.to_string()))?;
        self.saves.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn create_child(&self, _: &str, _: &ChildSpec) -> Result<(), EngineError> { Ok(()) }
    fn result_text(&self, _: &str) -> Option<String> { None }
    fn reports_mcp_results(&self, _: &str) -> bool { false }
    fn task_results(&self, _: &str, _: &str, _: Vec<zeron_doc::TaskResultPart>) -> Result<(), EngineError> { Ok(()) }
    fn kick_drain(&self, _: &str) {}
}

#[tokio::test]
async fn stop_parked_native_survives_crash_before_runtime_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "stop-test", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    let apply = |id: &str, event| wire.driver.apply_command(id, None, event).unwrap();
    apply("queue", Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 }));
    apply("start", Event::Run(RunEvent::StartFromQueue { t: parent }));
    let path = vec!["background".to_owned()];
    crate::orchestrator::native::observe(&wire.driver, parent, &zeron_proto::AgentEvent::ToolCall {
        id: path[0].clone(), call: zeron_proto::ToolCall::Unknown {
            name: "Agent".into(), input: Some(serde_json::json!({"run_in_background":true})),
        },
    }).unwrap();
    crate::orchestrator::native::activate(&wire.driver, parent, &path).unwrap();
    let key = crate::orchestrator::native::path_key(&wire.driver, parent, &path).unwrap().unwrap();
    let binding = crate::orchestrator::native::binding(&wire.driver, key).unwrap().unwrap();
    let native_doc = zeron_doc::SessionDoc::init(&binding.doc_id).unwrap();
    native_doc.push_message(&zeron_doc::SessionMessageEntry {
        id: "partial".into(), role: zeron_doc::MessageRole::Assistant,
        parts: vec![zeron_doc::MessagePart::Text { id: "text".into(), text: "partial native result".into() }],
        created_at: 1, device_id: "stop-test".into(), status: Some(zeron_doc::MessageStatus::Streaming),
        continuation_of: None, duration_ms: None,
    }).unwrap();
    store.save_snapshot(&binding.doc_id, &native_doc.export_snapshot().unwrap()).unwrap();
    let at = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    let terminal = zeron_orch::ProviderEvent { node: zeron_orch::Node::Root, run: at.run.ord, attempt: at.n, outcome: Outcome::Completed };
    apply("terminal", Event::Run(RunEvent::ProviderTerminal { t: parent, run: at.run.ord, attempt: at.n, outcome: Outcome::Completed }));
    apply("end", Event::Run(RunEvent::IngestEnd { t: parent, ev: terminal }));
    for row in wire.driver.pending().unwrap() {
        wire.driver.complete(&row.effect_id).unwrap();
    }
    let ids = with_command("stop-command", async { wire.request_interrupt("parent") }).await.unwrap();
    assert!(wire.command_receipt("stop-command"));
    assert!(wire.driver.pending().unwrap().iter().any(|row| ids.contains(&row.effect_id) && matches!(row.effect, Effect::InterruptRun { .. })));
    drop(wire);
    drop(store);

    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "stop-test", OrchOptions::default()).unwrap();
    let engine = StopEngine { interrupted: Mutex::new(Vec::new()), store: store.clone(), saves: std::sync::atomic::AtomicUsize::new(0), journal_done: None };
    let ran = wire.run_effects(&engine, Some(&ids)).await;
    assert!(!ran.is_empty());
    assert!(ran.iter().all(Result::is_ok));
    assert_eq!(*lock(&engine.interrupted), ["parent"]);
    assert_eq!(wire.driver.state().unwrap().natives[&(parent, key)].status, zeron_orch::native::NativeStatus::Failed);
    assert_eq!(engine.saves.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(wire.driver.meta(&format!("native-terminal:{parent:?}:{key}")).unwrap().is_none());
    let persisted = loro::LoroDoc::new();
    persisted.import(&store.load_snapshot(&binding.doc_id).unwrap().unwrap()).unwrap();
    let persisted = zeron_doc::SessionDoc::from_doc(persisted);
    assert_eq!(persisted.read_entries().unwrap()[0].status, Some(zeron_doc::MessageStatus::Aborted));
    assert!(serde_json::to_string(&persisted.read_entries().unwrap()).unwrap().contains("partial native result"));
    assert!(!wire.driver.pending().unwrap().iter().any(|row| ids.contains(&row.effect_id)));
    let ran = wire.run_effects(&engine, Some(&ids)).await;
    assert!(ran.iter().all(Result::is_ok));
    assert_eq!(*lock(&engine.interrupted), ["parent"]);
    assert_eq!(engine.saves.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_old_narrowed_attempt_done_cannot_interrupt_its_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store, "narrow-fence", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend {
        t: parent, mode: Mode::Queue, msg: 1,
    })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue {t:parent})).unwrap();
    let old = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    wire.driver.apply_command("narrow", None, Event::Del(DelEvent::NarrowAccess {t:parent,access:0})).unwrap();
    let next = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    assert_ne!(old, next);
    lock(&wire.expect).insert("parent".into(), next);
    assert!(!lock(&wire.quiet).contains_key("parent"));
    wire.provider_done_at("parent", DoneStatus::Interrupted, None, old);
    let state = wire.driver.state().unwrap();
    let current = state.thread(parent).unwrap();
    assert_eq!(current.attempt_id(), next);
    assert!(current.is_live());
    assert!(!current.int_req);
    assert!(!wire.driver.trace_lines().unwrap().iter().any(|line| line.contains("\"InterruptChat\"")));
    let ids = wire.request_interrupt("parent").unwrap();
    assert!(!ids.is_empty());
    wire.provider_done_at("parent", DoneStatus::Interrupted, None, next);
    assert!(!wire.driver.state().unwrap().thread(parent).unwrap().is_live());
}

#[tokio::test]
async fn a_completed_effect_cannot_be_reclaimed_from_a_stale_pending_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "stale-pending", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    wire.request_interrupt("parent").unwrap();
    let snapshot_a = wire.driver.pending().unwrap();
    let snapshot_b = wire.driver.pending().unwrap();
    let row_a = snapshot_a.iter().find(|row| matches!(row.effect, Effect::InterruptRun { .. })).unwrap();
    let row_b = snapshot_b.iter().find(|row| row.effect_id == row_a.effect_id).unwrap();
    let engine = StopEngine { interrupted: Mutex::new(Vec::new()), store, saves: std::sync::atomic::AtomicUsize::new(0), journal_done: None };
    assert!(wire.claim_pending(&row_a.effect_id).unwrap());
    assert!(!wire.claim_pending(&row_b.effect_id).unwrap());
    wire.execute(&engine, row_a, false).await.unwrap();
    wire.driver.complete(&row_a.effect_id).unwrap();
    lock(&wire.claimed).remove(&row_a.effect_id);
    assert!(!wire.claim_pending(&row_b.effect_id).unwrap());
    assert!(!lock(&wire.claimed).contains(&row_b.effect_id));
    assert_eq!(*lock(&engine.interrupted), ["parent"]);
    assert!(wire.run_effects(&engine, None).await.iter().all(Result::is_ok));
    wire.recover(&engine).await.unwrap();
    assert_eq!(*lock(&engine.interrupted), ["parent"]);
}

#[tokio::test]
async fn boot_recovery_cannot_bind_a_different_saved_input_or_a_stopped_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "recovery-binding", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    let at = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    wire.driver.put_meta("jseq:parent", "1:1:0").unwrap();
    wire.driver.put_meta("start-input:parent", &serde_json::to_string(&(1u32, 1u32, "saved-input")).unwrap()).unwrap();
    drop(wire);
    let wire = OrchWire::open(store.clone(), "recovery-binding", OrchOptions::default()).unwrap();
    let engine = StopEngine { interrupted: Mutex::new(Vec::new()), store, saves: std::sync::atomic::AtomicUsize::new(0), journal_done: None };
    let pending = |id: &str| Pending { chat: "parent".into(), message_id: id.into(), via: Via::Resume { prompt: "recovered".into(), worktree: None } };
    assert_eq!(wire.recover_start(&engine, pending("other-input")).unwrap(), "");
    assert!(wire.runtime_attempt_current(at));
    wire.request_interrupt("parent").unwrap();
    assert!(!wire.runtime_attempt_current(at));
    assert_eq!(wire.recover_start(&engine, pending("saved-input")).unwrap(), "");
    assert!(engine.interrupted.lock().unwrap().is_empty());
}

#[test]
fn retained_turn_admission_rejects_stale_attempt_access_and_stop_before_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store, "retained-admission", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    let old = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    wire.driver.apply_command("narrow", None, Event::Del(DelEvent::NarrowAccess { t: parent, access: 0 })).unwrap();
    let next = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    let committed = std::cell::Cell::new(0);
    let commit = || { committed.set(committed.get() + 1); Ok(()) };
    assert!(wire.with_runtime_admission(old, 0, commit).is_err());
    assert!(wire.with_runtime_admission(next, 2, commit).is_err());
    assert_eq!(committed.get(), 0);
    wire.with_runtime_admission(next, 0, commit).unwrap();
    assert_eq!(committed.get(), 1);
    wire.request_interrupt("parent").unwrap();
    assert!(wire.with_runtime_admission(next, 0, commit).is_err());
    assert_eq!(committed.get(), 1);
}

#[test]
fn synthetic_engine_interrupt_rolls_back_its_effect_and_receipt_on_store_fault() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    Driver::open_with_capture(store.clone(), "synthetic-interrupt", Config::default(), true).unwrap();
    let wire = OrchWire::open(store, "synthetic-interrupt", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    let at = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    let before_state = wire.driver.state().unwrap();
    let before_outbox = wire.driver.outbox().unwrap();
    let before_capture = wire.driver.mechanism_lines().unwrap();
    let before_trace = wire.driver.trace_lines().unwrap();
    wire.driver.crash_at(FaultPoint::Fp2EffectRunNotMarked, 0);
    assert!(wire.engine_interrupt("parent", at).is_err());
    assert_eq!(wire.driver.state().unwrap(), before_state);
    assert_eq!(wire.driver.outbox().unwrap(), before_outbox);
    assert_eq!(wire.driver.mechanism_lines().unwrap(), before_capture);
    assert_eq!(wire.driver.trace_lines().unwrap(), before_trace);
    wire.engine_interrupt("parent", at).unwrap();
    assert!(wire.driver.state().unwrap().thread(parent).unwrap().int_req);
    assert!(wire.driver.pending().unwrap().is_empty());
    let rows: Vec<serde_json::Value> = wire.driver.mechanism_lines().unwrap().iter()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    let applied = rows.iter().rev().find(|row| row["action"] == "Apply"
        && row["args"]["key"] == "int:parent:1:1:engine").unwrap();
    let completed = rows.iter().rev().find(|row| row["action"] == "CompleteEffect"
        && row["args"]["effect_id"].as_str().is_some_and(|id| id.starts_with("int:parent:1:1:engine#"))).unwrap();
    assert!(applied["origin"]["transaction"].is_string());
    assert_eq!(applied["origin"]["transaction"], completed["origin"]["transaction"]);
    assert_ne!(applied["origin"]["commit"], completed["origin"]["commit"]);
    let outbox = wire.driver.outbox().unwrap();
    assert!(outbox.iter().any(|row| matches!(row.effect, Effect::InterruptRun { .. }) && row.done));
    wire.engine_interrupt("parent", at).unwrap();
    assert_eq!(wire.driver.outbox().unwrap(), outbox);
}

#[test]
fn synthetic_callback_does_not_retire_a_genuine_stop() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store, "genuine-stop", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    let at = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
    wire.request_interrupt("parent").unwrap();
    let pending = wire.driver.pending().unwrap();
    assert!(pending.iter().any(|row| matches!(row.effect, Effect::InterruptRun { .. })));
    wire.engine_interrupt("parent", at).unwrap();
    assert_eq!(wire.driver.pending().unwrap(), pending);
}

#[tokio::test]
async fn lost_boot_runtime_retries_a_new_attempt_with_its_exact_input() {
    for journal_done in [None, Some((2, DoneStatus::Interrupted, None))] {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let wire = OrchWire::open(store.clone(), "lost-boot", OrchOptions::default()).unwrap();
        let parent = wire.thread("parent").unwrap();
        wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
        wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
        let old = wire.driver.state().unwrap().thread(parent).unwrap().attempt_id();
        for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
        wire.driver.put_meta("jseq:parent", "1:1:0").unwrap();
        wire.driver.put_meta("start-input:parent", &serde_json::to_string(&(1u32, 1u32, "saved-input")).unwrap()).unwrap();
        drop(wire);
        let wire = OrchWire::open(store.clone(), "lost-boot", OrchOptions::default()).unwrap();
        let engine = StopEngine { interrupted: Mutex::new(Vec::new()), store, saves: std::sync::atomic::AtomicUsize::new(0), journal_done };
        let pending = Pending { chat: "parent".into(), message_id: "saved-input".into(), via: Via::Resume { prompt: "exact saved context".into(), worktree: None } };
        assert_eq!(wire.recover_start(&engine, pending.clone()).unwrap(), "");
        let state = wire.driver.state().unwrap();
        let next = state.thread(parent).unwrap().attempt_id();
        assert_eq!(next.run, old.run);
        assert_eq!(next.n, old.n + 1);
        assert!(!state.thread(parent).unwrap().int_req);
        let retry = wire.attempt_pending(&engine, next, None).unwrap();
        assert_eq!(serde_json::to_value(retry).unwrap(), serde_json::to_value(pending).unwrap());
        assert!(wire.driver.pending().unwrap().iter().any(|row| matches!(row.effect, Effect::StartAttempt { attempt, .. } if attempt == next)));
    }
}

#[tokio::test]
async fn the_startup_gate_blocks_worker_ticks_and_direct_effect_drains() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "startup-gate", OrchOptions {
        reconcile_every: Duration::from_millis(1), ..OrchOptions::default()
    }).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    wire.request_interrupt("parent").unwrap();
    let pending = wire.driver.pending().unwrap();
    assert!(!pending.is_empty());
    let before = wire.driver.state().unwrap();
    let engine = Arc::new(StopEngine { interrupted: Mutex::new(Vec::new()), store, saves: std::sync::atomic::AtomicUsize::new(0), journal_done: Some((2, DoneStatus::Interrupted, None)) });
    wire.attach_recovering(engine.clone());
    wire.wake.notify_one();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(wire.recovery_pending());
    assert_eq!(wire.driver.state().unwrap(), before);
    let drained = wire.run_effects(engine.as_ref(), None).await;
    assert!(matches!(&drained[..], [Err(EngineError::Other(message))] if message.contains("recovery is pending or blocked")));
    assert_eq!(wire.driver.state().unwrap(), before);
    assert_eq!(wire.driver.pending().unwrap(), pending);
    assert!(engine.interrupted.lock().unwrap().is_empty());
    wire.detach();
}

#[tokio::test]
async fn a_recovery_effect_failure_keeps_the_startup_gate_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(store.clone(), "startup-error", OrchOptions::default()).unwrap();
    let parent = wire.thread("parent").unwrap();
    wire.driver.apply_command("queue", None, Event::Run(RunEvent::PersonSend { t: parent, mode: Mode::Queue, msg: 1 })).unwrap();
    wire.driver.apply_command("start", None, Event::Run(RunEvent::StartFromQueue { t: parent })).unwrap();
    for row in wire.driver.pending().unwrap() { wire.driver.complete(&row.effect_id).unwrap(); }
    wire.request_interrupt("parent").unwrap();
    let engine = Arc::new(StopEngine { interrupted: Mutex::new(Vec::new()), store, saves: std::sync::atomic::AtomicUsize::new(0), journal_done: None });
    wire.attach_recovering(engine.clone());
    wire.driver.crash_at(FaultPoint::Fp2EffectRunNotMarked, 0);
    assert!(wire.recover(engine.as_ref()).await.is_err());
    assert!(wire.recovery_pending());
    assert_eq!(*engine.interrupted.lock().unwrap(), ["parent"]);
    assert!(wire.run_effects(engine.as_ref(), None).await.iter().all(Result::is_err));
    assert_eq!(*engine.interrupted.lock().unwrap(), ["parent"]);
    assert!(!wire.driver.pending().unwrap().is_empty());
    wire.detach();
}
