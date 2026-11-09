use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zeron_engine::orchestrator::{
    OrchOptions,
    wire::{ChildSpec, DelegateRequest},
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::*;

#[derive(Default)]
struct Agent {
    answers: Arc<Mutex<Vec<Vec<UserInputAnswer>>>>,
}
fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: None,
        failure: None,
    }
}
#[async_trait]
impl Harness for Agent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Questions"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
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
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let answers = self.answers.clone();
        tx.unbounded_send(Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock".into(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: "s".into(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        }))
        .unwrap();
        tokio::spawn(async move {
            if request.prompt == "question" {
                let answer = (controls.request_input)(vec![UserInputQuestion {
                    id: "pick".into(),
                    header: "Choice".into(),
                    question: "Pick one".into(),
                    options: vec!["one".into()],
                    multi_select: false,
                    prefill: None,
                    multiline: false,
                }])
                .await
                .unwrap();
                answers.lock().unwrap().push(answer);
                tx.unbounded_send(Ok(done())).unwrap();
            } else if request.prompt.starts_with("native") {
                tx.unbounded_send(Ok(AgentEvent::ToolCall {
                    id: "spawn1".into(),
                    call: ToolCall::Unknown {
                        name: "Agent".into(),
                        input: Some(serde_json::json!({"run_in_background":true})),
                    },
                }))
                .unwrap();
                tx.unbounded_send(Ok(AgentEvent::Subagent {
                    parent_tool_use_id: "spawn1".into(),
                    event: Box::new(AgentEvent::TextDelta {
                        text: "sub result".into(),
                    }),
                }))
                .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                if request.prompt == "native-late" {
                    tx.unbounded_send(Ok(done())).unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                tx.unbounded_send(Ok(AgentEvent::Subagent {
                    parent_tool_use_id: "spawn1".into(),
                    event: Box::new(AgentEvent::TextDelta {
                        text: " FINAL_UNIQUE_NATIVE_TAIL".into(),
                    }),
                }))
                .unwrap();
                tx.unbounded_send(Ok(AgentEvent::Subagent {
                    parent_tool_use_id: "spawn1".into(),
                    event: Box::new(done()),
                }))
                .unwrap();
                controls.interrupt.cancelled().await;
                let _ = tx.unbounded_send(Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                    failure: None,
                }));
            } else {
                controls.interrupt.cancelled().await;
                let _ = tx.unbounded_send(Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                    failure: None,
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
        model: Some("mock".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: AccessMode::FullAccess,
        plan: false,
        attachments: vec![],
        worktree: None,
        resume: None,
        extra_mcp: vec![],
    }
}
async fn wait(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .expect("state reached");
}
async fn boot() -> (
    tempfile::TempDir,
    EngineCore,
    Arc<Agent>,
    Arc<zeron_engine::orchestrator::OrchWire>,
) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Arc::new(Agent::default());
    let registry = HarnessRegistry::new();
    registry.register(agent.clone());
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({"op":"createChat","chatId":"parent","deviceId":core.device_id}),
        )
        .await
        .unwrap();
    let wire = core.enable_orchestrator_v2(OrchOptions::default()).unwrap();
    (dir, core, agent, wire)
}
async fn send(core: &EngineCore, prompt: &str) {
    core.doc_host
        .queue_command(
            "parent",
            zeron_doc::SessionCommandPayload::Run {
                request: request(prompt),
                message_id: "m1".into(),
            },
        )
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_input_round_trip_uses_real_input_resolver() {
    let (_dir, core, agent, wire) = boot().await;
    send(&core, "parent").await;
    wait(|| wire.thread_state("parent").is_some_and(|t| t.is_live())).await;
    let config = ChatConfig {
        harness: HarnessId::Mock,
        model: Some("mock".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
        access: AccessMode::FullAccess,
        plan: false,
    };
    let view = wire
        .delegate(
            &core.sessions,
            DelegateRequest {
                origin_chat_id: "parent".into(),
                task: "question".into(),
                title: None,
                role: None,
                mode: Some("wait".into()),
                timeout_ms: Some(5000),
                client_request_id: Some("question-task".into()),
                observed_input: None,
                followup: false,
                child: ChildSpec {
                    chat_id: "child".into(),
                    config: Some(config),
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap();
    assert!(view.has_pending_question);
    let parent = wire.thread_state("parent").unwrap().id;
    wire.driver()
        .apply(
            "observe-question-wait",
            zeron_orch::Event::Del(zeron_orch::DelEvent::WaitReturn { t: parent }),
        )
        .unwrap();
    let state = wire.driver().state().unwrap();
    let q = state.questions.values().find(|q| q.pending).unwrap();
    let epoch = q.epoch;
    let id = wire
        .driver()
        .meta(&format!("question-id:{}", q.qid))
        .unwrap()
        .unwrap();
    assert!(
        wire.answer_input_command("parent", "child", &id, Some(epoch - 1), &[])
            .is_err()
    );
    wire.answer_input_command(
        "parent",
        "child",
        &id,
        Some(epoch),
        &[UserInputAnswer {
            question_id: "pick".into(),
            labels: vec!["one".into()],
        }],
    )
    .unwrap();
    wait(|| agent.answers.lock().unwrap().len() == 1).await;
    assert_eq!(agent.answers.lock().unwrap()[0][0].labels, ["one"]);
    wait(|| {
        wire.driver()
            .state()
            .unwrap()
            .questions
            .values()
            .all(|q| !q.pending)
    })
    .await;
    core.shutdown().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_native_done_does_not_end_parent_run() {
    let (_dir, core, _agent, wire) = boot().await;
    send(&core, "native").await;
    wait(|| {
        wire.driver()
            .state()
            .unwrap()
            .natives
            .values()
            .any(|n| n.status == zeron_orch::native::NativeStatus::Completed)
    })
    .await;
    let state = wire.driver().state().unwrap();
    assert!(wire.thread_state("parent").unwrap().is_live());
    assert_eq!(
        state.natives.values().next().unwrap().delivery,
        zeron_orch::Delivery::Delivered
    );
    let native = wire.native_tasks("parent").unwrap();
    assert_eq!(native.len(), 1);
    assert_eq!(native[0]["origin"], "native");
    assert_eq!(native[0]["status"], "completed");
    assert_eq!(native[0]["background"], true);
    let id = native[0]["taskId"].as_str().unwrap();
    let list = wire.task_list(&core.sessions, "parent").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].origin, "native");
    assert_eq!(list[0].status, "completed");
    assert!(
        wire.task_cancel(&core.sessions, "parent", id, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("harness owns")
    );
    wait(||core.doc_host.open("parent").unwrap().doc().read_entries().unwrap().iter().flat_map(|e|e.parts.iter()).any(|p|matches!(p,zeron_doc::MessagePart::Tool{id,subagent_status:Some(zeron_doc::SubagentStatus::Done),..}if id=="spawn1"))).await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_native_late_result_queues_once_across_recovery() {
    let (_dir, core, _agent, wire) = boot().await;
    send(&core, "native-late").await;
    wait(|| {
        wire.driver()
            .state()
            .unwrap()
            .natives
            .values()
            .any(|n| n.delivery == zeron_orch::Delivery::Delivered)
    })
    .await;
    let count = || {
        let lines = wire.trace_lines().unwrap();
        zeron_engine::orchestrator::trace::parse_lines(&lines.join("\n"))
            .unwrap()
            .into_iter()
            .filter(|line| line.ok && !line.dup && line.act == "ServerEnqueue")
            .count()
    };
    assert_eq!(count(), 1, "late result has one durable queue destination");
    wait(|| {
        let messages = core
            .doc_host
            .open("parent")
            .unwrap()
            .doc()
            .read_entries()
            .unwrap();
        serde_json::to_string(&messages)
            .unwrap()
            .contains("FINAL_UNIQUE_NATIVE_TAIL")
    })
    .await;
    wire.recover(&core.sessions).await.unwrap();
    assert_eq!(count(), 1);
    core.shutdown().await;
}
