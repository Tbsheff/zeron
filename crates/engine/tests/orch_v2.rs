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

const CHAT: &str = "chat-orch";

#[derive(Clone, Copy)]
enum Plan {
    Done(DoneStatus, Option<FailureClass>),
    Hold(DoneStatus, Option<FailureClass>),
}

struct Agent {
    steering: Option<SteeringMode>,
    finish: tokio::sync::broadcast::Sender<()>,
    runs: Arc<Mutex<Vec<String>>>,
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
            "limit-hold" => Plan::Hold(Errored, Some(FailureClass::UsageLimit)),
            "limit" => Plan::Done(Errored, Some(FailureClass::UsageLimit)),
            "deny" => Plan::Done(Errored, Some(FailureClass::PermissionError)),
            "invalid" => Plan::Done(Errored, Some(FailureClass::ValidationError)),
            "unknown" => Plan::Done(Errored, Some(FailureClass::Unknown)),
            "selfstop" => Plan::Done(DoneStatus::Interrupted, None),
            _ => Plan::Done(Completed, None),
        }
    }
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
                steers.lock().unwrap().push(m.prompt);
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
        self.runs.lock().unwrap().push(request.prompt.clone());
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
        let finish = self.finish.subscribe();
        let steers = self.steers.clone();
        let delay = self.interrupt_delay;
        let parks = self.parks;
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.statuses() != want {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "timed out waiting for {what}: statuses {:?}, runs {:?}, acts {:?}",
                    self.statuses(),
                    self.agent.runs(),
                    self.acts()
                );
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
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
    }

    async fn finish(self) {
        if self.wire.is_some() {
            self.check_trace();
        }
        let _ = self.agent.finish.send(());
        self.core.shutdown().await;
    }
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
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
