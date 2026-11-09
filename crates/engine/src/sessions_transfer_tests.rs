use super::*;
use zeron_orch::transfer::TransferStatus;

struct TransferHarness {
    store: Mutex<Option<Arc<zeron_sync::DocsStore>>>,
    captured: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl Harness for TransferHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Transfer fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> zeron_proto::SteeringMode {
        zeron_proto::SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
        zeron_harness::HarnessError,
    > {
        let store = lock(&self.store).clone().unwrap();
        let bytes = store
            .load_snapshot("transfer-target")
            .unwrap()
            .expect("receipt persisted before provider start");
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).unwrap();
        let persisted = SessionDoc::from_doc(raw);
        let transfers = crate::orchestrator::transfer::status(&persisted);
        assert_eq!(transfers["fixture"].status, TransferStatus::Consumed);
        lock(&self.captured).push(request.prompt.clone());
        Ok(futures::stream::iter([
            Ok(AgentEvent::SessionStarted {
                session_id: "fixture-session".into(),
                cwd: request.cwd,
                harness: HarnessId::Mock,
                model: "fixture".into(),
                tools: vec![],
                assistant_message_id: "fixture-reply".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: Some("done".into()),
                error: None,
                session_id: Some("fixture-session".into()),
                failure: None,
            }),
        ])
        .boxed())
    }
}
#[tokio::test]
async fn drive_run_persists_consumed_packet_before_provider_start() {
    drive_fixture(false).await;
}
#[tokio::test]
async fn failed_transfer_aborts_start_and_persists_retry_failure() {
    drive_fixture(true).await;
}
async fn drive_fixture(fail: bool) {
    let temp = tempfile::tempdir().unwrap();
    let agent = Arc::new(TransferHarness {
        store: Mutex::new(None),
        captured: Mutex::new(vec![]),
    });
    let registry = HarnessRegistry::new();
    registry.register(agent.clone());
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(temp.path()).unwrap(),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    core.enable_orchestrator_v2(crate::orchestrator::OrchOptions::default())
        .unwrap();
    core.workspace
        .create_chat("transfer-target", None, Some(&core.device_id), None, None)
        .unwrap();
    *lock(&agent.store) = Some(core.doc_host.docs_store());
    let handle = core.doc_host.open("transfer-target").unwrap();
    let entry = zeron_doc::SessionMessageEntry {
        id: "source-end".into(),
        role: zeron_doc::MessageRole::Assistant,
        parts: vec![MessagePart::Text {
            id: "text".into(),
            text: if fail {
                "oversized anchor".repeat(2000)
            } else {
                "prior source context".into()
            },
        }],
        created_at: 1,
        device_id: core.device_id.clone(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    };
    crate::orchestrator::transfer::pin(
        handle.doc(),
        "fixture",
        "source",
        "transfer-target",
        None,
        "source-end",
        &[entry],
    )
    .unwrap();
    let request = RunRequest {
        prompt: "current input".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: temp.path().to_string_lossy().into_owned(),
        sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        env: Default::default(),
        mcp: None,
        extra_mcp: vec![],
    };
    core.sessions
        .dispatch(
            "transfer-target",
            HarnessId::Mock,
            request,
            Some("current".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if (fail || !lock(&agent.captured).is_empty())
                && !core.sessions.turn_in_flight("transfer-target")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    if let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        let lines = core
            .sessions
            .orchestrator()
            .unwrap()
            .driver()
            .mechanism_lines()
            .unwrap();
        std::fs::write(
            root.join(if fail {
                "transfer-failed.jsonl"
            } else {
                "transfer-start.jsonl"
            }),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }
    if fail {
        assert!(
            lock(&agent.captured).is_empty(),
            "failed transfer must not start provider"
        );
        let bytes = core
            .doc_host
            .docs_store()
            .load_snapshot("transfer-target")
            .unwrap()
            .unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).unwrap();
        let persisted = SessionDoc::from_doc(raw);
        assert!(matches!(
            crate::orchestrator::transfer::status(&persisted)["fixture"].status,
            TransferStatus::Failed(_)
        ));
        assert!(
            crate::orchestrator::transfer::consume(
                &persisted,
                "mock:fresh:current",
                "current",
                16000
            )
            .is_err()
        );
        core.sessions.shutdown().await;
        return;
    }
    let prompts = lock(&agent.captured);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("prior source context"));
    assert!(prompts[0].ends_with("current input"));
    drop(prompts);
    let bytes = core
        .doc_host
        .docs_store()
        .load_snapshot("transfer-target")
        .unwrap()
        .unwrap();
    let raw = loro::LoroDoc::new();
    raw.import(&bytes).unwrap();
    let persisted = SessionDoc::from_doc(raw);
    crate::orchestrator::transfer::pin(
        &persisted,
        "return",
        "source",
        "transfer-target",
        None,
        "source-end",
        &[zeron_doc::SessionMessageEntry {
            id: "source-end".into(),
            role: zeron_doc::MessageRole::Assistant,
            parts: vec![MessagePart::Text {
                id: "text".into(),
                text: "prior source context".into(),
            }],
            created_at: 1,
            device_id: core.device_id.clone(),
            status: Some(zeron_doc::MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }],
    )
    .unwrap();
    let delta = crate::orchestrator::transfer::consume(
        &persisted,
        "mock:fixture-session",
        "return-input",
        16000,
    )
    .unwrap()
    .unwrap();
    assert!(
        !delta.contains("prior source context"),
        "SessionStarted bound the durable fresh receipt"
    );
    core.sessions.shutdown().await;
}
