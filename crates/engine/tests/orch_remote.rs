use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zeron_doc::{OrchMessage, SessionDoc};
use zeron_engine::{
    EngineError,
    orchestrator::{
        FaultPoint, OrchOptions, OrchWire,
        placement::{self, ChatRoute, Registry},
        wire::{ChildSpec, DelegateRequest, Pending, Ran, RunEngine, SendKind, Via},
    },
};
use zeron_orch::{Hold, check_invariants, well_formed};
use zeron_proto::{AccessMode, DoneStatus, FailureClass};

struct Network {
    routes: BTreeMap<String, ChatRoute>,
    docs: BTreeMap<String, SessionDoc>,
    capable: BTreeSet<String>,
}
impl Registry for Network {
    fn chat(&self, chat: &str) -> Option<ChatRoute> {
        self.routes.get(chat).cloned()
    }
    fn supports_orch(&self, device: &str) -> bool {
        self.capable.contains(device)
    }
}
struct Host {
    device: &'static str,
    network: Arc<Mutex<Network>>,
    starts: Mutex<Vec<(String, String)>>,
    running: Mutex<BTreeSet<String>>,
    interrupts: Mutex<Vec<String>>,
    fail_send: AtomicBool,
    send_attempts: Mutex<Vec<OrchMessage>>,
    fail_response: AtomicBool,
    answers: Mutex<Vec<(String, String)>>,
}
impl Host {
    fn new(device: &'static str, network: Arc<Mutex<Network>>) -> Self {
        Self {
            device,
            network,
            starts: Mutex::new(Vec::new()),
            running: Mutex::new(BTreeSet::new()),
            interrupts: Mutex::new(Vec::new()),
            fail_send: AtomicBool::new(false),
            send_attempts: Mutex::new(Vec::new()),
            fail_response: AtomicBool::new(false),
            answers: Mutex::new(Vec::new()),
        }
    }
    fn count(&self, chat: &str) -> usize {
        self.starts
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| c == chat)
            .count()
    }
    fn backfill(&self, wire: &OrchWire, room: &str) -> Vec<Result<(), String>> {
        let messages = {
            let network = self.network.lock().unwrap();
            let doc = zeron_doc::rebuild_thin_doc(network.docs.get(room).unwrap())
                .unwrap()
                .doc;
            placement::scan(&*network, self.device, &doc).unwrap()
        };
        messages
            .iter()
            .map(|m| wire.receive_remote(self, m).map_err(|e| e.to_string()))
            .collect()
    }
    fn finish(&self, wire: &OrchWire, chat: &str, status: DoneStatus) {
        self.running.lock().unwrap().remove(chat);
        wire.provider_done(chat, status, None);
    }
}
#[async_trait]
impl RunEngine for Host {
    async fn start(&self, p: &Pending) -> Result<String, EngineError> {
        assert_eq!(
            self.orch_route(&p.chat).unwrap().host,
            self.device,
            "provider start on wrong host"
        );
        let prompt = match &p.via {
            Via::Resume { prompt, .. } | Via::Steer { prompt, .. } => prompt.clone(),
            Via::Dispatch { request, .. } => request.prompt.clone(),
        };
        self.starts.lock().unwrap().push((p.chat.clone(), prompt));
        self.running.lock().unwrap().insert(p.chat.clone());
        Ok(p.message_id.clone())
    }
    async fn steer(&self, _: &Pending) -> Result<Ran, EngineError> {
        Ok(Ran::Nothing)
    }
    async fn stop_attempt(&self, chat: &str) -> bool {
        self.interrupts.lock().unwrap().push(chat.into());
        true
    }
    async fn interrupt_run(&self, chat: &str) -> bool {
        self.interrupts.lock().unwrap().push(chat.into());
        true
    }
    fn owns_run(&self, chat: &str) -> bool {
        self.running.lock().unwrap().contains(chat)
    }
    fn last_done(&self, _: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)> {
        None
    }
    fn journal_seq(&self, _: &str) -> u64 {
        0
    }
    fn create_child(&self, parent: &str, spec: &ChildSpec) -> Result<(), EngineError> {
        let mut n = self.network.lock().unwrap();
        let route = ChatRoute {
            chat: spec.chat_id.clone(),
            host: spec.device_id.clone().unwrap(),
            parent: Some(parent.into()),
        };
        if let Some(old) = n.routes.get(&spec.chat_id) {
            assert_eq!(old, &route)
        } else {
            n.routes.insert(spec.chat_id.clone(), route);
            n.docs.insert(
                spec.chat_id.clone(),
                SessionDoc::init(&spec.chat_id).unwrap(),
            );
        }
        Ok(())
    }
    fn respond_question(
        &self,
        chat: &str,
        request: &str,
        _answers: Vec<zeron_proto::UserInputAnswer>,
    ) -> Result<bool, EngineError> {
        if self.fail_response.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.answers
            .lock()
            .unwrap()
            .push((chat.into(), request.into()));
        Ok(true)
    }
    fn result_text(&self, chat: &str) -> Option<String> {
        Some(format!("{chat} result"))
    }
    fn task_results(
        &self,
        _chat: &str,
        _message_id: &str,
        _results: Vec<zeron_doc::TaskResultPart>,
    ) -> Result<(), EngineError> {
        Ok(())
    }
    fn reports_mcp_results(&self, _: &str) -> bool {
        false
    }
    fn kick_drain(&self, _: &str) {}
    fn orch_route(&self, chat: &str) -> Option<ChatRoute> {
        self.network.lock().unwrap().chat(chat)
    }
    fn orch_supports(&self, device: &str) -> bool {
        self.network.lock().unwrap().supports_orch(device)
    }
    fn orch_send(&self, message: &OrchMessage) -> Result<bool, String> {
        self.send_attempts.lock().unwrap().push(message.clone());
        if self.fail_send.load(Ordering::SeqCst) {
            return Err("offline sender".into());
        }
        let n = self.network.lock().unwrap();
        let room = if message.upward() {
            &message.sender_chat
        } else {
            &message.target_chat
        };
        placement::send(&*n, self.device, n.docs.get(room).unwrap(), message)
            .map_err(|e| e.to_string())
    }
}
fn network() -> Arc<Mutex<Network>> {
    Arc::new(Mutex::new(Network {
        routes: [(
            "p".into(),
            ChatRoute {
                chat: "p".into(),
                host: "laptop".into(),
                parent: None,
            },
        )]
        .into(),
        docs: [("p".into(), SessionDoc::init("p").unwrap())].into(),
        capable: ["laptop".into(), "cloud".into()].into(),
    }))
}
fn wire(dir: &std::path::Path, device: &str) -> Arc<OrchWire> {
    OrchWire::open(
        Arc::new(zeron_sync::DocsStore::open(dir).unwrap()),
        device,
        OrchOptions::default(),
    )
    .unwrap()
}
async fn flush(wire: &OrchWire, host: &Host) {
    for _ in 0..12 {
        let results = wire.run_effects(host, None).await;
        if results.is_empty() {
            return;
        }
        for r in results {
            r.unwrap();
        }
    }
    panic!("outbox did not drain")
}
async fn parent(wire: &OrchWire, host: &Host) {
    wire.send(
        host,
        Pending {
            chat: "p".into(),
            message_id: "person-1".into(),
            via: Via::Resume {
                prompt: "root task".into(),
                worktree: None,
            },
        },
        SendKind::Person,
        false,
    )
    .await
    .unwrap();
}
fn request(child: &str) -> DelegateRequest {
    DelegateRequest {
        origin_chat_id: "p".into(),
        task: "remote work".into(),
        title: None,
        role: None,
        mode: Some("async".into()),
        timeout_ms: None,
        client_request_id: Some(format!("request-{child}")),
        observed_input: None,
        followup: false,
        child: ChildSpec {
            chat_id: child.into(),
            device_id: Some("cloud".into()),
            ..ChildSpec::default()
        },
    }
}
fn check(wire: &OrchWire) {
    let s = wire.driver().state().unwrap();
    well_formed(&s).unwrap();
    check_invariants(&s).unwrap();
}
fn trace_pair(name: &str, a: &OrchWire, b: &OrchWire) {
    if let Ok(root) = std::env::var("ORCH_MECHANISM_OUT") {
        let dir = std::path::Path::new(&root).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (dev, wire) in [("laptop", a), ("cloud", b)] {
            std::fs::write(
                dir.join(format!("{dev}.jsonl")),
                wire.driver().mechanism_lines().unwrap().join("\n") + "\n",
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("capture.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "test_name": std::thread::current().name(),
                "files": ["laptop.jsonl", "cloud.jsonl"]
            }))
            .unwrap(),
        )
        .unwrap();
    }
    if let Ok(root) = std::env::var("ORCH_TRACE_OUT") {
        let dir = std::path::Path::new(&root).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (dev, w) in [("laptop", a), ("cloud", b)] {
            std::fs::write(
                dir.join(format!("{dev}.jsonl")),
                w.trace_lines().unwrap().join("\n") + "\n",
            )
            .unwrap();
        }
    }
}

#[tokio::test]
async fn remote_runtime_offline_backfill_duplicate_receipt_and_parent_restart() {
    let n = network();
    let a = Host::new("laptop", n.clone());
    let b = Host::new("cloud", n);
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let wa = wire(da.path(), "laptop");
    let wb = wire(db.path(), "cloud");
    parent(&wa, &a).await;
    wa.delegate(&a, request("c")).await.unwrap();
    flush(&wa, &a).await;
    assert_eq!(a.count("p"), 1);
    assert_eq!(a.count("c"), 0);
    assert!(wb.thread_state("c").is_none());
    a.finish(&wa, "p", DoneStatus::Completed);
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(b.count("c"), 1);
    assert_eq!(b.count("p"), 0);
    assert_ne!(
        wa.thread_state("c").unwrap().id,
        wb.thread_state("c").unwrap().id
    );
    check(&wb);
    b.finish(&wb, "c", DoneStatus::Completed);
    flush(&wb, &b).await;
    drop(wa);
    let wa = wire(da.path(), "laptop");
    wa.driver().crash_at(FaultPoint::Fp3ReceivedNoReceipt, 0);
    assert!(a.backfill(&wa, "c").into_iter().any(|r| r.is_err()));
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert_eq!(a.count("p"), 2);
    assert_eq!(a.count("c"), 0);
    assert_eq!(b.count("c"), 1);
    assert!(
        a.starts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .contains("c result")
    );
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert_eq!(a.count("p"), 2);
    assert_eq!(wa.task_status(&a, "p", "c:1").unwrap().status, "completed");
    check(&wa);
    check(&wb);
    let next = wa
        .followup(&a, "p", "c", "second remote task", "follow-2")
        .await
        .unwrap();
    assert_eq!(next.task_id, "c:2");
    assert_eq!(b.count("c"), 1);
    let old_notice = {
        let n = a.network.lock().unwrap();
        n.docs["c"]
            .read_orch_out()
            .unwrap()
            .into_iter()
            .find(|m| m.id == "notice:c:1")
            .unwrap()
    };
    wa.receive_remote(&a, &old_notice).unwrap();
    assert_ne!(wa.task_status(&a, "p", "c:2").unwrap().status, "completed");
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(b.count("c"), 2);
    assert_eq!(
        b.starts.lock().unwrap().last().unwrap().1,
        "second remote task"
    );
    wa.receive_remote(&a, &old_notice).unwrap();
    assert_ne!(wa.task_status(&a, "p", "c:2").unwrap().status, "completed");
    b.finish(&wb, "c", DoneStatus::Completed);
    flush(&wb, &b).await;
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert_eq!(wa.task_status(&a, "p", "c:2").unwrap().status, "completed");
    a.finish(&wa, "p", DoneStatus::Completed);
    flush(&wa, &a).await;
    assert_eq!(a.count("p"), 3);
    check(&wa);
    check(&wb);
    trace_pair("remote-runtime", &wa, &wb);
}

#[tokio::test]
async fn remote_runtime_retries_failed_send_and_routes_stop_and_live_clamp() {
    let n = network();
    let a = Host::new("laptop", n.clone());
    let b = Host::new("cloud", n.clone());
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let wa = wire(da.path(), "laptop");
    let wb = wire(db.path(), "cloud");
    parent(&wa, &a).await;
    a.fail_send.store(true, Ordering::SeqCst);
    wa.delegate(&a, request("c")).await.unwrap();
    assert!(!wa.driver().pending().unwrap().is_empty());
    assert_eq!(a.count("c"), 0);
    drop(wa);
    let wa = wire(da.path(), "laptop");
    wa.recover(&a).await.unwrap();
    assert!(!wa.driver().pending().unwrap().is_empty());
    a.fail_send.store(false, Ordering::SeqCst);
    wa.recover(&a).await.unwrap();
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(b.count("c"), 1);
    a.fail_send.store(true, Ordering::SeqCst);
    wa.narrow_access(&a, "p", AccessMode::ALL[0]).await.unwrap();
    wa.recover(&a).await.unwrap();
    let _ = wa.run_effects(&a, None).await;
    wa.recover(&a).await.unwrap();
    assert!(
        wa.driver()
            .pending()
            .unwrap()
            .iter()
            .any(|r| matches!(r.effect, zeron_orch::Effect::Reconfigure { .. }))
    );
    a.fail_send.store(false, Ordering::SeqCst);
    wa.recover(&a).await.unwrap();
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(wb.thread_state("c").unwrap().access, 0);
    assert!(!b.interrupts.lock().unwrap().is_empty());
    a.fail_send.store(true, Ordering::SeqCst);
    wa.task_cancel(&a, "p", "c:1", Some("cancel"))
        .await
        .unwrap();
    drop(wa);
    let wa = wire(da.path(), "laptop");
    wa.recover(&a).await.unwrap();
    assert!(wa.driver().pending().unwrap().iter().any(|r| matches!(
        r.effect,
        zeron_orch::Effect::Send(zeron_orch::Message::Stop { .. })
    )));
    a.fail_send.store(false, Ordering::SeqCst);
    wa.recover(&a).await.unwrap();
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(wb.thread_state("c").unwrap().held, Hold::Stop);
    assert!(wb.thread_state("c").unwrap().ul.armed.is_none());
    b.finish(&wb, "c", DoneStatus::Interrupted);
    flush(&wb, &b).await;
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert_eq!(wa.task_status(&a, "p", "c:1").unwrap().status, "cancelled");
    assert_eq!(a.count("c"), 0);
    check(&wa);
    check(&wb);
    trace_pair("remote-stop-clamp", &wa, &wb);
    n.lock().unwrap().capable.remove("cloud");
    assert!(wa.delegate(&a, request("old-engine")).await.is_err());
}

#[tokio::test]
async fn remote_runtime_stop_before_target_online_never_starts_provider() {
    let n = network();
    let a = Host::new("laptop", n.clone());
    let b = Host::new("cloud", n);
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let wa = wire(da.path(), "laptop");
    let wb = wire(db.path(), "cloud");
    parent(&wa, &a).await;
    wa.delegate(&a, request("c")).await.unwrap();
    wa.task_cancel(&a, "p", "c:1", Some("before-online"))
        .await
        .unwrap();
    flush(&wa, &a).await;
    let messages = {
        let n = b.network.lock().unwrap();
        placement::scan(&*n, "cloud", n.docs.get("c").unwrap()).unwrap()
    };
    let stop = messages
        .iter()
        .find(|m| matches!(m.body, zeron_doc::OrchBody::Stop { .. }))
        .unwrap();
    assert!(wb.receive_remote(&b, stop).is_err());
    assert!(
        !wb.driver()
            .has_receipt(&format!("remote-recv:{}", stop.id))
            .unwrap()
    );
    for message in &messages {
        wb.receive_remote(&b, message).unwrap();
    }
    flush(&wb, &b).await;
    assert_eq!(b.count("c"), 0);
    assert_eq!(a.count("c"), 0);
    assert_eq!(wb.thread_state("c").unwrap().held, Hold::Stop);
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert_eq!(wa.task_status(&a, "p", "c:1").unwrap().status, "cancelled");
    check(&wa);
    check(&wb);
    trace_pair("remote-offline-stop", &wa, &wb);
}

#[tokio::test]
async fn remote_runtime_question_answer_closes_once_across_hosts() {
    let n = network();
    let a = Host::new("laptop", n.clone());
    let b = Host::new("cloud", n);
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let wa = wire(da.path(), "laptop");
    let wb = wire(db.path(), "cloud");
    parent(&wa, &a).await;
    wa.delegate(&a, request("c")).await.unwrap();
    flush(&wa, &a).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    a.finish(&wa, "p", DoneStatus::Completed);
    flush(&wa, &a).await;
    wb.question_event(
        "c",
        &zeron_proto::AgentEvent::InputRequested {
            request_id: "req".into(),
            questions: vec![zeron_proto::UserInputQuestion {
                id: "choice".into(),
                header: "Choose".into(),
                question: "Pick one".into(),
                options: vec!["one".into()],
                multi_select: false,
                prefill: None,
                multiline: false,
            }],
        },
    );
    flush(&wb, &b).await;
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    let state = wa.driver().state().unwrap();
    let question = state.questions.values().next().unwrap();
    let epoch = question.epoch;
    assert!(question.pending);
    assert!(question.seen.contains(&wa.thread_state("p").unwrap().id));
    assert_eq!(a.count("p"), 2);
    let answers = vec![zeron_proto::UserInputAnswer {
        question_id: "choice".into(),
        labels: vec!["one".into()],
    }];
    assert!(
        wa.answer_input_command("p", "c", "req", Some(epoch - 1), &answers)
            .is_err()
    );
    wa.answer_input_command("p", "c", "req", Some(epoch), &answers)
        .unwrap()
        .unwrap();
    flush(&wa, &a).await;
    b.fail_response.store(true, Ordering::SeqCst);
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    assert!(
        wb.run_effects(&b, None)
            .await
            .into_iter()
            .any(|r| r.is_err())
    );
    assert!(
        wb.driver()
            .pending()
            .unwrap()
            .iter()
            .any(|row| matches!(row.effect, zeron_orch::Effect::RespondInput { .. }))
    );
    assert!(b.answers.lock().unwrap().is_empty());
    b.fail_response.store(false, Ordering::SeqCst);
    flush(&wb, &b).await;
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(&*b.answers.lock().unwrap(), &[("c".into(), "req".into())]);
    assert!(a.backfill(&wa, "c").into_iter().all(|r| r.is_ok()));
    flush(&wa, &a).await;
    assert!(
        !wa.driver()
            .state()
            .unwrap()
            .questions
            .values()
            .next()
            .unwrap()
            .pending
    );
    assert!(
        !wb.driver()
            .state()
            .unwrap()
            .questions
            .values()
            .next()
            .unwrap()
            .pending
    );
    assert!(
        wa.answer_input_command("p", "c", "req", Some(epoch), &answers)
            .is_err()
    );
    check(&wa);
    check(&wb);
    trace_pair("remote-question", &wa, &wb);
}

#[tokio::test]
async fn remote_recover_blocks_later_controls_and_starts_another_thread() {
    let n = network();
    let a = Host::new("laptop", n.clone());
    let b = Host::new("cloud", n.clone());
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let wa = wire(da.path(), "laptop");
    let wb = wire(db.path(), "cloud");
    parent(&wa, &a).await;
    a.fail_send.store(true, Ordering::SeqCst);
    wa.delegate(&a, request("c")).await.unwrap();
    wa.narrow_access(&a, "p", AccessMode::ALL[0]).await.unwrap();
    wa.task_cancel(&a, "p", "c:1", Some("ordered-stop"))
        .await
        .unwrap();
    {
        let mut net = n.lock().unwrap();
        net.routes.insert(
            "other".into(),
            ChatRoute {
                chat: "other".into(),
                host: "laptop".into(),
                parent: None,
            },
        );
        net.docs
            .insert("other".into(), SessionDoc::init("other").unwrap());
    }
    wa.send(
        &a,
        Pending {
            chat: "other".into(),
            message_id: "other-person".into(),
            via: Via::Resume {
                prompt: "independent task".into(),
                worktree: None,
            },
        },
        SendKind::Person,
        false,
    )
    .await
    .unwrap();
    assert_eq!(a.count("other"), 1);
    let other = wa.thread_state("other").unwrap().id;
    wa.driver()
        .apply(
            "independent-access",
            zeron_orch::Event::Del(zeron_orch::DelEvent::NarrowAccess {
                t: other,
                access: 0,
            }),
        )
        .unwrap();
    assert!(
        wa.driver()
            .pending()
            .unwrap()
            .iter()
            .any(|row| matches!(row.effect, zeron_orch::Effect::StartAttempt { .. }))
    );
    a.send_attempts.lock().unwrap().clear();
    wa.recover(&a).await.unwrap();
    assert_eq!(
        a.count("other"),
        2,
        "failed remote Create blocked another thread"
    );
    {
        let attempts = a.send_attempts.lock().unwrap();
        assert!(!attempts.is_empty());
        assert!(
            attempts
                .iter()
                .all(|m| matches!(m.body, zeron_doc::OrchBody::CreateChild { .. })),
            "later control overtook failed Create"
        );
    }
    let pending = wa.driver().pending().unwrap();
    assert!(
        pending
            .iter()
            .any(|row| matches!(row.effect, zeron_orch::Effect::CreateChild { .. }))
    );
    assert!(
        pending
            .iter()
            .any(|row| matches!(row.effect, zeron_orch::Effect::Reconfigure { .. }))
    );
    assert!(pending.iter().any(|row| matches!(
        row.effect,
        zeron_orch::Effect::Send(zeron_orch::Message::Stop { .. })
    )));
    a.fail_send.store(false, Ordering::SeqCst);
    a.send_attempts.lock().unwrap().clear();
    wa.recover(&a).await.unwrap();
    flush(&wa, &a).await;
    let attempts = a.send_attempts.lock().unwrap().clone();
    assert!(matches!(
        attempts[0].body,
        zeron_doc::OrchBody::CreateChild { .. }
    ));
    assert!(
        attempts
            .iter()
            .any(|m| matches!(m.body, zeron_doc::OrchBody::Clamp { .. }))
    );
    assert!(
        attempts
            .iter()
            .any(|m| matches!(m.body, zeron_doc::OrchBody::Stop { .. }))
    );
    assert!(b.backfill(&wb, "c").into_iter().all(|r| r.is_ok()));
    flush(&wb, &b).await;
    assert_eq!(b.count("c"), 0);
    assert_eq!(wb.thread_state("c").unwrap().held, Hold::Stop);
    check(&wa);
    check(&wb);
    trace_pair("remote-recovery-order", &wa, &wb);
}
