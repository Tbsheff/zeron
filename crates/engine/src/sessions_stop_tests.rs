use super::*;
use crate::orchestrator::wire::{ChildSpec, DelegateRequest};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct StopHarness {
    feeds: Mutex<HashMap<String, mpsc::UnboundedSender<AgentEvent>>>,
    cancelled: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl Harness for StopHarness {
    fn id(&self) -> HarnessId { HarnessId::Mock }
    fn display_name(&self) -> &str { "Stop fixture" }
    fn supports_steering(&self) -> bool { true }
    fn steering_mode(&self) -> zeron_proto::SteeringMode { zeron_proto::SteeringMode::TurnBoundary }
    fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] { &[] }
    async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> { Ok(vec![]) }
    async fn run(&self, request: RunRequest, controls: RunControls) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>, zeron_harness::HarnessError> {
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&self.feeds).insert(request.prompt, tx);
        let cancelled = self.cancelled.clone();
        Ok(futures::stream::unfold((rx, controls.interrupt, cancelled), |(mut rx, interrupt, cancelled)| async move {
            tokio::select! {
                biased;
                _ = interrupt.cancelled() => { cancelled.fetch_add(1, Ordering::SeqCst); None }
                event = rx.recv() => event.map(|event| (Ok(event), (rx, interrupt, cancelled))),
            }
        }).boxed())
    }
}
fn request(prompt: &str) -> RunRequest {
    RunRequest {
        env: Default::default(), prompt: prompt.into(), harness: Some(HarnessId::Mock), model: None,
        reasoning: None, model_options: Default::default(), cwd: "/tmp".into(),
        sandbox: zeron_proto::SandboxLevel::WorkspaceWrite, auto_approve: false,
        access: zeron_proto::AccessMode::FullAccess, plan: false, attachments: vec![],
        worktree: None, resume: None, mcp: None, extra_mcp: vec![],
    }
}
fn done() -> AgentEvent {
    AgentEvent::Done { status: DoneStatus::Completed, result: None, error: None, session_id: None, failure: None }
}
async fn wait(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !ready() { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.expect("fixture state reached");
}
async fn boot() -> (tempfile::TempDir, crate::EngineCore, Arc<StopHarness>) {
    let dir = tempfile::tempdir().unwrap();
    let harness = Arc::new(StopHarness::default());
    let registry = HarnessRegistry::new();
    registry.register(harness.clone());
    let core = crate::EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.workspace.create_chat("parent", None, Some(&core.device_id), None, None).unwrap();
    (dir, core, harness)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_parked_runtime_with_background_native_and_app_child() {
    let (_dir, core, harness) = boot().await;
    let wire = core.enable_orchestrator_v2(crate::orchestrator::OrchOptions::default()).unwrap();
    core.doc_host.queue_command("parent", zeron_doc::SessionCommandPayload::Run { request: request("parent"), message_id: "m1".into() }).unwrap();
    wait(|| lock(&harness.feeds).contains_key("parent")).await;
    let feed = lock(&harness.feeds)["parent"].clone();
    feed.send(AgentEvent::ToolCall { id: "background".into(), call: zeron_proto::ToolCall::Unknown { name: "Agent".into(), input: Some(serde_json::json!({"run_in_background":true})) } }).unwrap();
    feed.send(AgentEvent::Subagent { parent_tool_use_id: "background".into(), event: Box::new(AgentEvent::TextDelta { text: "partial result".into() }) }).unwrap();
    wait(|| wire.driver().state().unwrap().natives.values().any(|n| n.background)).await;
    wire.delegate(&core.sessions, DelegateRequest {
        origin_chat_id: "parent".into(), task: "child".into(), title: None, role: None,
        mode: Some("async".into()), timeout_ms: None, client_request_id: Some("stop-child".into()),
        observed_input: None, followup: false, child: ChildSpec {
            chat_id: "child".into(),
            config: Some(zeron_proto::ChatConfig {
                harness: HarnessId::Mock, model: None, reasoning: None, model_options: Default::default(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                access: zeron_proto::AccessMode::FullAccess, plan: false,
            }),
            ..Default::default()
        },
    }).await.unwrap();
    wait(|| core.sessions.has_run("child") && lock(&harness.feeds).len() == 2).await;
    feed.send(done()).unwrap();
    wait(|| wire.thread_state("parent").is_some_and(|t| !t.is_live()) && !core.sessions.turn_in_flight("parent")).await;
    assert!(core.sessions.has_run("parent"));
    assert!(wire.driver().state().unwrap().natives.values().any(|n| n.status == zeron_orch::native::NativeStatus::Running));
    assert!(core.sessions.interrupt("parent").await.unwrap());
    wait(|| !core.sessions.has_run("parent") && !core.sessions.has_run("child")).await;
    wait(|| wire.driver().state().unwrap().natives.values().all(|n| n.status != zeron_orch::native::NativeStatus::Running)).await;
    assert_eq!(harness.cancelled.load(Ordering::SeqCst), 2);
    core.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_old_runtime_guard_preserves_new_runtime() {
    let (_dir, core, harness) = boot().await;
    core.sessions.dispatch("parent", HarnessId::Mock, request("old"), None).await.unwrap();
    wait(|| lock(&harness.feeds).contains_key("old")).await;
    let old = lock(&core.sessions.inner.runs)["parent"].run_id.clone();
    assert!(core.sessions.terminate("parent").await.unwrap());
    wait(|| !core.sessions.has_run("parent")).await;
    core.sessions.dispatch("parent", HarnessId::Mock, request("new"), None).await.unwrap();
    wait(|| lock(&harness.feeds).contains_key("new")).await;
    assert!(!core.sessions.terminate_runtime("parent", Some(&old)).await.unwrap());
    assert!(core.sessions.has_run("parent"));
    assert_eq!(harness.cancelled.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_ends_captured_runtime_when_wire_step_returns_error() {
    let (_dir, core, harness) = boot().await;
    let wire = core.enable_orchestrator_v2(crate::orchestrator::OrchOptions::default()).unwrap();
    core.doc_host.queue_command("parent", zeron_doc::SessionCommandPayload::Run { request: request("parent"), message_id: "m1".into() }).unwrap();
    wait(|| lock(&harness.feeds).contains_key("parent") && wire.thread_state("parent").is_some_and(|t| t.is_live())).await;
    wire.driver().crash_at(crate::orchestrator::driver::FaultPoint::Fp1StepCommittedEffectNotRun, 0);
    assert!(core.sessions.interrupt("parent").await.is_err());
    wait(|| !core.sessions.has_run("parent")).await;
    assert_eq!(harness.cancelled.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}
