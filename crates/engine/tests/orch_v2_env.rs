use std::sync::Arc;
use std::time::Duration;

use zeron_doc::SessionCommandPayload;
use zeron_engine::orchestrator::wire::{Pending, Ran, RunEngine, spawn_task};
use zeron_engine::orchestrator::{FLAG_ENV, OrchOptions, OrchWire, trace};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::mock::MockHarness;
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, ReasoningLevel, RunRequest, SandboxLevel};

const CHAT: &str = "chat-env";

fn set_flag() {
    unsafe { std::env::set_var(FLAG_ENV, "1") };
}

fn registry() -> Arc<HarnessRegistry> {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness {
        script: vec![
            AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "sess-env".into(),
                assistant_message_id: "a1".into(),
            },
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-env".into()),
                failure: None,
            },
        ],
    }));
    Arc::new(registry)
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

struct Idle;

#[async_trait::async_trait]
impl RunEngine for Idle {
    async fn start(&self, _: &Pending) -> Result<String, zeron_engine::EngineError> {
        Ok(String::new())
    }
    async fn steer(&self, _: &Pending) -> Result<Ran, zeron_engine::EngineError> {
        Ok(Ran::Nothing)
    }
    async fn stop_attempt(&self, _: &str) -> bool {
        false
    }
    async fn interrupt_run(&self, _: &str) -> bool {
        false
    }
    fn owns_run(&self, _: &str) -> bool {
        false
    }
    fn last_done(&self, _: &str) -> Option<(u64, DoneStatus, Option<zeron_proto::FailureClass>)> {
        None
    }
    fn journal_seq(&self, _: &str) -> u64 {
        0
    }
    fn kick_drain(&self, _: &str) {}
}

#[test]
fn attaching_the_wire_outside_a_runtime_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).expect("store"));
    let wire = OrchWire::open(store, "dev-1", OrchOptions::default()).expect("wire");
    wire.attach(Arc::new(Idle));
    assert!(!spawn_task("probe", async {}), "no runtime, no task");
    assert!(!wire.is_dead());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_env_flag_enables_the_orchestrator_at_assembly() {
    set_flag();
    let dir = tempfile::tempdir().unwrap().keep();
    let core = EngineCore::assemble(&dir.join("data"), registry(), HarnessId::Mock, None)
        .expect("engine assembles");
    let wire = core.sessions.orchestrator().expect("flag on");
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({"op": "createChat", "chatId": CHAT, "deviceId": core.device_id}),
        )
        .await
        .expect("createChat");
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("hello"),
                message_id: "m1".into(),
            },
        )
        .expect("queue run");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let acts = loop {
        let lines = wire.trace_lines().expect("trace");
        let parsed = trace::parse_lines(&lines.join("\n")).expect("parse");
        let acts: Vec<String> = parsed
            .iter()
            .filter(|l| l.ok)
            .map(|l| l.act.clone())
            .collect();
        if acts.iter().any(|a| a == "IngestEnd") {
            trace::check(&parsed).unwrap_or_else(|e| panic!("trace check: {e}"));
            break acts;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no run end in {acts:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    for act in ["OpenRoot", "PersonSend", "StartFromQueue"] {
        assert!(acts.iter().any(|a| a == act), "{act} missing from {acts:?}");
    }
    core.shutdown().await;
}
