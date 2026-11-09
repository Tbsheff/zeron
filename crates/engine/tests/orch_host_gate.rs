use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zeron_doc::{SessionCommandPayload, SessionCommandStatus};
use zeron_engine::{EngineCore, HarnessRegistry, orchestrator::OrchOptions};
use zeron_harness::{Harness, HarnessError, RunControls, mock::MockHarness};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

struct CountHarness(AtomicUsize);
#[async_trait]
impl Harness for CountHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "host gate fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        MockHarness {
            script: vec![AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
                failure: None,
            }],
        }
        .run(request, controls)
        .await
    }
}
fn request(cwd: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: "check host".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: vec![],
        worktree: None,
        resume: None,
        extra_mcp: vec![],
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v2_unknown_and_remote_hosts_never_start_or_claim_and_unknown_work_stays_pending() {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    let harness = Arc::new(CountHarness(AtomicUsize::new(0)));
    registry.register(harness.clone());
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.enable_orchestrator_v2(OrchOptions::default()).unwrap();
    let req = request(dir.path().to_str().unwrap());
    assert!(
        core.sessions
            .dispatch(
                "unknown",
                HarnessId::Mock,
                req.clone(),
                Some("direct-unknown".into())
            )
            .await
            .is_err()
    );
    assert!(core.workspace.chat("unknown").unwrap().is_none());
    core.workspace
        .create_chat(
            "remote",
            None,
            Some("other-device"),
            None,
            Some(req.cwd.clone()),
        )
        .unwrap();
    assert!(
        core.sessions
            .dispatch(
                "remote",
                HarnessId::Mock,
                req.clone(),
                Some("direct-remote".into())
            )
            .await
            .is_err()
    );
    let id = core
        .doc_host
        .queue_command(
            "unknown",
            SessionCommandPayload::Run {
                request: req.clone(),
                message_id: "queued-unknown".into(),
            },
        )
        .unwrap();
    let unknown = core.doc_host.open("unknown").unwrap();
    core.doc_host.drain_commands(&unknown).await;
    assert_eq!(
        unknown
            .doc()
            .read_commands()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .status,
        SessionCommandStatus::Pending
    );
    assert_eq!(harness.0.load(Ordering::SeqCst), 0);
    assert!(core.workspace.chat("unknown").unwrap().is_none());
    core.workspace
        .create_chat(
            "unknown",
            None,
            Some(&core.device_id),
            None,
            Some(req.cwd.clone()),
        )
        .unwrap();
    core.doc_host.drain_commands(&unknown).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while harness.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(harness.0.load(Ordering::SeqCst), 1);
    core.workspace
        .create_chat_with_parent(
            "local-child",
            None,
            Some(&core.device_id),
            None,
            Some(req.cwd.clone()),
            Some("unknown".into()),
        )
        .unwrap();
    core.sessions
        .dispatch(
            "local-child",
            HarnessId::Mock,
            req,
            Some("known-child".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while harness.0.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(harness.0.load(Ordering::SeqCst), 2);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flag_off_keeps_legacy_unknown_chat_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    let harness = Arc::new(CountHarness(AtomicUsize::new(0)));
    registry.register(harness.clone());
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.sessions
        .dispatch(
            "legacy-unknown",
            HarnessId::Mock,
            request(dir.path().to_str().unwrap()),
            Some("legacy-message".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while harness.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(harness.0.load(Ordering::SeqCst), 1);
    core.shutdown().await;
}
