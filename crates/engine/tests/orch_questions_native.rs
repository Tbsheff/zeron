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
    nested_release: Arc<tokio::sync::Notify>,
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
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let answers = self.answers.clone();
        let nested_release = self.nested_release.clone();
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
            } else if request.prompt == "nested-spawn-only" {
                tx.unbounded_send(Ok(AgentEvent::ToolCall {
                    id: "outer".into(),
                    call: ToolCall::Unknown {
                        name: "Agent".into(),
                        input: Some(serde_json::json!({"run_in_background":true})),
                    },
                }))
                .unwrap();
                tx.unbounded_send(Ok(AgentEvent::Subagent {
                    parent_tool_use_id: "outer".into(),
                    event: Box::new(AgentEvent::ToolCall {
                        id: "unstarted".into(),
                        call: ToolCall::Unknown {
                            name: "Agent".into(),
                            input: Some(serde_json::json!({"run_in_background":true})),
                        },
                    }),
                }))
                .unwrap();
                tx.unbounded_send(Ok(done())).unwrap();
                controls.interrupt.cancelled().await;
            } else if request.prompt == "nested-native" {
                for outer in ["outer-a", "outer-b"] {
                    tx.unbounded_send(Ok(AgentEvent::ToolCall {
                        id: outer.into(),
                        call: ToolCall::Unknown {
                            name: "Agent".into(),
                            input: Some(serde_json::json!({"run_in_background":true})),
                        },
                    }))
                    .unwrap();
                    tx.unbounded_send(Ok(AgentEvent::Subagent {
                        parent_tool_use_id: outer.into(),
                        event: Box::new(AgentEvent::TextDelta {
                            text: "outer running".into(),
                        }),
                    }))
                    .unwrap();
                }
                tx.unbounded_send(Ok(done())).unwrap();
                let _ = controls.steering.recv().await;
                tx.unbounded_send(Ok(AgentEvent::Steered {
                    assistant_message_id: None,
                    next_assistant_message_id: None,
                }))
                .unwrap();
                tx.unbounded_send(Ok(AgentEvent::TextDelta {
                    text: "new root active".into(),
                }))
                .unwrap();
                nested_release.notified().await;
                for outer in ["outer-a", "outer-b"] {
                    let wrap = |event| AgentEvent::Subagent {
                        parent_tool_use_id: outer.into(),
                        event: Box::new(event),
                    };
                    tx.unbounded_send(Ok(wrap(AgentEvent::ToolCall {
                        id: "local".into(),
                        call: ToolCall::Unknown {
                            name: "Agent".into(),
                            input: Some(serde_json::json!({"run_in_background":false})),
                        },
                    })))
                    .unwrap();
                    if outer == "outer-a" {
                        tx.unbounded_send(Ok(wrap(done()))).unwrap();
                    }
                    tx.unbounded_send(Ok(wrap(AgentEvent::Subagent {
                        parent_tool_use_id: "local".into(),
                        event: Box::new(AgentEvent::TextDelta {
                            text: format!("FINAL_NESTED_{outer}"),
                        }),
                    })))
                    .unwrap();
                    for _ in 0..2 {
                        tx.unbounded_send(Ok(wrap(AgentEvent::Subagent {
                            parent_tool_use_id: "local".into(),
                            event: Box::new(done()),
                        })))
                        .unwrap();
                    }
                }
                controls.interrupt.cancelled().await;
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
    export_capture(&wire, &core.device_id);
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
    export_capture(&wire, &core.device_id);
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
    if wire.driver().mechanism_capture_enabled() {
        let rows = wire.driver().mechanism_lines().unwrap();
        let last: serde_json::Value = serde_json::from_str(rows.last().unwrap()).unwrap();
        let facts = last["snapshot"]["facts"].as_array().unwrap();
        let source: Vec<_> = facts
            .iter()
            .filter(|f| f["kind"] == "provider_event")
            .map(|f| &f["data"])
            .collect();
        assert!(source.iter().any(|f| f["event_kind"] == "root_terminal"
            && f["observer"]["provider_alive"] == true
            && f["observer"]["root_turn_active"] == false));
        assert!(source.iter().any(|f| f["event_kind"] == "native_terminal"
            && f["status"] == "completed"
            && f["observer"]["provider_alive"] == true
            && f["observer"]["root_turn_active"] == false));
    }
    export_capture(&wire, &core.device_id);
    core.shutdown().await;
}

fn export_capture(wire: &zeron_engine::orchestrator::OrchWire, device: &str) {
    let test_name = std::thread::current()
        .name()
        .expect("test thread name")
        .to_owned();
    for (env, raw) in [("ORCH_TRACE_OUT", false), ("ORCH_MECHANISM_OUT", true)] {
        let Some(root) = std::env::var_os(env) else {
            continue;
        };
        let rows = if raw {
            wire.driver().mechanism_lines().unwrap()
        } else {
            wire.trace_lines().unwrap()
        };
        assert!(!rows.is_empty(), "capture output must not be empty");
        let dir = std::path::PathBuf::from(root)
            .join(format!("orch_questions_native-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let filename = format!("{device}.jsonl");
        std::fs::write(dir.join(&filename), rows.join("\n") + "\n").unwrap();
        std::fs::write(dir.join("capture.json"),serde_json::to_vec(&serde_json::json!({"test_name":test_name,"devices":[device],"filenames":[filename],"capture":"present"})).unwrap()).unwrap();
    }
}

async fn wait_native(
    label: &str,
    wire: &zeron_engine::orchestrator::OrchWire,
    device: &str,
    mut ready: impl FnMut() -> bool,
) {
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if result.is_err() {
        export_capture(wire, device);
        panic!(
            "{label}: state={:?}; pending={:?}; trace={:?}",
            wire.driver().state().unwrap(),
            wire.driver().pending().unwrap(),
            wire.trace_lines().unwrap()
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_nested_native_paths_save_distinct_docs_and_deliver_once() {
    let (_dir, core, agent, wire) = boot().await;
    send(&core, "nested-native").await;
    wait_native("root parked", &wire, &core.device_id, || {
        wire.thread_state("parent").is_some_and(|t| !t.is_live())
            && wire.driver().state().unwrap().natives.len() == 2
    })
    .await;
    core.doc_host
        .queue_command(
            "parent",
            zeron_doc::SessionCommandPayload::Run {
                request: request("next root"),
                message_id: "m2".into(),
            },
        )
        .unwrap();
    wait_native("new root live", &wire, &core.device_id, || {
        wire.thread_state("parent")
            .is_some_and(|t| t.is_live() && t.rord() == 2)
    })
    .await;
    agent.nested_release.notify_one();
    wait_native("nested delivered", &wire, &core.device_id, || {
        wire.driver()
            .state()
            .unwrap()
            .natives
            .values()
            .filter(|n| n.ancestor.is_some() && n.delivery == zeron_orch::Delivery::Delivered)
            .count()
            == 2
    })
    .await;
    wait_native(
        "native changed effects committed",
        &wire,
        &core.device_id,
        || {
            !wire
                .driver()
                .pending()
                .unwrap()
                .iter()
                .any(|effect| matches!(effect.effect, zeron_orch::Effect::NativeChanged { .. }))
        },
    )
    .await;
    let state = wire.driver().state().unwrap();
    assert_eq!(state.natives.len(), 4);
    assert_eq!(
        state
            .natives
            .values()
            .filter(
                |n| n.ancestor.is_none() && n.status == zeron_orch::native::NativeStatus::Running
            )
            .count(),
        1
    );
    let mut docs = std::collections::HashSet::new();
    for ((_, key), task) in state.natives.iter().filter(|(_, n)| n.ancestor.is_some()) {
        assert!(task.background);
        assert_eq!(task.attempt.run.ord, 1);
        let binding = wire.native_binding(*key).unwrap().unwrap();
        assert!(!binding.own_background);
        assert_eq!(binding.path.len(), 2);
        assert!(docs.insert(binding.doc_id.clone()));
        let text = serde_json::to_string(
            &core
                .doc_host
                .open(&binding.doc_id)
                .unwrap()
                .doc()
                .read_entries()
                .unwrap(),
        )
        .unwrap();
        assert!(text.contains(&format!("FINAL_NESTED_{}", binding.path[0])));
        let owner = core.doc_host.open(&binding.chip_doc_id).unwrap();
        assert!(owner.doc().read_entries().unwrap().iter().flat_map(|e|&e.parts).any(|p|matches!(p,zeron_doc::MessagePart::Tool{id,subagent_ref:Some(doc),subagent_status:Some(zeron_doc::SubagentStatus::Done),..} if id=="local" && doc==&binding.doc_id)));
        let view = wire
            .task_status(&core.sessions, "parent", &binding.doc_id)
            .unwrap();
        assert_eq!(view.status, "completed");
    }
    assert_eq!(wire.task_list(&core.sessions, "parent").unwrap().len(), 4);
    let count = || {
        zeron_engine::orchestrator::trace::parse_lines(&wire.trace_lines().unwrap().join("\n"))
            .unwrap()
            .into_iter()
            .filter(|r| r.ok && !r.dup && r.act == "ServerEnqueue")
            .count()
    };
    assert_eq!(count(), 3);
    wire.recover(&core.sessions).await.unwrap();
    assert_eq!(count(), 3);
    assert!(
        !wire
            .driver()
            .pending()
            .unwrap()
            .iter()
            .any(|e| matches!(e.effect, zeron_orch::Effect::NativeChanged { .. }))
    );
    export_capture(&wire, &core.device_id);
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_closes_nested_spawn_before_first_progress() {
    let (_dir, core, _agent, wire) = boot().await;
    send(&core, "nested-spawn-only").await;
    wait(|| {
        wire.driver().state().unwrap().natives.len() == 2
            && wire.thread_state("parent").is_some_and(|t| !t.is_live())
    })
    .await;
    assert!(core.sessions.interrupt("parent").await.unwrap());
    wait(|| {
        wire.driver()
            .state()
            .unwrap()
            .natives
            .values()
            .all(|n| n.status != zeron_orch::native::NativeStatus::Running)
    })
    .await;
    let rows = wire.native_tasks("parent").unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["status"] == "failed"));
    export_capture(&wire, &core.device_id);
    core.shutdown().await;
}
