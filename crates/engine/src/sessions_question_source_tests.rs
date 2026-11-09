use super::*;
use zeron_orch::{Event, Mode, Node, Outcome, ProviderEvent, RunEvent};

#[tokio::test]
async fn retained_input_issuance_cannot_cross_attempt_rebind() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
    let wire = OrchWire::open(
        store,
        "question-source",
        crate::orchestrator::OrchOptions::default(),
    )
    .unwrap();
    let (id, _) = wire.driver().intern("chat:child", "n:thread").unwrap();
    let child = zeron_orch::ThreadId(u32::try_from(id).unwrap());
    wire.driver()
        .put_meta(&format!("thread:{}", child.0), "child")
        .unwrap();
    let apply = |key: &str, event| wire.driver().apply(key, event).unwrap();
    apply(
        "open",
        Event::Del(zeron_orch::DelEvent::OpenRoot {
            t: child,
            access: 2,
            reconfigure: true,
        }),
    );
    apply(
        "person1",
        Event::Run(RunEvent::PersonSend {
            t: child,
            mode: Mode::Queue,
            msg: 1,
        }),
    );
    apply("start1", Event::Run(RunEvent::StartFromQueue { t: child }));
    let issued = wire
        .driver()
        .state()
        .unwrap()
        .thread(child)
        .unwrap()
        .attempt_id();
    let pending: PendingInputs = Arc::new(Mutex::new(HashMap::new()));
    let (engine_tx, mut engine_rx) = mpsc::unbounded_channel();
    let terminal = ProviderEvent {
        node: Node::Root,
        run: issued.run.ord,
        attempt: issued.n,
        outcome: Outcome::Completed,
    };
    apply(
        "done1",
        Event::Run(RunEvent::ProviderTerminal {
            t: child,
            run: terminal.run,
            attempt: terminal.attempt,
            outcome: terminal.outcome,
        }),
    );
    apply(
        "end1",
        Event::Run(RunEvent::IngestEnd {
            t: child,
            ev: terminal,
        }),
    );
    apply(
        "person2",
        Event::Run(RunEvent::PersonSend {
            t: child,
            mode: Mode::Queue,
            msg: 2,
        }),
    );
    apply("start2", Event::Run(RunEvent::StartFromQueue { t: child }));
    let rebound = wire
        .driver()
        .state()
        .unwrap()
        .thread(child)
        .unwrap()
        .attempt_id();
    let (reply, answer) = oneshot::channel();
    ask_person(&pending, &engine_tx, vec![], reply, Some(issued), None);
    let stale = engine_rx.recv().await.unwrap();
    let AgentEvent::InputRequested { request_id, .. } = &stale else {
        panic!("input event expected")
    };
    assert!(lock(&pending).contains_key(request_id));
    wire.question_event_at("child", &stale, Some(issued));
    assert!(wire.driver().state().unwrap().questions.is_empty());
    assert!(!current_input(
        &pending,
        request_id,
        Some(rebound),
        Some(&wire),
        "child"
    ));
    assert!(answer.await.unwrap().is_empty());
    assert!(lock(&pending).is_empty());
    let (reply, _answer) = oneshot::channel();
    ask_person(&pending, &engine_tx, vec![], reply, Some(rebound), None);
    let fresh = engine_rx.recv().await.unwrap();
    let AgentEvent::InputRequested { request_id, .. } = &fresh else {
        panic!("input event expected")
    };
    assert!(current_input(
        &pending,
        request_id,
        Some(rebound),
        Some(&wire),
        "child"
    ));
    wire.question_event_at("child", &fresh, Some(rebound));
    assert_eq!(wire.driver().state().unwrap().questions.len(), 1);
    if let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") {
        let out = std::path::PathBuf::from(root).join(format!("question-source-{}", new_id()));
        std::fs::create_dir_all(&out).unwrap();
        let file = "question-source.jsonl";
        std::fs::write(
            out.join(file),
            wire.driver().mechanism_lines().unwrap().join("\n") + "\n",
        )
        .unwrap();
        std::fs::write(out.join("capture.json"),serde_json::to_vec(&serde_json::json!({"test_name":std::thread::current().name(),"devices":["question-source"],"filenames":[file],"synthetic":true,"scope":"unit input bridge and routing"})).unwrap()).unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn source_steer_pins_callbacks_before_drive_consumes_boundaries() {
    let attempt = |ord| zeron_orch::AttemptId {
        run: zeron_orch::RunId {
            thread: zeron_orch::ThreadId(1),
            ord,
        },
        n: 1,
    };
    let steer = |ord| RoutedSteer {
        orch_attempt: Some(attempt(ord)),
        prompt: String::new(),
        message_id: format!("m{ord}"),
        fork_history: false,
        after_stop: false,
        request: None,
    };
    let source = Arc::new(Mutex::new(InputSource {
        attempt: Some(attempt(1)),
        ..Default::default()
    }));
    let ledger = Arc::new(Mutex::new(std::collections::VecDeque::from([
        steer(2),
        steer(3),
    ])));
    let turn = TurnControl::default();
    let (stamp, accepted) = (source.clone(), ledger.clone());
    turn.set_source_steered(move || lock(&stamp).confirm(&lock(&accepted)));
    let pending: PendingInputs = Arc::new(Mutex::new(HashMap::new()));
    let (events, mut received) = mpsc::unbounded_channel();
    let (gate, delayed) = oneshot::channel::<()>();
    let callback = |wait: Option<oneshot::Receiver<()>>| {
        let issued = lock(&source).attempt;
        let (pending, events) = (pending.clone(), events.clone());
        let (reply, answer) = oneshot::channel();
        tokio::spawn(async move {
            if let Some(wait) = wait {
                let _ = wait.await;
            }
            ask_person(&pending, &events, vec![], reply, issued, None);
        });
        answer
    };
    let _old = callback(Some(delayed));
    turn.source_steered();
    let _new = callback(None);
    turn.source_steered();
    let _newest = callback(None);
    assert_eq!(lock(&ledger).len(), 2);
    assert_eq!(lock(&source).attempt, Some(attempt(3)));
    gate.send(()).unwrap();
    let mut stamps = Vec::new();
    for _ in 0..3 {
        let AgentEvent::InputRequested { request_id, .. } = received.recv().await.unwrap() else {
            panic!("input expected")
        };
        stamps.push(lock(&pending)[&request_id].attempt.unwrap());
    }
    stamps.sort();
    assert_eq!(stamps, vec![attempt(1), attempt(2), attempt(3)]);
    lock(&ledger).pop_front();
    lock(&ledger).pop_front();
    lock(&ledger).push_back(steer(4));
    turn.source_steered();
    assert_eq!(lock(&source).attempt, Some(attempt(4)));
    assert_eq!(lock(&source).confirmed.len(), 1);
}
