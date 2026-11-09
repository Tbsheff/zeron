use super::*;
use zeron_orch::transfer::TransferStatus;

struct TransferHarness {
    store: Mutex<Option<Arc<zeron_sync::DocsStore>>>,
    captured: Mutex<Vec<String>>,
    reject_resume: bool,
    fail_starts: Mutex<usize>,
    attempts: Mutex<Vec<(Option<String>, String)>>,
    wire: Mutex<Option<Arc<crate::orchestrator::OrchWire>>>,
    provider: Mutex<rusqlite::Connection>,
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
        lock(&self.attempts).push((request.resume.clone(), request.prompt.clone()));
        let failed = {
            let mut left = lock(&self.fail_starts);
            if *left > 0 {
                *left -= 1;
                true
            } else {
                false
            }
        };
        if failed {
            use sha2::{Digest, Sha256};
            let packet = format!("{:x}", Sha256::digest(request.prompt.as_bytes()));
            let session = request
                .resume
                .clone()
                .unwrap_or_else(|| format!("unreported-{}", lock(&self.attempts).len()));
            lock(&self.provider)
                .execute(
                    "INSERT INTO accepted(session,packet,items) VALUES (?1,?2,?3)",
                    rusqlite::params![session, packet, "[]"],
                )
                .unwrap();
            return Ok(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some("fixture transport failed before start".into()),
                session_id: None,
                failure: None,
            })])
            .boxed());
        }
        if self.reject_resume && request.resume.is_some() {
            return Ok(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some("Session not found".into()),
                session_id: None,
                failure: None,
            })])
            .boxed());
        }
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
        use sha2::{Digest, Sha256};
        let packet_digest = format!("{:x}", Sha256::digest(request.prompt.as_bytes()));
        let context_packet_digest = request
            .prompt
            .rsplit_once("\nCurrent request:\n")
            .map(|(packet, _)| format!("{:x}", Sha256::digest(packet.as_bytes())));
        let accepted_ids: Vec<_> = request
            .prompt
            .lines()
            .filter_map(|line| {
                line.strip_prefix("[assistant ")
                    .and_then(|line| line.strip_suffix(']'))
                    .map(str::to_owned)
            })
            .collect();
        lock(&self.provider)
            .execute(
                "INSERT INTO accepted(session,packet,items) VALUES (?1,?2,?3)",
                rusqlite::params![
                    "fixture-session",
                    packet_digest,
                    serde_json::to_string(&accepted_ids).unwrap()
                ],
            )
            .unwrap();
        let wire = lock(&self.wire).clone().unwrap();
        wire.driver().observe_mechanism(&format!("transfer-provider:accepted:{packet_digest}"),"transfer_provider",serde_json::json!({"adapter":"sqlite_transfer_fixture","stage":"accepted_request","chat":"transfer-target","input":"current","session":"fixture-session","packet_digest":packet_digest,"context_packet_digest":context_packet_digest,"accepted_ids":accepted_ids,"protocol_assumption":"request accepted at adapter boundary; SessionStarted proves session identity"})).unwrap();
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
    drive_fixture(false, false, false, true).await;
}
#[tokio::test]
async fn failed_transfer_aborts_start_and_persists_retry_failure() {
    drive_fixture(true, false, false, true).await;
}
#[tokio::test]
async fn drive_run_rejected_resume_uses_full_stable_prefix() {
    drive_fixture(false, true, false, true).await;
}
#[tokio::test]
async fn drive_run_unconfirmed_context_survives_two_failed_starts() {
    drive_fixture(false, false, true, true).await;
}
#[tokio::test]
async fn drive_run_fresh_context_survives_two_failed_starts() {
    drive_fixture(false, false, true, false).await;
}
async fn drive_fixture(fail: bool, reject_resume: bool, failed_starts: bool, known_session: bool) {
    let temp = tempfile::tempdir().unwrap();
    let provider = rusqlite::Connection::open(temp.path().join("provider.sqlite")).unwrap();
    provider.execute_batch("CREATE TABLE accepted(session TEXT NOT NULL, packet TEXT NOT NULL, items TEXT NOT NULL)").unwrap();
    let agent = Arc::new(TransferHarness {
        store: Mutex::new(None),
        captured: Mutex::new(vec![]),
        reject_resume,
        fail_starts: Mutex::new(if failed_starts { 2 } else { 0 }),
        attempts: Mutex::new(vec![]),
        wire: Mutex::new(None),
        provider: Mutex::new(provider),
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
    let wire = core.enable_orchestrator_v2(crate::orchestrator::OrchOptions::default())
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while wire.recovery_pending() {
            tokio::task::yield_now().await;
        }
    }).await.expect("orchestrator startup recovery completes");
    *lock(&agent.wire) = core.sessions.orchestrator();
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
    core.workspace
        .create_chat("source", None, Some(&core.device_id), None, None)
        .unwrap();
    let source = core.doc_host.open("source").unwrap();
    source.doc().push_message(&entry).unwrap();
    let wire = core.sessions.orchestrator().unwrap();
    crate::orchestrator::transfer::begin_operation(
        wire.driver(),
        &core.doc_host,
        &source,
        "source",
    )
    .unwrap();
    let before = crate::orchestrator::transfer::begin_operation(
        wire.driver(),
        &core.doc_host,
        &handle,
        "transfer-target",
    )
    .unwrap();
    let pin_args = serde_json::json!({"id":"fixture","source_chat":"source","target_chat":"transfer-target","base":null,"point":"source-end","source":crate::orchestrator::transfer::source_fact(std::slice::from_ref(&entry)),"target_local":true,"caller_relation_checked":true});
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
    crate::orchestrator::transfer::end_operation(
        wire.driver(),
        &core.doc_host,
        "transfer-target",
        before,
        "TransferPin",
        pin_args,
        true,
    )
    .unwrap();
    if reject_resume || (failed_starts && known_session) {
        let entry = source.doc().read_entries().unwrap().remove(0);
        if reject_resume {
            handle.doc().push_message(&entry).unwrap();
        }
        handle
            .doc()
            .set_provider_session(
                "mock",
                &zeron_doc::ProviderSessionMark {
                    session_id: "missing-session".into(),
                    cwd: temp.path().to_string_lossy().into_owned(),
                    seen: Some("source-end".into()),
                },
            )
            .unwrap();
        core.sessions.inner.remember_harness_session(
            "transfer-target",
            "missing-session",
            &temp.path().to_string_lossy(),
            HarnessId::Mock,
        );
    }
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
    if reject_resume {
        use crate::orchestrator::wire::{Pending, SendKind, Via};
        wire.send(
            &core.sessions,
            Pending {
                chat: "transfer-target".into(),
                message_id: "current".into(),
                via: Via::Dispatch { harness: HarnessId::Mock, request: Box::new(request.clone()) },
            },
            SendKind::Person,
            false,
        ).await.unwrap();
    } else {
        core.sessions
            .dispatch_with(
                "transfer-target",
                HarnessId::Mock,
                request.clone(),
                Some("current".into()),
                failed_starts,
                0,
                None,
            )
            .await
            .unwrap();
    }
    if failed_starts {
        for expected in 1..=2 {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if lock(&agent.attempts).len() >= expected
                        && !lock(&core.sessions.inner.runs).contains_key("transfer-target")
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|error| panic!("failed start {expected} did not end: {error}; attempts={:?}; runs={:?}; status={:?}", lock(&agent.attempts), lock(&core.sessions.inner.runs).keys().cloned().collect::<Vec<_>>(), core.sessions.session_status("transfer-target")));
            if expected == 1 {
                assert_eq!(lock(&agent.attempts)[0].0.is_some(), known_session);
                core.sessions
                    .dispatch_with(
                        "transfer-target",
                        HarnessId::Mock,
                        request.clone(),
                        Some("current".into()),
                        true,
                        0,
                        None,
                    )
                    .await
                    .unwrap();
            }
        }
        let attempts = lock(&agent.attempts).clone();
        assert_eq!(attempts.len(), 2);
        assert_eq!(
            attempts[0].1, attempts[1].1,
            "same input retains exact packet"
        );
        assert!(
            attempts[1].0.is_none(),
            "unconfirmed acceptance requires fresh provider identity"
        );
        assert_eq!(attempts[0].0.is_some(), known_session);
        assert!(attempts[0].1.contains("prior source context"));
        let bytes = core
            .doc_host
            .docs_store()
            .load_snapshot("transfer-target")
            .unwrap()
            .unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).unwrap();
        assert!(crate::orchestrator::transfer::uncertain_for_other_input(
            &SessionDoc::from_doc(raw),
            "mock",
            if known_session {
                "missing-session"
            } else {
                "fresh:current"
            },
            "later"
        ));
        let mut local = source.doc().read_entries().unwrap().remove(0);
        local.id = "local-complete".into();
        local.parts = vec![MessagePart::Text {
            id: "local-text".into(),
            text: "local stable context".into(),
        }];
        handle.doc().push_message(&local).unwrap();
        let mut later = request.clone();
        later.prompt = "later input".into();
        core.sessions
            .dispatch(
                "transfer-target",
                HarnessId::Mock,
                later,
                Some("later".into()),
            )
            .await
            .unwrap();
    }
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
    .unwrap_or_else(|error| panic!("transfer did not finish: {error}; attempts={:?}; runs={:?}; status={:?}", lock(&agent.attempts), lock(&core.sessions.inner.runs).keys().cloned().collect::<Vec<_>>(), core.sessions.session_status("transfer-target")));
    if reject_resume {
        let attempts = lock(&agent.attempts);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].0.as_deref(), Some("missing-session"));
        assert!(attempts[1].0.is_none(), "rejected resume must start fresh");
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
        crate::orchestrator::transfer::export_capture_for_test(
            core.sessions.orchestrator().unwrap().driver(),
            "failed_transfer_aborts_start_and_persists_retry_failure",
            "focused-mock-engine",
        );
        core.sessions.shutdown().await;
        return;
    }
    if failed_starts {
        let count: i64 = lock(&agent.provider)
            .query_row("SELECT COUNT(*) FROM accepted", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 3,
            "provider accepted both failed requests before identity events"
        );
    }
    let prompts = lock(&agent.captured);
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("prior source context"));
    assert!(prompts[0].ends_with(if failed_starts {
        "later input"
    } else {
        "current input"
    }));
    if failed_starts {
        assert!(
            lock(&agent.attempts).last().unwrap().0.is_none(),
            "new input must use fresh provider"
        );
        assert!(prompts[0].contains("local stable context"));
    }
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
    crate::orchestrator::transfer::export_capture_for_test(
        core.sessions.orchestrator().unwrap().driver(),
        if failed_starts {
            if known_session {
                "drive_run_unconfirmed_context_survives_two_failed_starts"
            } else {
                "drive_run_fresh_context_survives_two_failed_starts"
            }
        } else if reject_resume {
            "drive_run_rejected_resume_uses_full_stable_prefix"
        } else {
            "drive_run_persists_consumed_packet_before_provider_start"
        },
        "focused-mock-engine",
    );
    core.sessions.shutdown().await;
}
