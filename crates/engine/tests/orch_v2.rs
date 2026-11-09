use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::SessionCommandPayload;
use zeron_engine::orchestrator::wire::RESUME_PROMPT;
use zeron_engine::orchestrator::{FaultPoint, OrchOptions, OrchWire, trace};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_orch::{Config, Hold, RunStatus, Thread};
use zeron_proto::{
    AgentEvent, DoneStatus, FailureClass, HarnessId, Model, ReasoningLevel, RunRequest,
    SandboxLevel, SteeringMode,
};
use zeron_rpc::RpcService;

const CHAT: &str = "chat-orch";
const FAILSAFE: Duration = Duration::from_secs(60);
const RECHECK: Duration = Duration::from_millis(250);

static TICK: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::Sender::new(0));

fn poke() {
    TICK.send_modify(|n| *n = n.wrapping_add(1));
}

fn forward_changes(wire: &OrchWire) {
    let mut rx = wire.driver().changes();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            poke();
        }
    });
}

async fn until(mut done: impl FnMut() -> bool) -> bool {
    let mut rx = TICK.subscribe();
    let deadline = tokio::time::Instant::now() + FAILSAFE;
    while !done() {
        if tokio::time::Instant::now() > deadline {
            return false;
        }
        rx.mark_unchanged();
        let _ = tokio::time::timeout(RECHECK, rx.changed()).await;
    }
    true
}

#[derive(Clone, Copy)]
enum Plan {
    Done(DoneStatus, Option<FailureClass>),
    Hold(DoneStatus, Option<FailureClass>),
}

struct Agent {
    steering: Option<SteeringMode>,
    finish: tokio::sync::broadcast::Sender<()>,
    runs: Arc<Mutex<Vec<String>>>,
    accesses: Mutex<Vec<zeron_proto::AccessMode>>,
    folders: Mutex<Vec<String>>,
    projection_store: Mutex<Option<std::path::PathBuf>>,
    projection_seen: std::sync::atomic::AtomicBool,
    steers: Arc<Mutex<Vec<String>>>,
    tries: Mutex<HashMap<String, u32>>,
    interrupt_delay: Duration,
    parks: bool,
}

impl Agent {
    fn parking() -> Arc<Self> {
        let mut a = Arc::into_inner(Self::new(Some(SteeringMode::TurnBoundary))).expect("fresh");
        a.parks = true;
        Arc::new(a)
    }

    fn new(steering: Option<SteeringMode>) -> Arc<Self> {
        let (finish, _) = tokio::sync::broadcast::channel(16);
        Arc::new(Self {
            steering,
            finish,
            runs: Arc::default(),
            accesses: Mutex::default(),
            folders: Mutex::default(),
            projection_store: Mutex::default(),
            projection_seen: std::sync::atomic::AtomicBool::new(false),
            steers: Arc::default(),
            tries: Mutex::default(),
            interrupt_delay: Duration::from_millis(300),
            parks: false,
        })
    }

    fn runs(&self) -> Vec<String> {
        self.runs.lock().unwrap().clone()
    }

    fn plan(&self, prompt: &str) -> Plan {
        use DoneStatus::{Completed, Errored};
        let word = prompt.split_whitespace().next().unwrap_or_default();
        if let Some(n) = word.strip_prefix("flaky") {
            let fails: u32 = n.parse().unwrap_or(1);
            let mut tries = self.tries.lock().unwrap();
            let seen = tries.entry(prompt.to_owned()).or_default();
            *seen += 1;
            return if *seen <= fails {
                Plan::Done(Errored, Some(FailureClass::TransportError))
            } else {
                Plan::Done(Completed, None)
            };
        }
        match word {
            "hold" => Plan::Hold(Completed, None),
            "limit-hold" => Plan::Hold(Errored, Some(FailureClass::UsageLimit { reset_at: None })),
            "limit" => Plan::Done(Errored, Some(FailureClass::UsageLimit { reset_at: None })),
            "limit-at" => Plan::Done(
                Errored,
                Some(FailureClass::UsageLimit {
                    reset_at: Some(chrono::Utc::now().timestamp_millis() + 2_000),
                }),
            ),
            "deny" => Plan::Done(Errored, Some(FailureClass::PermissionError)),
            "invalid" => Plan::Done(Errored, Some(FailureClass::ValidationError)),
            "unknown" => Plan::Done(Errored, Some(FailureClass::Unknown)),
            "selfstop" => Plan::Done(DoneStatus::Interrupted, None),
            "deny-hold" => Plan::Hold(Errored, Some(FailureClass::PermissionError)),
            _ => Plan::Done(Completed, None),
        }
    }
}

fn ack_wait(
    tx: &futures::channel::mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>,
    crid: &str,
) {
    let id = format!("call-{crid}");
    let call = zeron_proto::ToolCall::Mcp {
        server: "zeron".into(),
        tool: "delegate_task".into(),
        input: Some(serde_json::json!({ "mode": "wait", "clientRequestId": crid })),
    };
    let _ = tx.unbounded_send(Ok(AgentEvent::ToolCall {
        id: id.clone(),
        call,
    }));
    let _ = tx.unbounded_send(Ok(AgentEvent::ToolResult {
        id,
        is_error: false,
        output: None,
        diff: None,
        exit_code: None,
    }));
}

fn done(status: DoneStatus, failure: Option<FailureClass>) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: (status == DoneStatus::Errored).then(|| "scripted failure".into()),
        session_id: Some("sess-orch".into()),
        failure,
    }
}

async fn hold(
    then: AgentEvent,
    controls: &mut RunControls,
    finish: &mut tokio::sync::broadcast::Receiver<()>,
    steers: &Arc<Mutex<Vec<String>>>,
    tx: &futures::channel::mpsc::UnboundedSender<Result<AgentEvent, HarnessError>>,
    delay: Duration,
) -> AgentEvent {
    loop {
        tokio::select! {
            _ = finish.recv() => return then,
            () = controls.interrupt.cancelled() => {
                tokio::time::sleep(delay).await;
                return done(DoneStatus::Interrupted, None);
            }
            Some(m) = controls.steering.recv() => {
                if let Some(crid) = m.prompt.strip_prefix("ack ") {
                    ack_wait(tx, crid);
                }
                steers.lock().unwrap().push(m.prompt);
                poke();
                let _ = tx.unbounded_send(Ok(AgentEvent::Steered {
                    assistant_message_id: None,
                    next_assistant_message_id: Some(uuid::Uuid::new_v4().to_string()),
                }));
            }
        }
    }
}

#[async_trait]
impl Harness for Agent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Orch"
    }
    fn supports_steering(&self) -> bool {
        self.steering.is_some()
    }
    fn steering_mode(&self) -> SteeringMode {
        self.steering.unwrap_or(SteeringMode::TurnBoundary)
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        if request.prompt.starts_with("Task results") {
            let path = self.projection_store.lock().unwrap().clone();
            if let Some(path) = path {
                let store = zeron_sync::DocsStore::open(path).expect("fresh result store");
                let bytes = store
                    .load_snapshot(CHAT)
                    .expect("load result snapshot")
                    .expect("durable result snapshot");
                let doc = zeron_doc::SessionDoc::init(CHAT).unwrap();
                doc.doc().import(&bytes).expect("import durable snapshot");
                let seen = doc
                    .read_entries()
                    .unwrap()
                    .iter()
                    .flat_map(|e| &e.parts)
                    .any(|p| matches!(p, zeron_doc::MessagePart::TaskResults { .. }));
                self.projection_seen
                    .store(seen, std::sync::atomic::Ordering::SeqCst);
            }
        }
        if request.prompt.starts_with("busy") {
            self.runs.lock().unwrap().push(request.prompt.clone());
            poke();
            return Err(HarnessError::Io(std::io::ErrorKind::ResourceBusy.into()));
        }
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let _ = tx.unbounded_send(Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: "sess-orch".into(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        }));
        let plan = self.plan(&request.prompt);
        self.accesses.lock().unwrap().push(request.access);
        self.folders.lock().unwrap().push(request.cwd.clone());
        let finish = self.finish.subscribe();
        self.runs.lock().unwrap().push(request.prompt.clone());
        poke();
        let steers = self.steers.clone();
        let delay = self.interrupt_delay;
        let parks = self.parks;
        let prompt = request.prompt.clone();
        tokio::spawn(async move {
            let (mut controls, mut finish, mut plan) = (controls, finish, plan);
            loop {
                let last = match plan {
                    Plan::Done(status, failure) => done(status, failure),
                    Plan::Hold(status, failure) => {
                        let then = done(status, failure);
                        hold(then, &mut controls, &mut finish, &steers, &tx, delay).await
                    }
                };
                let stopped = matches!(
                    last,
                    AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        ..
                    }
                );
                if !stopped {
                    let text = format!("result of {}", prompt);
                    let _ = tx.unbounded_send(Ok(AgentEvent::TextDelta { text }));
                }
                let _ = tx.unbounded_send(Ok(last));
                if !parks || stopped {
                    return;
                }
                let next = tokio::select! {
                    m = controls.steering.recv() => m,
                    () = controls.interrupt.cancelled() => None,
                };
                let Some(m) = next else { return };
                plan = if m.prompt.starts_with("hold") {
                    Plan::Hold(DoneStatus::Completed, None)
                } else {
                    Plan::Done(DoneStatus::Completed, None)
                };
                steers.lock().unwrap().push(m.prompt);
                poke();
                let _ = tx.unbounded_send(Ok(AgentEvent::Steered {
                    assistant_message_id: None,
                    next_assistant_message_id: Some(uuid::Uuid::new_v4().to_string()),
                }));
            }
        });
        Ok(rx.boxed())
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("mock-1".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        extra_mcp: Vec::new(),
    }
}

fn options(delay: Duration) -> OrchOptions {
    OrchOptions {
        config: Config::default(),
        limit_delays: vec![delay, delay],
        ..OrchOptions::default()
    }
}

struct Rig {
    core: EngineCore,
    agent: Arc<Agent>,
    wire: Option<Arc<OrchWire>>,
    dir: std::path::PathBuf,
}

async fn rig(agent: Arc<Agent>, orch: Option<OrchOptions>) -> Rig {
    let dir = tempfile::tempdir().unwrap().keep();
    boot(dir, agent, orch).await
}

async fn boot(dir: std::path::PathBuf, agent: Arc<Agent>, orch: Option<OrchOptions>) -> Rig {
    let registry = HarnessRegistry::new();
    registry.register(agent.clone());
    let core = EngineCore::assemble(&dir.join("data"), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine assembles");
    create_chat(&core).await;
    let wire = orch.map(|o| {
        core.enable_orchestrator_v2(o)
            .expect("orchestrator enables")
    });
    if let Some(w) = &wire {
        forward_changes(w);
    }
    Rig {
        core,
        agent,
        wire,
        dir,
    }
}

async fn create_chat(core: &EngineCore) {
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({"op": "createChat", "chatId": CHAT, "deviceId": core.device_id}),
        )
        .await
        .expect("createChat");
    core.workspace
        .rename_chat(CHAT, "Pre-titled")
        .expect("rename chat");
}

impl Rig {
    fn send(&self, prompt: &str, message_id: &str) {
        self.core
            .doc_host
            .queue_command(
                CHAT,
                SessionCommandPayload::Run {
                    request: request(prompt),
                    message_id: message_id.into(),
                },
            )
            .expect("queue run");
    }

    fn wire(&self) -> &Arc<OrchWire> {
        self.wire.as_ref().expect("orchestrator on")
    }

    fn thread(&self) -> Option<Thread> {
        self.wire().thread_state(CHAT)
    }

    fn statuses(&self) -> Vec<RunStatus> {
        self.thread()
            .map(|t| t.runs.iter().map(|r| r.status).collect())
            .unwrap_or_default()
    }

    async fn wait_statuses(&self, want: &[RunStatus], what: &str) {
        if !until(|| self.statuses() == want).await {
            panic!(
                "timed out waiting for {what}: statuses {:?}, runs {:?}, acts {:?}",
                self.statuses(),
                self.agent.runs(),
                self.acts()
            );
        }
    }

    fn acts(&self) -> Vec<String> {
        let lines = self.wire().trace_lines().expect("trace");
        trace::parse_lines(&lines.join("\n"))
            .expect("parse")
            .into_iter()
            .filter(|l| l.ok && !l.dup)
            .map(|l| l.act)
            .collect()
    }

    fn check_trace(&self) {
        let lines = self.wire().trace_lines().expect("trace");
        let parsed = trace::parse_lines(&lines.join("\n")).expect("parse");
        trace::check(&parsed).unwrap_or_else(|e| panic!("trace check: {e}"));
        if let Some(root) = std::env::var_os("ORCH_TRACE_OUT") {
            let recording =
                std::path::PathBuf::from(root).join(format!("orch_v2-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&recording).expect("create trace recording");
            std::fs::write(
                recording.join(format!("{}.jsonl", self.core.device_id)),
                format!("{}\n", lines.join("\n")),
            )
            .expect("export trace recording");
        }
        if let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") {
            let rows = self.wire().driver().mechanism_lines().expect("mechanism capture");
            assert!(!rows.is_empty(), "ORCH_MECHANISM_OUT requires ORCH_CAPTURE_MECHANISM=1");
            let recording = std::path::PathBuf::from(root).join(format!("orch_v2-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&recording).expect("create mechanism recording");
            std::fs::write(recording.join(format!("{}.jsonl", self.core.device_id)), format!("{}\n", rows.join("\n"))).expect("export mechanism recording");
            std::fs::write(recording.join("capture.json"), serde_json::to_vec(&serde_json::json!({
                "test_name": std::thread::current().name().expect("test thread name"),
                "devices": [self.core.device_id], "filenames": [format!("{}.jsonl", self.core.device_id)],
                "capture": "present"
            })).unwrap()).expect("export capture inventory");
        }
    }

    async fn finish(self) {
        if self.wire.is_some() {
            self.check_trace();
        } else if let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") {
            let recording = std::path::PathBuf::from(root).join(format!("orch_v2-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&recording).expect("create disabled capture inventory");
            std::fs::write(recording.join("capture.json"), serde_json::to_vec(&serde_json::json!({
                "test_name": std::thread::current().name().expect("test thread name"),
                "devices": [], "filenames": [], "capture": "skipped", "reason": "orchestrator_disabled"
            })).unwrap()).expect("export disabled capture inventory");
        }
        let _ = self.agent.finish.send(());
        self.core.shutdown().await;
    }
}

async fn wait_for(predicate: impl FnMut() -> bool, what: &str) {
    assert!(until(predicate).await, "timed out waiting for {what}");
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_runs_through_the_orchestrator_and_traces_each_step() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hello", "m1");
    wait_for(|| r.statuses() == [RunStatus::Completed], "run 1 completes").await;
    assert_eq!(r.agent.runs(), ["hello"]);
    let acts = r.acts();
    for act in [
        "OpenRoot",
        "PersonSend",
        "StartFromQueue",
        "ProviderTerminal",
        "IngestEnd",
    ] {
        assert!(acts.iter().any(|a| a == act), "{act} missing from {acts:?}");
    }
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_message_starts_from_the_orchestrator_queue_after_the_turn() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold first", "m1");
    wait_for(|| r.agent.runs() == ["hold first"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_message(CHAT, "second", Vec::new())
        .expect("queue");
    settle().await;
    assert_eq!(
        r.agent.runs(),
        ["hold first"],
        "the queue waits for the turn"
    );
    let _ = r.agent.finish.send(());
    wait_for(
        || r.statuses() == [RunStatus::Completed, RunStatus::Completed],
        "both runs complete",
    )
    .await;
    assert_eq!(r.agent.runs(), ["hold first", "second"]);
    let starts = r.acts().iter().filter(|a| *a == "StartFromQueue").count();
    assert_eq!(starts, 2);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_boundary_steer_is_a_steer_event_in_the_same_run() {
    let r = rig(
        Agent::new(Some(SteeringMode::StepBoundary)),
        Some(OrchOptions::default()),
    )
    .await;
    r.send("hold first", "m1");
    wait_for(|| r.agent.runs() == ["hold first"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Steer {
                prompt: "nudge".into(),
                message_id: Some("s1".into()),
            },
        )
        .expect("steer");
    wait_for(
        || r.agent.steers.lock().unwrap().as_slice() == ["nudge"],
        "the steer reaches the live turn",
    )
    .await;
    assert!(r.acts().iter().any(|a| a == "Steer"));
    assert_eq!(r.statuses(), [RunStatus::Live]);
    assert_eq!(r.agent.runs(), ["hold first"]);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_boundary_steer_waits_and_starts_the_next_run() {
    let r = rig(
        Agent::new(Some(SteeringMode::TurnBoundary)),
        Some(OrchOptions::default()),
    )
    .await;
    r.send("hold first", "m1");
    wait_for(|| r.agent.runs() == ["hold first"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Steer {
                prompt: "later".into(),
                message_id: Some("s1".into()),
            },
        )
        .expect("steer");
    settle().await;
    assert!(!r.acts().iter().any(|a| a == "Steer"));
    let _ = r.agent.finish.send(());
    let delivered = || r.agent.runs().len() + r.agent.steers.lock().unwrap().len();
    wait_for(|| delivered() == 2, "the held steer reaches the agent").await;
    settle().await;
    assert_eq!(delivered(), 2, "the steer is delivered once");
    assert_eq!(r.statuses().len(), 2, "the held steer is run 2");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_now_restarts_the_active_attempt() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold first", "m1");
    wait_for(|| r.agent.runs() == ["hold first"], "turn 1 starts").await;
    let id = r
        .core
        .doc_host
        .queue_message(CHAT, "urgent", Vec::new())
        .expect("queue");
    assert!(
        r.core
            .doc_host
            .send_queued_now(CHAT, &id)
            .await
            .expect("send now")
    );
    wait_for(
        || r.agent.runs() == ["hold first", "urgent"],
        "the restart runs",
    )
    .await;
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "the one run completes",
    )
    .await;
    let th = r.thread().expect("thread");
    assert_eq!(th.attempt.n, 2, "send now is attempt 2 of the same run");
    assert!(r.acts().iter().any(|a| a == "RestartActive"));
    assert!(r.acts().iter().any(|a| a == "IngestStale"));
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_is_a_request_and_the_run_ends_on_the_provider_done() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold first", "m1");
    wait_for(|| r.agent.runs() == ["hold first"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .expect("interrupt");
    wait_for(
        || r.thread().is_some_and(|t| t.int_req),
        "the interrupt is requested",
    )
    .await;
    assert_eq!(r.statuses(), [RunStatus::Live], "R7: a request, not an end");
    wait_for(
        || r.statuses() == [RunStatus::Interrupted],
        "the provider confirms",
    )
    .await;
    assert_eq!(r.thread().expect("thread").held, Hold::Stop);
    let acts = r.acts();
    let at = |name: &str| acts.iter().position(|a| a == name).expect(name);
    assert!(at("Stop") < at("ProviderTerminal"));
    r.send("again", "m2");
    wait_for(
        || r.statuses().len() == 2,
        "a person send releases the stop",
    )
    .await;
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_made_interrupt_ends_the_run_without_a_stop_hold() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("selfstop", "m1");
    r.wait_statuses(&[RunStatus::Interrupted], "the run ends interrupted")
        .await;
    assert_eq!(r.thread().expect("thread").held, Hold::None);
    let acts = r.acts();
    assert!(acts.iter().any(|a| a == "InterruptChat"), "{acts:?}");
    assert!(!acts.iter().any(|a| a == "Stop"), "{acts:?}");
    r.core
        .doc_host
        .queue_message(CHAT, "after", Vec::new())
        .expect("queue");
    r.wait_statuses(
        &[RunStatus::Interrupted, RunStatus::Completed],
        "the queue is not held",
    )
    .await;
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failure_hold_keeps_earlier_rows_until_the_person_queues_again() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("deny-hold", "m1");
    wait_for(|| r.agent.runs() == ["deny-hold"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_message(CHAT, "earlier", Vec::new())
        .expect("queue");
    settle().await;
    let _ = r.agent.finish.send(());
    r.wait_statuses(&[RunStatus::Failed], "the run fails").await;
    assert_eq!(r.thread().expect("thread").held, Hold::Fail);
    settle().await;
    assert_eq!(
        r.agent.runs(),
        ["deny-hold"],
        "RL2: a fail hold blocks the queue"
    );
    r.core
        .doc_host
        .queue_message(CHAT, "later", Vec::new())
        .expect("queue");
    r.wait_statuses(
        &[
            RunStatus::Failed,
            RunStatus::Completed,
            RunStatus::Completed,
        ],
        "RL1: the new row releases the hold",
    )
    .await;
    assert_eq!(r.agent.runs(), ["deny-hold", "earlier", "later"]);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn done_failure_classes_map_to_run_outcomes() {
    let r = rig(Agent::new(None), Some(options(Duration::from_secs(3600)))).await;
    let cases = [
        ("deny", RunStatus::Failed, Hold::Fail),
        ("invalid", RunStatus::Failed, Hold::Fail),
        ("unknown", RunStatus::Failed, Hold::Fail),
        ("fine", RunStatus::Completed, Hold::None),
        ("limit", RunStatus::UsageLimit, Hold::Limit),
    ];
    for (i, (prompt, status, held)) in cases.iter().enumerate() {
        r.send(prompt, &format!("m{i}"));
        wait_for(
            || r.statuses().len() == i + 1 && r.statuses()[i] != RunStatus::Live,
            prompt,
        )
        .await;
        let th = r.thread().expect("thread");
        assert_eq!(
            (th.runs.last().map(|r| r.status), th.held),
            (Some(*status), *held),
            "{prompt}"
        );
    }
    assert_eq!(r.thread().expect("thread").ul.armed, Some(5));
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transient_failure_retries_within_the_budget() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("flaky2", "m1");
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "the retried run completes",
    )
    .await;
    assert_eq!(r.agent.runs(), ["flaky2", "flaky2", "flaky2"]);
    assert_eq!(r.thread().expect("thread").attempt.n, 3);
    r.send("flaky9", "m2");
    wait_for(
        || r.statuses().len() == 2 && r.statuses()[1] == RunStatus::Failed,
        "budget spent",
    )
    .await;
    assert_eq!(r.agent.runs().len(), 6, "one try plus max_retry 2 retries");
    assert_eq!(r.acts().iter().filter(|a| *a == "RetryAttempt").count(), 4);
    r.finish().await;
}

fn ticking(every: Duration) -> OrchOptions {
    OrchOptions {
        reconcile_every: every,
        ..OrchOptions::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_done_is_reconciled_on_the_timer_and_the_queue_moves_on() {
    let r = rig(Agent::new(None), Some(ticking(Duration::from_millis(100)))).await;
    r.wire().lose_next_done(CHAT);
    r.send("hello", "m1");
    wait_for(|| r.agent.runs() == ["hello"], "the run starts").await;
    r.wait_statuses(
        &[RunStatus::Completed],
        "the tick ends the run from the journal",
    )
    .await;
    r.core
        .doc_host
        .queue_message(CHAT, "next", Vec::new())
        .expect("queue");
    r.wait_statuses(
        &[RunStatus::Completed, RunStatus::Completed],
        "the queue drains",
    )
    .await;
    assert_eq!(r.agent.runs(), ["hello", "next"], "no prompt ran twice");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_done_is_reconciled_when_the_engine_starts() {
    let agent = Agent::new(None);
    let slow = ticking(Duration::from_secs(3600));
    let r = rig(agent.clone(), Some(slow.clone())).await;
    r.wire().lose_next_done(CHAT);
    r.send("hello", "m1");
    wait_for(|| r.agent.runs() == ["hello"], "the run starts").await;
    settle().await;
    assert_eq!(r.statuses(), [RunStatus::Live], "the Done was lost");
    let dir = r.dir.clone();
    r.core.shutdown().await;
    drop(r);
    let r = boot(dir, agent, Some(slow)).await;
    r.wait_statuses(&[RunStatus::Completed], "startup reconcile ends the run")
        .await;
    assert_eq!(r.agent.runs(), ["hello"]);
    r.finish().await;
}

fn fast_retries() -> OrchOptions {
    OrchOptions {
        retry_delays: vec![Duration::from_millis(20)],
        ..OrchOptions::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_on_the_orchestrator_alone_retries_a_transient_start_failure() {
    let r = rig(Agent::new(None), Some(fast_retries())).await;
    r.send("busy", "m1");
    r.wait_statuses(&[RunStatus::Failed], "the retries run out")
        .await;
    settle().await;
    assert_eq!(
        r.agent.runs().len(),
        3,
        "one try plus max_retry 2, with no engine spawn retries stacked under each"
    );
    assert_eq!(r.acts().iter().filter(|a| *a == "RetryAttempt").count(), 2);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_off_the_engine_retries_a_transient_start_failure() {
    let r = rig(Agent::new(None), None).await;
    r.send("busy", "m1");
    wait_for(|| r.agent.runs().len() == 4, "one try plus 3 spawn retries").await;
    settle().await;
    assert_eq!(r.agent.runs().len(), 4);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_usage_limit_resumes_at_reset_and_then_sends_the_held_person_message() {
    let r = rig(Agent::new(None), Some(options(Duration::from_millis(500)))).await;
    r.send("limit-hold", "m1");
    wait_for(|| r.agent.runs() == ["limit-hold"], "turn 1 starts").await;
    r.core
        .doc_host
        .queue_message(CHAT, "after", Vec::new())
        .expect("queue");
    settle().await;
    let _ = r.agent.finish.send(());
    wait_for(
        || r.thread().is_some_and(|t| t.held == Hold::Limit),
        "the limit holds the queue",
    )
    .await;
    settle().await;
    assert_eq!(
        r.agent.runs(),
        ["limit-hold"],
        "D6: the person message waits"
    );
    wait_for(
        || r.agent.runs() == ["limit-hold", RESUME_PROMPT, "after"],
        "resume, then the held message",
    )
    .await;
    r.wait_statuses(
        &[
            RunStatus::UsageLimit,
            RunStatus::Completed,
            RunStatus::Completed,
        ],
        "all three runs end",
    )
    .await;
    let acts = r.acts();
    for act in ["ResetPasses", "UsageLimitRecovery"] {
        assert!(acts.iter().any(|a| a == act), "{act} missing");
    }
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parked_runtime_takes_each_next_run_through_its_mailbox() {
    let r = rig(Agent::parking(), Some(OrchOptions::default())).await;
    r.send("first", "m1");
    r.wait_statuses(&[RunStatus::Completed], "run 1 ends").await;
    r.send("second", "m2");
    r.wait_statuses(&[RunStatus::Completed, RunStatus::Completed], "run 2 ends")
        .await;
    r.core
        .doc_host
        .queue_message(CHAT, "third", Vec::new())
        .expect("queue");
    r.wait_statuses(&[RunStatus::Completed; 3], "run 3 ends")
        .await;
    assert_eq!(
        r.agent.steers.lock().unwrap().as_slice(),
        ["second"],
        "run 2 went through the parked runtime's mailbox"
    );
    assert_eq!(
        r.agent.runs(),
        ["first", "third"],
        "the queue drain's chat-row request replaces the runtime, as with the flag off"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_usage_limit_timer_is_rearmed_after_a_restart() {
    let agent = Agent::new(None);
    let r = rig(agent.clone(), Some(options(Duration::from_millis(1500)))).await;
    r.send("limit", "m1");
    wait_for(
        || r.statuses() == [RunStatus::UsageLimit],
        "the limit run ends",
    )
    .await;
    let dir = r.dir.clone();
    r.finish().await;
    let r = boot(dir, agent, Some(options(Duration::from_millis(1500)))).await;
    wait_for(
        || r.agent.runs() == ["limit", RESUME_PROMPT],
        "the rearmed timer resumes",
    )
    .await;
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_usage_limit_with_a_reset_time_resumes_at_that_time_not_the_fallback_delay() {
    let r = rig(Agent::new(None), Some(options(Duration::from_secs(3600)))).await;
    let sent = std::time::Instant::now();
    r.send("limit-at", "m1");
    wait_for(
        || r.agent.runs() == ["limit-at", RESUME_PROMPT],
        "the reset time resumes the run",
    )
    .await;
    assert!(
        sent.elapsed() >= Duration::from_millis(1500),
        "resumed before the reset: {:?}",
        sent.elapsed()
    );
    r.wait_statuses(
        &[RunStatus::UsageLimit, RunStatus::Completed],
        "both runs end",
    )
    .await;
    r.finish().await;
}

async fn crash_then_restart(at: FaultPoint, skip: usize, prompt: &str) -> Rig {
    let agent = Agent::new(None);
    let r = rig(agent.clone(), Some(OrchOptions::default())).await;
    r.wire().driver().crash_at(at, skip);
    r.send(prompt, "m1");
    wait_for(|| r.wire().is_dead(), "the fault fires").await;
    settle().await;
    let dir = r.dir.clone();
    r.core.shutdown().await;
    drop(r);
    boot(dir, agent, Some(OrchOptions::default())).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fp1_a_committed_start_with_no_effect_replays_on_restart() {
    let r = crash_then_restart(FaultPoint::Fp1StepCommittedEffectNotRun, 2, "hello").await;
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "the replayed start runs",
    )
    .await;
    assert_eq!(r.agent.runs(), ["hello"], "exactly one provider run");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fp2_an_effect_that_ran_but_was_not_marked_does_not_run_twice() {
    let r = crash_then_restart(FaultPoint::Fp2EffectRunNotMarked, 0, "hello").await;
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "recovery ends the run",
    )
    .await;
    settle().await;
    assert_eq!(r.agent.runs(), ["hello"], "exactly one provider run");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fp3_a_rolled_back_send_leaves_nothing_to_replay() {
    let r = crash_then_restart(FaultPoint::Fp3ReceivedNoReceipt, 1, "hello").await;
    settle().await;
    assert!(r.agent.runs().is_empty(), "the rolled-back send never ran");
    assert!(r.statuses().is_empty());
    r.send("again", "m2");
    wait_for(|| r.statuses() == [RunStatus::Completed], "a new send runs").await;
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fp4_a_receipted_command_without_a_ledger_mark_is_applied_not_run_again() {
    let r = crash_then_restart(FaultPoint::Fp4ReceiptNotMarked, 0, "hello").await;
    let statuses = || {
        r.core
            .doc_host
            .open(CHAT)
            .expect("open")
            .doc()
            .read_commands()
            .expect("commands")
            .iter()
            .map(|c| c.status)
            .collect::<Vec<_>>()
    };
    wait_for(
        || statuses() == [zeron_doc::SessionCommandStatus::Applied],
        "recovery marks the receipted command applied",
    )
    .await;
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "the run ends once",
    )
    .await;
    settle().await;
    assert_eq!(r.agent.runs(), ["hello"], "exactly one provider run");
    assert!(
        r.acts().iter().filter(|a| *a == "PersonSend").count() == 1,
        "the command applied once"
    );
    r.finish().await;
}

async fn duplicate_attempts(orch: Option<OrchOptions>) -> Vec<String> {
    let r = rig(Agent::new(None), orch).await;
    r.send("once", "m1");
    wait_for(|| r.agent.runs().len() == 1, "the first attempt runs").await;
    settle().await;
    r.send("once", "m1");
    settle().await;
    settle().await;
    let runs = r.agent.runs();
    r.finish().await;
    runs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r17_flag_off_a_second_attempt_for_one_message_runs_the_provider_twice() {
    assert_eq!(duplicate_attempts(None).await, ["once", "once"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r17_flag_on_a_second_attempt_for_one_message_is_refused() {
    assert_eq!(
        duplicate_attempts(Some(OrchOptions::default())).await,
        ["once"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r17_concurrent_dispatches_of_one_message_id_start_one_run() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    let a = r.core.sessions.dispatch(
        CHAT,
        HarnessId::Mock,
        request("hold twin"),
        Some("m1".into()),
    );
    let b = r.core.sessions.dispatch(
        CHAT,
        HarnessId::Mock,
        request("hold twin"),
        Some("m1".into()),
    );
    let (a, b) = tokio::join!(a, b);
    a.expect("first");
    b.expect("second");
    settle().await;
    assert_eq!(r.agent.runs(), ["hold twin"]);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r18_a_landed_message_with_no_receipt_is_not_run_again() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("once", "m1");
    wait_for(
        || r.statuses() == [RunStatus::Completed],
        "the first run ends",
    )
    .await;
    r.wire()
        .driver()
        .delete_meta("ran:m1")
        .expect("drop marker");
    r.send("once", "m1");
    settle().await;
    settle().await;
    assert_eq!(r.agent.runs(), ["once"], "the doc gate refuses it");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_without_a_registry_row_is_not_orchestrated() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.core
        .sessions
        .dispatch(
            "no-row",
            HarnessId::Mock,
            request("stray"),
            Some("x1".into()),
        )
        .await
        .expect("legacy dispatch");
    wait_for(|| r.agent.runs() == ["stray"], "the legacy path runs it").await;
    assert!(r.wire().thread_state("no-row").is_none());
    r.finish().await;
}

fn child_config() -> serde_json::Value {
    serde_json::to_value(zeron_proto::ChatConfig {
        harness: HarnessId::Mock,
        model: Some("mock-1".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
        access: Default::default(),
        plan: false,
    })
    .unwrap()
}

impl Rig {
    async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        zeron_rpc::memory_client(self.core.rpc_service())
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    async fn delegate(
        &self,
        origin: &str,
        crid: &str,
        task: &str,
        mode: &str,
    ) -> Result<serde_json::Value, String> {
        self.delegate_with(origin, crid, task, mode, None).await
    }

    async fn delegate_with(
        &self,
        origin: &str,
        crid: &str,
        task: &str,
        mode: &str,
        timeout_ms: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({
            "originChatId": origin,
            "task": task,
            "title": crid,
            "mode": mode,
            "timeoutMs": timeout_ms,
            "clientRequestId": crid,
            "observedInput": {"mode":mode,"clientRequestId":crid},
            "child": {
                "chatId": uuid::Uuid::new_v4().to_string(),
                "deviceId": self.core.device_id,
                "config": child_config(),
                "cwd": "/tmp",
            },
        });
        self.rpc(zeron_rpc::methods::ORCH_DELEGATE, params).await
    }

    fn chat_statuses(&self, chat: &str) -> Vec<RunStatus> {
        self.wire()
            .thread_state(chat)
            .map(|t| t.runs.iter().map(|r| r.status).collect())
            .unwrap_or_default()
    }

    fn wake_runs(&self) -> Vec<String> {
        self.agent
            .runs()
            .into_iter()
            .filter(|p| p.starts_with("Task results"))
            .collect()
    }

    fn check_traces(&self) {
        self.check_trace();
    }

    fn diag(&self) -> String {
        let acts = self.acts();
        let tail = &acts[acts.len().saturating_sub(40)..];
        let threads: Vec<_> = self
            .wire()
            .driver()
            .state()
            .map(|s| {
                s.threads
                    .iter()
                    .map(|(_, t)| {
                        let runs: Vec<_> = t.runs.iter().map(|r| r.status).collect();
                        (t.id.0, runs, t.held, t.task)
                    })
                    .collect()
            })
            .unwrap_or_default();
        format!(
            "runs {:?}, acts {tail:?}, threads {threads:?}",
            self.agent.runs()
        )
    }

    async fn wait(&self, predicate: impl FnMut() -> bool, what: &str) {
        if !until(predicate).await {
            panic!("timed out waiting for {what}: {}", self.diag());
        }
    }
}

fn child_of(view: &serde_json::Value) -> String {
    view["childThreadId"]
        .as_str()
        .expect("childThreadId")
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f2_a_child_result_is_pushed_to_the_parent_as_a_wake_run() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    *r.agent.projection_store.lock().unwrap() =
        Some(r.core.uploads.dir().parent().unwrap().to_path_buf());
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "c1", "quick", "async")
        .await
        .expect("delegate");
    if view["status"] == "running" {
        assert_eq!(view["next"], zeron_engine::orchestrator::wire::RUNNING_NEXT);
    } else {
        assert_eq!(view["status"], "completed", "{view}");
    }
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "the child runs its task",
    )
    .await;
    settle().await;
    assert!(r.wake_runs().is_empty(), "S4: no wake into a live parent");
    let _ = r.agent.finish.send(());
    r.wait(|| r.wake_runs().len() == 1, "the wake run starts")
        .await;
    assert!(
        r.wake_runs()[0].contains("result of quick"),
        "{:?}",
        r.wake_runs()
    );
    let entries = r
        .core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    let projected: Vec<_> = entries
        .iter()
        .flat_map(|e| &e.parts)
        .filter_map(|part| {
            if let zeron_doc::MessagePart::TaskResults { results, .. } = part {
                Some(results)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(projected.len(), 1, "one structured task-results message");
    assert_eq!(projected[0][0].task_id, view["taskId"]);
    assert_eq!(projected[0][0].text, "result of quick");
    assert!(
        r.agent
            .projection_seen
            .load(std::sync::atomic::Ordering::SeqCst),
        "fresh disk snapshot contains TaskResults before provider start"
    );
    let acts = r.acts();
    for act in ["Delegate", "DoNotice", "DoOffer", "StartFromQueueWake"] {
        assert!(acts.iter().any(|a| a == act), "{act} missing from {acts:?}");
    }
    assert!(!acts.iter().any(|a| a == "ReadResult"), "F2: no read call");
    let status = r
        .rpc(
            zeron_rpc::methods::ORCH_TASK_STATUS,
            serde_json::json!({"originChatId": CHAT, "taskId": view["taskId"]}),
        )
        .await
        .expect("status");
    assert_eq!(status["status"], "completed");
    assert!(status.get("next").is_none());
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f3_results_that_settle_while_the_parent_is_busy_share_one_wake() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let a = child_of(&r.delegate(CHAT, "a", "alpha", "async").await.expect("a"));
    let b = child_of(&r.delegate(CHAT, "b", "beta", "async").await.expect("b"));
    r.wait(
        || {
            r.chat_statuses(&a) == [RunStatus::Completed]
                && r.chat_statuses(&b) == [RunStatus::Completed]
        },
        "both children end",
    )
    .await;
    settle().await;
    let _ = r.agent.finish.send(());
    r.wait(|| r.wake_runs().len() == 1, "one wake run").await;
    settle().await;
    let wakes = r.wake_runs();
    assert_eq!(wakes.len(), 1, "F3: one coalesced wake");
    assert!(
        wakes[0].contains("result of alpha") && wakes[0].contains("result of beta"),
        "{wakes:?}"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f4_s7_a_wait_returns_the_result_and_an_observed_reply_acknowledges_it() {
    let r = rig(
        Agent::new(Some(SteeringMode::StepBoundary)),
        Some(OrchOptions::default()),
    )
    .await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "w1", "waited", "wait")
        .await
        .expect("wait");
    assert_eq!(view["status"], "completed");
    assert_eq!(view["summary"], "result of waited");
    assert_eq!(view["waitTimedOut"], false);
    assert!(view.get("next").is_none());
    r.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Steer {
                prompt: "ack w1".into(),
                message_id: Some("s1".into()),
            },
        )
        .expect("steer");
    r.wait(
        || r.acts().iter().any(|a| a == "WaitReturn"),
        "S7: the observed reply acks",
    )
    .await;
    let _ = r.agent.finish.send(());
    r.wait_statuses(&[RunStatus::Completed], "root ends").await;
    settle().await;
    assert!(
        r.wake_runs().is_empty(),
        "an acknowledged result never wakes again"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r1_r2_an_unobserved_wait_result_is_swept_into_a_wake_at_run_end() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "w2", "waited", "wait")
        .await
        .expect("wait");
    assert_eq!(view["status"], "completed");
    let _ = r.agent.finish.send(());
    r.wait(
        || r.wake_runs().len() == 1,
        "R1: the sweep wakes the parent",
    )
    .await;
    let acts = r.acts();
    assert!(acts.iter().any(|a| a == "WaitAbandon"), "{acts:?}");
    assert!(
        !acts.iter().any(|a| a == "WaitReturn"),
        "R2: no ack without an observed reply"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_that_times_out_keeps_the_child_and_says_so() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate_with(CHAT, "slow", "hold slow", "wait", Some(300))
        .await
        .expect("wait");
    assert_eq!(view["waitTimedOut"], true, "{view} {:?}", r.acts());
    assert_eq!(view["status"], "running");
    assert_eq!(
        view["next"],
        "Still running. The result is pushed to you when it settles; do not poll."
    );
    assert!(r.acts().iter().any(|a| a == "WaitTimeout"));
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f1_a_grandchild_result_reaches_the_child_and_the_child_result_reaches_the_root() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "c", "hold child", "async")
            .await
            .expect("child"),
    );
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "the child runs",
    )
    .await;
    let grand = r
        .delegate(&child, "g", "grand", "wait")
        .await
        .expect("grandchild");
    assert_eq!(grand["status"], "completed");
    assert_eq!(grand["summary"], "result of grand");
    let deep = r
        .delegate(&child_of(&grand), "too-deep", "x", "async")
        .await;
    assert!(
        deep.is_err(),
        "S5: the grandchild is not live, and depth 3 is capped"
    );
    let _ = r.agent.finish.send(());
    r.wait(
        || {
            r.wake_runs().iter().any(|w| {
                w.split("\n## ")
                    .any(|b| b.starts_with("c (completed)") && b.contains("result of"))
            })
        },
        "the child's result wakes the root",
    )
    .await;
    r.check_traces();
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s5_depth_and_r10_fanout_caps_count_unsettled_tasks() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "d1", "hold d1", "async")
            .await
            .expect("d1"),
    );
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "the child runs",
    )
    .await;
    let grand = child_of(
        &r.delegate(&child, "d2", "hold d2", "async")
            .await
            .expect("d2"),
    );
    r.wait(
        || r.chat_statuses(&grand) == [RunStatus::Live],
        "the grandchild runs",
    )
    .await;
    let deep = r
        .delegate(&grand, "d3", "x", "async")
        .await
        .expect_err("depth 3");
    assert!(deep.contains("capability_denied"), "{deep}");
    for i in 0..7 {
        r.delegate(CHAT, &format!("f{i}"), &format!("hold f{i}"), "async")
            .await
            .expect("fan-out under 8");
    }
    let full = r.delegate(CHAT, "f8", "x", "async").await;
    let full = full.map(|v| panic!("ninth: {v} {}", r.diag())).unwrap_err();
    assert!(full.contains("capability_denied"), "{full}");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f8_s11a_a_person_stop_cascades_to_every_descendant() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "s1", "hold s1", "async")
            .await
            .expect("child"),
    );
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "the child runs",
    )
    .await;
    let grand = child_of(
        &r.delegate(&child, "s2", "hold s2", "async")
            .await
            .expect("grand"),
    );
    r.wait(
        || r.chat_statuses(&grand) == [RunStatus::Live],
        "the grandchild runs",
    )
    .await;
    r.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .expect("stop");
    for (chat, what) in [
        (CHAT, "root"),
        (child.as_str(), "child"),
        (grand.as_str(), "grandchild"),
    ] {
        r.wait(
            || r.chat_statuses(chat) == [RunStatus::Interrupted],
            &format!("{what} is interrupted"),
        )
        .await;
    }
    for chat in [CHAT, child.as_str(), grand.as_str()] {
        assert_eq!(
            r.wire().thread_state(chat).expect("thread").held,
            Hold::Stop
        );
    }
    let acts = r.acts();
    assert_eq!(
        acts.iter().filter(|a| *a == "DoStop").count(),
        2,
        "{acts:?}"
    );
    let refused = r
        .delegate(CHAT, "after", "x", "async")
        .await
        .expect_err("S11a");
    assert!(refused.contains("parent_not_active"), "{refused}");
    settle().await;
    assert!(
        r.wake_runs().is_empty(),
        "stopped cohorts dispose their results"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r21_a_failure_hold_does_not_strand_a_result_wake() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("deny-hold root", "m1");
    r.wait(|| r.agent.runs() == ["deny-hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "r21", "quick", "async")
            .await
            .expect("child"),
    );
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "the child ends",
    )
    .await;
    let _ = r.agent.finish.send(());
    r.wait(|| r.wake_runs().len() == 1, "the wake passes the fail hold")
        .await;
    assert_eq!(r.chat_statuses(CHAT)[0], RunStatus::Failed);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10_a_repeated_client_request_id_returns_the_first_task() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let first = r
        .delegate(CHAT, "same", "once", "async")
        .await
        .expect("first");
    let again = r
        .delegate(CHAT, "same", "once", "async")
        .await
        .expect("again");
    assert_eq!(first["taskId"], again["taskId"]);
    settle().await;
    assert_eq!(r.agent.runs().iter().filter(|p| *p == "once").count(), 1);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_cancel_stops_a_running_child() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "k", "hold k", "async")
        .await
        .expect("child");
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "the child runs",
    )
    .await;
    let out = r
        .rpc(
            zeron_rpc::methods::ORCH_TASK_CANCEL,
            serde_json::json!({"originChatId": CHAT, "taskId": view["taskId"]}),
        )
        .await
        .expect("cancel");
    assert_eq!(out["status"], "cancel_requested");
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Interrupted],
        "the child stops",
    )
    .await;
    assert_eq!(
        r.chat_statuses(CHAT),
        [RunStatus::Live],
        "the parent keeps running"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_off_delegation_is_refused() {
    let r = rig(Agent::new(None), None).await;
    let err = r
        .delegate(CHAT, "x", "x", "async")
        .await
        .expect_err("flag off");
    assert!(err.contains("capability_denied"), "{err}");
    r.finish().await;
}

async fn restart(r: Rig) -> Rig {
    let (dir, agent) = (r.dir.clone(), r.agent.clone());
    r.core.shutdown().await;
    drop(r);
    boot(dir, agent, Some(OrchOptions::default())).await
}

async fn delegate_then_crash(at: FaultPoint) -> (Rig, bool) {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    r.wire().driver().crash_at(at, 0);
    let placed = r.delegate(CHAT, "fp", "quick", "async").await.is_ok();
    r.wait(|| r.wire().is_dead(), "the fault fires").await;
    settle().await;
    (restart(r).await, placed)
}

fn quick_runs(r: &Rig) -> usize {
    r.agent.runs().iter().filter(|p| *p == "quick").count()
}

fn root_settled_after_restart(r: &Rig) -> bool {
    let retried = r.agent.runs().iter().filter(|p| *p == "hold root").count() == 2;
    retried || r.statuses().last().is_some_and(|s| *s != RunStatus::Live)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fp1_fp2_a_crash_after_delegate_commits_runs_the_child_once_and_wakes_the_root() {
    for at in [
        FaultPoint::Fp1StepCommittedEffectNotRun,
        FaultPoint::Fp2EffectRunNotMarked,
    ] {
        let (r, _) = delegate_then_crash(at).await;
        r.wait(|| quick_runs(&r) == 1, "recovery starts the child")
            .await;
        r.wait(
            || root_settled_after_restart(&r),
            "recovery handles the root",
        )
        .await;
        let _ = r.agent.finish.send(());
        r.wait(
            || r.wake_runs().iter().any(|w| w.contains("result of quick")),
            "the child's result wakes the root",
        )
        .await;
        settle().await;
        assert_eq!(quick_runs(&r), 1, "{at:?}: the child ran once");
        assert_eq!(r.wake_runs().len(), 1, "{at:?}: one wake");
        r.finish().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fp3_a_rolled_back_delegate_places_nothing_and_a_retry_places_it_once() {
    let (r, placed) = delegate_then_crash(FaultPoint::Fp3ReceivedNoReceipt).await;
    assert!(!placed, "the rolled-back delegate reports an error");
    r.wait(
        || root_settled_after_restart(&r),
        "recovery handles the root",
    )
    .await;
    settle().await;
    assert_eq!(quick_runs(&r), 0, "no child ran");
    let view = r
        .delegate(CHAT, "fp", "quick", "async")
        .await
        .expect("retry");
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "the retried child runs",
    )
    .await;
    assert_eq!(quick_runs(&r), 1);
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fp4_a_stop_receipted_but_not_marked_still_cascades_once_after_restart() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "k", "hold k", "async")
            .await
            .expect("child"),
    );
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "the child runs",
    )
    .await;
    r.wire()
        .driver()
        .crash_at(FaultPoint::Fp4ReceiptNotMarked, 0);
    r.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .expect("stop");
    r.wait(|| r.wire().is_dead(), "the fault fires").await;
    settle().await;
    let r = restart(r).await;
    for (chat, what) in [(CHAT, "root"), (child.as_str(), "child")] {
        r.wait(
            || {
                r.chat_statuses(chat)
                    .last()
                    .is_some_and(|s| *s != RunStatus::Live)
            },
            &format!("{what} ends"),
        )
        .await;
    }
    settle().await;
    for chat in [CHAT, child.as_str()] {
        assert_eq!(
            r.wire().thread_state(chat).expect("thread").held,
            Hold::Stop
        );
    }
    let acts = r.acts();
    assert_eq!(acts.iter().filter(|a| *a == "Stop").count(), 1, "{acts:?}");
    assert_eq!(
        acts.iter().filter(|a| *a == "DoStop").count(),
        1,
        "{acts:?}"
    );
    assert!(
        r.wake_runs().is_empty(),
        "a stopped cohort never wakes the root"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s7_forged_server_and_request_cannot_ack_a_wait() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    r.delegate(CHAT, "bound", "quick", "wait")
        .await
        .expect("wait");
    for (server, crid, changed) in [
        ("other", "bound", false),
        ("zeron", "wrong", false),
        ("zeron", "bound", true),
    ] {
        let mut input = serde_json::json!({"mode":"wait", "clientRequestId":crid});
        if changed {
            input["task"] = serde_json::json!("changed input");
        }
        r.wire().tool_event(
            CHAT,
            &AgentEvent::ToolCall {
                id: "forged".into(),
                call: zeron_proto::ToolCall::Mcp {
                    server: server.into(),
                    tool: "delegate_task".into(),
                    input: Some(input),
                },
            },
        );
        r.wire().tool_event(
            CHAT,
            &AgentEvent::ToolResult {
                id: "forged".into(),
                is_error: false,
                output: None,
                diff: None,
                exit_code: None,
            },
        );
        assert!(r.wire().thread_state(CHAT).unwrap().wait_on.is_some());
    }
    let _ = r.agent.finish.send(());
    r.wait(|| r.wake_runs().len() == 1, "unobserved result wakes")
        .await;
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s6_access_narrowing_restarts_live_descendants_with_the_new_ceiling() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let child = child_of(
        &r.delegate(CHAT, "access", "hold child", "async")
            .await
            .unwrap(),
    );
    r.wait(|| r.agent.runs().len() == 2, "child starts").await;
    r.wire()
        .narrow_access(&r.core.sessions, CHAT, zeron_proto::AccessMode::Supervised)
        .await
        .unwrap();
    r.wait(|| r.agent.runs().len() == 4, "both attempts restart")
        .await;
    let access = r.agent.accesses.lock().unwrap().clone();
    assert_eq!(&access[2..], &[zeron_proto::AccessMode::Supervised; 2]);
    assert_eq!(r.wire().thread_state(&child).unwrap().access, 0);
    let denied = r
        .delegate(CHAT, "wider", "quick", "async")
        .await
        .unwrap_err();
    assert!(
        denied.contains("runtime_mode_escalation_denied"),
        "{denied}"
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s12_worktree_isolation_runs_the_child_in_a_fresh_checkout() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    let repo = r.dir.join("fixture-repo");
    std::fs::create_dir_all(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r.rpc(zeron_rpc::methods::ORCH_DELEGATE, serde_json::json!({
        "originChatId":CHAT,"task":"isolated","mode":"async","clientRequestId":"isolate",
        "child":{"chatId":"isolated-child","deviceId":r.core.device_id,"config":child_config(),
            "cwd":repo.to_str().unwrap(),"worktree":{"repoPath":repo.to_str().unwrap(),"base":"main"}}
    })).await.unwrap();
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "isolated child completes",
    )
    .await;
    let folders = r.agent.folders.lock().unwrap().clone();
    assert_ne!(std::path::Path::new(&folders[1]), repo);
    let checkout = std::path::Path::new(&folders[1]);
    assert!(checkout.join(".git").is_file(), "{folders:?}");
    assert!(repo.join(".git").is_dir());
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10_sender_scope_overrides_forged_origin_on_status_and_delegate() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "auth", "hold child", "async")
        .await
        .unwrap();
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Live],
        "child starts",
    )
    .await;
    let service = r.core.rpc_service();
    let status = zeron_rpc::with_caller(
        zeron_rpc::Caller::Agent {
            chat: Some(child.clone()),
        },
        service.handle(
            zeron_rpc::methods::ORCH_TASK_STATUS,
            serde_json::json!({"originChatId":CHAT,"taskId":view["taskId"]}),
        ),
    )
    .await;
    assert!(
        status
            .err()
            .expect("forged origin refused")
            .to_string()
            .contains("task_not_found")
    );
    let no_chat = zeron_rpc::with_caller(
        zeron_rpc::Caller::Agent { chat: None },
        service.handle(
            zeron_rpc::methods::ORCH_DELEGATE,
            serde_json::json!({"originChatId":CHAT}),
        ),
    )
    .await;
    assert!(
        no_chat
            .err()
            .expect("missing caller refused")
            .to_string()
            .contains("Only an agent")
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r36_an_old_attempt_tool_reply_cannot_ack_a_result() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let view = r
        .delegate(CHAT, "old-call", "quick", "async")
        .await
        .unwrap();
    let child = child_of(&view);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "child ends",
    )
    .await;
    r.wire().tool_event(
        CHAT,
        &AgentEvent::ToolCall {
            id: "old-attempt".into(),
            call: zeron_proto::ToolCall::Mcp {
                server: "zeron".into(),
                tool: "task_status".into(),
                input: Some(serde_json::json!({"taskId":view["taskId"]})),
            },
        },
    );
    let before = r.wire().thread_state(CHAT).unwrap().attempt_id();
    r.wire()
        .narrow_access(&r.core.sessions, CHAT, zeron_proto::AccessMode::Supervised)
        .await
        .unwrap();
    r.wait(
        || r.wire().thread_state(CHAT).unwrap().attempt_id() != before,
        "new attempt starts",
    )
    .await;
    r.wire().tool_event(
        CHAT,
        &AgentEvent::ToolResult {
            id: "old-attempt".into(),
            is_error: false,
            output: None,
            diff: None,
            exit_code: None,
        },
    );
    assert!(!r.acts().iter().any(|a| a == "ReadResult"));
    assert_ne!(
        r.wire()
            .thread_state(&child)
            .unwrap()
            .task
            .unwrap()
            .delivery,
        zeron_orch::Delivery::Acknowledged
    );
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10_idempotency_receipts_survive_engine_reopen() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let first = r
        .delegate(CHAT, "reopen-id", "once", "async")
        .await
        .unwrap();
    let child = child_of(&first);
    r.wait(
        || r.chat_statuses(&child) == [RunStatus::Completed],
        "child completes",
    )
    .await;
    let r = restart(r).await;
    let again = r
        .delegate(CHAT, "reopen-id", "ignored duplicate", "async")
        .await
        .unwrap();
    assert_eq!(first["taskId"], again["taskId"]);
    assert_eq!(r.agent.runs().iter().filter(|p| *p == "once").count(), 1);
    assert!(!r.agent.runs().iter().any(|p| p == "ignored duplicate"));
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejecting_a_reused_child_id_preserves_its_retry_prompt() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let first = r
        .delegate(CHAT, "original", "hold child", "async")
        .await
        .unwrap();
    let child = child_of(&first);
    let failed = r.rpc(zeron_rpc::methods::ORCH_DELEGATE, serde_json::json!({
        "originChatId":CHAT,"task":"changed retry prompt","mode":"async","clientRequestId":"different-id",
        "child":{"chatId":child,"deviceId":r.core.device_id,"config":child_config(),"cwd":"/tmp"}
    })).await;
    assert!(failed.is_err());
    let json = r
        .wire()
        .driver()
        .meta(&format!("child:{child}"))
        .unwrap()
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(spec["task"], "hold child");
    r.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r25_followups_reuse_the_child_and_keep_old_results_and_receipts() {
    let r = rig(Agent::new(None), Some(OrchOptions::default())).await;
    r.send("hold root", "m1");
    r.wait(|| r.agent.runs() == ["hold root"], "root starts")
        .await;
    let first = r
        .delegate(CHAT, "follow-first", "first result", "async")
        .await
        .unwrap();
    let child = child_of(&first);
    for (old, prompt, id, want) in [
        (&first, "second result", "follow-second", 2),
        (
            &serde_json::json!({"taskId":format!("{child}:2")}),
            "third result",
            "follow-third",
            3,
        ),
    ] {
        r.wait(
            || {
                r.wire().thread_state(&child).unwrap().task.unwrap().st
                    == zeron_orch::TaskSt::Settled
            },
            "prior task settles",
        )
        .await;
        r.wire().tool_event(
            CHAT,
            &AgentEvent::ToolCall {
                id: "read-followup".into(),
                call: zeron_proto::ToolCall::Mcp {
                    server: "zeron".into(),
                    tool: "task_status".into(),
                    input: Some(serde_json::json!({"taskId":old["taskId"]})),
                },
            },
        );
        r.wire().tool_event(
            CHAT,
            &AgentEvent::ToolResult {
                id: "read-followup".into(),
                is_error: false,
                output: None,
                diff: None,
                exit_code: None,
            },
        );
        let next = r
            .rpc(
                zeron_rpc::methods::ORCH_DELEGATE,
                serde_json::json!({
                    "originChatId":CHAT,"task":prompt,"followup":true,"clientRequestId":id,
                    "child":{"chatId":child,"config":child_config()}
                }),
            )
            .await
            .unwrap();
        assert_eq!(next["taskId"], format!("{child}:{want}"));
    }
    r.wait(
        || r.wire().thread_state(&child).unwrap().task.unwrap().st == zeron_orch::TaskSt::Settled,
        "third task settles",
    )
    .await;
    let first_read = r
        .rpc(
            zeron_rpc::methods::ORCH_TASK_STATUS,
            serde_json::json!({"originChatId":CHAT,"taskId":first["taskId"]}),
        )
        .await
        .unwrap();
    assert_eq!(first_read["summary"], "result of first result");
    let replay = r
        .delegate(CHAT, "follow-first", "ignored", "async")
        .await
        .unwrap();
    assert_eq!(replay["taskId"], first["taskId"]);
    let second_replay = r.rpc(zeron_rpc::methods::ORCH_DELEGATE, serde_json::json!({
        "originChatId":CHAT,"task":"ignored","followup":true,"clientRequestId":"follow-second",
        "child":{"chatId":child,"config":child_config()}
    })).await.unwrap();
    assert_eq!(second_replay["taskId"], format!("{child}:2"));
    assert_eq!(
        r.agent
            .runs()
            .iter()
            .filter(|p| *p == "second result")
            .count(),
        1
    );
    r.finish().await;
}
