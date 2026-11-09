use super::*;
use crate::orchestrator::questions::*;
use zeron_orch::questions::QuestionEvent;

impl OrchWire {
    pub fn question_event(&self, chat: &str, event: &zeron_proto::AgentEvent) {
        let source = self.known_thread(chat).ok().flatten().and_then(|t| {
            self.driver
                .state()
                .ok()?
                .thread(t)
                .ok()
                .map(|t| t.attempt_id())
        });
        self.question_event_at(chat, event, source);
    }
    pub(crate) fn question_attempt_current(&self, chat: &str, source: Option<AttemptId>) -> bool {
        self.known_thread(chat).ok().flatten().is_some_and(|t| {
            self.driver.state().ok().is_some_and(|s| {
                s.thread(t)
                    .is_ok_and(|t| t.is_live() && Some(t.attempt_id()) == source)
            })
        })
    }
    pub(crate) fn question_event_at(
        &self,
        chat: &str,
        event: &zeron_proto::AgentEvent,
        source: Option<AttemptId>,
    ) {
        let out = (|| -> Result<(), DriverError> {
            let Some(t) = self.known_thread(chat)? else {
                return Ok(());
            };
            match event {
                zeron_proto::AgentEvent::InputRequested {
                    request_id,
                    questions,
                } => {
                    if let Some(at) = source {
                        raised_at(&self.driver, t, request_id, questions, at)?;
                    }
                }
                zeron_proto::AgentEvent::InputResolved {
                    request_id,
                    answers: Some(answers),
                } => resolved(&self.driver, t, request_id, answers)?,
                _ => {}
            }
            Ok(())
        })();
        if let Err(e) = out {
            tracing::warn!(chat,error=%e,"question event failed");
        }
        self.wake.notify_one();
        self.task_events.notify_waiters();
    }
    pub fn native_event(&self, chat: &str, event: &zeron_proto::AgentEvent) {
        let out = (|| -> Result<(), DriverError> {
            let Some(t) = self.known_thread(chat)? else {
                return Ok(());
            };
            crate::orchestrator::native::observe(&self.driver, t, event)
        })();
        if let Err(e) = out {
            tracing::warn!(chat,error=%e,"native event failed");
        }
        self.wake.notify_one();
    }
    pub(super) fn deliver_question(
        &self,
        _engine: &dyn RunEngine,
        m: zeron_orch::Message,
    ) -> Result<(), DriverError> {
        self.driver.apply(
            &format!("question-deliver:{m:?}"),
            Event::Question(QuestionEvent::Deliver {
                message: m,
                how: zeron_orch::How::Ack,
            }),
        )?;
        self.pump_all()?;
        self.wake.notify_one();
        self.task_events.notify_waiters();
        Ok(())
    }
    pub(super) fn deliver_native(
        &self,
        engine: &dyn RunEngine,
        parent: ThreadId,
        key: u64,
        generation: u32,
    ) -> Result<(), DriverError> {
        let s = self.driver.state()?;
        let Some(n) = s.natives.get(&(parent, key)) else {
            return Ok(());
        };
        if n.generation != generation || n.delivery != zeron_orch::Delivery::Claimed {
            return Ok(());
        }
        let chat = self.chat_of(parent)?;
        let tool = crate::orchestrator::native::tool_id(&self.driver, key)?;
        let doc = crate::orchestrator::native::binding(&self.driver, key)?
            .map(|binding| binding.doc_id)
            .unwrap_or_else(|| crate::orchestrator::native::doc_id(&chat, &tool));
        let result = engine
            .result_text(&doc)
            .unwrap_or_else(|| "Native task ended. Read its transcript for the result.".into());
        let message_id = format!("orch-native-result-{key}");
        let msg = self.msg(&message_id)?;
        self.stash(
            msg,
            &Pending {
                chat: chat.clone(),
                message_id,
                via: Via::Resume {
                    prompt: format!("Native task {doc} finished.\n{result}"),
                    worktree: None,
                },
            },
        )?;
        let event = Event::Run(zeron_orch::RunEvent::ServerEnqueue { t: parent, msg });
        let applied = self.driver.apply_if(
            &format!("native-queue:{parent:?}:{key}"),
            event,
            &enabled(event),
        )?;
        if !matches!(
            applied,
            Some(crate::orchestrator::driver::ApplyResult {
                applied: crate::orchestrator::store::Applied::Accepted { .. },
                ..
            })
        ) {
            return Err(DriverError::Effect(
                "DeliverNative".into(),
                "native result queue is full; delivery remains pending".into(),
            ));
        }
        crate::orchestrator::native::delivered(&self.driver, parent, key, generation)?;
        self.pump_all()?;
        engine.kick_drain(&chat);
        self.wake.notify_one();
        Ok(())
    }
}

impl OrchWire {
    pub fn answer_input_command(
        &self,
        agent: &str,
        child: &str,
        request: &str,
        epoch: Option<u32>,
        answers: &[zeron_proto::UserInputAnswer],
    ) -> Result<Option<String>, crate::EngineError> {
        let out = (|| -> Result<Option<String>, DriverError> {
            let Some(c) = self.known_thread(child)? else {
                return Ok(None);
            };
            let Some(qid) = self.driver.meta(&format!("question:{c:?}:{request}"))? else {
                return Ok(None);
            };
            let qid: u64 = qid
                .parse()
                .map_err(|_| DriverError::Corrupt("invalid question id".into()))?;
            let a = self
                .known_thread(agent)?
                .ok_or_else(|| DriverError::Corrupt("question answerer is unknown".into()))?;
            let s = self.driver.state()?;
            let q = s
                .questions
                .get(&(c, qid))
                .ok_or_else(|| DriverError::Corrupt("question is unknown".into()))?;
            let epoch =
                epoch.ok_or_else(|| DriverError::Corrupt("question epoch is required".into()))?;
            if !q.pending || q.epoch != epoch || q.holder != Some(a) || !q.seen.contains(&a) {
                return Err(DriverError::Corrupt(
                    "question answerer or epoch is stale".into(),
                ));
            }
            crate::orchestrator::questions::answer(&self.driver, Some(a), c, qid, epoch, answers)?;
            Ok(Some(format!("question-answer:{child}:{qid}:{epoch}")))
        })();
        self.wake.notify_one();
        out.map_err(|e| self.to_engine(e))
    }
}

impl OrchWire {
    pub(super) fn remote_question_message(
        &self,
        engine: &dyn RunEngine,
        m: zeron_orch::Message,
    ) -> Result<Option<zeron_doc::OrchMessage>, DriverError> {
        use zeron_doc::OrchBody;
        use zeron_orch::Message;
        let s = self.driver.state()?;
        let (child, qid, epoch) = match m {
            Message::QuestionUp {
                child, qid, epoch, ..
            }
            | Message::AnswerDown {
                child, qid, epoch, ..
            }
            | Message::QuestionClosed {
                child, qid, epoch, ..
            }
            | Message::QuestionRelease {
                child, qid, epoch, ..
            } => (child, qid, epoch),
            _ => return Ok(None),
        };
        let leaf = self.chat_of(child)?;
        let request = self
            .driver
            .meta(&format!("question-id:{qid}"))?
            .ok_or_else(|| DriverError::Corrupt("question request missing".into()))?;
        let wire_id = serde_json::to_string(&(&leaf, &request))?;
        let hop_below = |ancestor: ThreadId| -> Result<ThreadId, DriverError> {
            let mut hop = child;
            while let Some(p) = s
                .thread(hop)
                .map_err(|_| DriverError::Corrupt("question route missing".into()))?
                .parent
            {
                if p == ancestor {
                    return Ok(hop);
                }
                hop = p;
            }
            Err(DriverError::Corrupt("question ancestor missing".into()))
        };
        let (sender, target, body) = match m {
            Message::QuestionUp { to: Some(to), .. } => {
                let target = self.chat_of(to)?;
                if !self.remote_shadow(&target)? {
                    return Ok(None);
                }
                let sender = self.chat_of(hop_below(to)?)?;
                let th = s
                    .thread(child)
                    .map_err(|_| DriverError::Corrupt("question child missing".into()))?;
                let parent = th.parent.map(|p| self.chat_of(p)).transpose()?;
                let body = self
                    .driver
                    .meta(&format!("question-body:{qid}"))?
                    .unwrap_or_default();
                let payload = serde_json::json!({"leaf":leaf,"request":request,"questions":body,"parent":parent,"depth":th.depth,"access":th.access,"cohort":th.task.map(|k|k.cohort).unwrap_or(1)});
                (
                    sender,
                    target,
                    OrchBody::Question {
                        question_id: wire_id,
                        epoch,
                        payload,
                    },
                )
            }
            Message::AnswerDown { via, .. } | Message::QuestionRelease { via, .. } => {
                let target = self.chat_of(hop_below(via)?)?;
                if !self.remote_shadow(&target)? {
                    return Ok(None);
                }
                let sender = self.chat_of(via)?;
                let release = matches!(m, Message::QuestionRelease { .. });
                let answers = if let Message::AnswerDown { answer, .. } = m {
                    self.driver
                        .meta(&format!("question-answer-body:{answer}"))?
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let payload = serde_json::json!({"leaf":leaf,"request":request,"answers":answers,"release":release});
                let question_id = if release {
                    format!("{wire_id}:release")
                } else {
                    wire_id
                };
                (
                    sender,
                    target,
                    OrchBody::Answer {
                        question_id,
                        epoch,
                        payload,
                    },
                )
            }
            Message::QuestionClosed { to, .. } => {
                let target = self.chat_of(to)?;
                if !self.remote_shadow(&target)? {
                    return Ok(None);
                }
                let sender = self.chat_of(hop_below(to)?)?;
                if engine
                    .orch_route(&sender)
                    .is_none_or(|r| r.host != self.driver.device())
                {
                    return Ok(None);
                }
                (
                    sender,
                    target,
                    OrchBody::Closed {
                        question_id: wire_id,
                        epoch,
                    },
                )
            }
            _ => return Ok(None),
        };
        self.remote_message(engine, &sender, &target, body)
            .map(Some)
    }
    pub(super) fn remote_question_event(
        &self,
        engine: &dyn RunEngine,
        message: &zeron_doc::OrchMessage,
    ) -> Result<Option<Event>, DriverError> {
        use zeron_doc::OrchBody;
        let (wire_id, epoch) = match &message.body {
            OrchBody::Question {
                question_id, epoch, ..
            }
            | OrchBody::Answer {
                question_id, epoch, ..
            }
            | OrchBody::Closed { question_id, epoch } => (question_id, *epoch),
            _ => return Ok(None),
        };
        let wire_id = wire_id.strip_suffix(":release").unwrap_or(wire_id);
        let (leaf, request): (String, String) = serde_json::from_str(wire_id)?;
        let mut lineage = std::collections::HashSet::new();
        let mut cursor = Some(leaf.clone());
        while let Some(chat) = cursor {
            if !lineage.insert(chat.clone()) {
                return Err(DriverError::Corrupt("question route cycle".into()));
            }
            let route = engine
                .orch_route(&chat)
                .ok_or_else(|| DriverError::Corrupt("question route missing".into()))?;
            if route.host != self.driver.device() {
                self.driver
                    .put_meta(&format!("remote-host:{chat}"), &route.host)?;
            }
            cursor = route.parent;
        }
        if !lineage.contains(&message.sender_chat) || !lineage.contains(&message.target_chat) {
            return Err(DriverError::Corrupt(
                "question route is outside the leaf lineage".into(),
            ));
        }
        let child = self.intern_thread(&leaf)?;
        let to = self.intern_thread(&message.target_chat)?;
        let qid = crate::orchestrator::questions::request_id(&self.driver, child, &request)?;
        match &message.body {
            OrchBody::Question { payload, .. } => {
                if !self.driver.state()?.threads.contains_key(&child) {
                    let parent_chat = payload["parent"]
                        .as_str()
                        .ok_or_else(|| DriverError::Corrupt("question parent missing".into()))?;
                    let p = self.intern_thread(parent_chat)?;
                    let route = engine.orch_route(parent_chat).ok_or_else(|| {
                        DriverError::Corrupt("question parent route missing".into())
                    })?;
                    let ancestor = route
                        .parent
                        .as_deref()
                        .map(|a| self.intern_thread(a))
                        .transpose()?;
                    let leaf_route = engine.orch_route(&leaf).ok_or_else(|| {
                        DriverError::Corrupt("question leaf route missing".into())
                    })?;
                    if leaf_route.parent.as_deref() != Some(parent_chat) {
                        return Err(DriverError::Corrupt("question leaf parent mismatch".into()));
                    }
                    self.driver
                        .put_meta(&format!("remote-host:{leaf}"), &leaf_route.host)?;
                    let depth = payload["depth"]
                        .as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| DriverError::Corrupt("question depth missing".into()))?;
                    let access = payload["access"]
                        .as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| DriverError::Corrupt("question access missing".into()))?;
                    let cohort = payload["cohort"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(1);
                    self.driver.apply(
                        &format!("question-import-child:{leaf}"),
                        Event::Remote(zeron_orch::remote::RemoteEvent::RemoteImport {
                            parent: p,
                            child,
                            ancestor,
                            depth,
                            access,
                            cohort,
                        }),
                    )?;
                }
                self.driver.put_meta(
                    &format!("question-body:{qid}"),
                    payload["questions"].as_str().unwrap_or_default(),
                )?;
                Ok(Some(Event::Question(QuestionEvent::ImportUp {
                    child,
                    qid,
                    epoch,
                    to,
                })))
            }
            OrchBody::Answer { payload, .. } => {
                let via = self.intern_thread(&message.sender_chat)?;
                if payload["release"].as_bool() == Some(true) {
                    return Ok(Some(Event::Question(QuestionEvent::ImportRelease {
                        child,
                        qid,
                        epoch,
                        via,
                    })));
                }
                let body = payload["answers"]
                    .as_str()
                    .ok_or_else(|| DriverError::Corrupt("question answer missing".into()))?;
                let _: Vec<zeron_proto::UserInputAnswer> = serde_json::from_str(body)?;
                let (answer, _) = self.driver.intern(
                    &format!("remote-answer:{}", message.id),
                    "question-answer-counter",
                )?;
                self.driver
                    .put_meta(&format!("question-answer-body:{answer}"), body)?;
                Ok(Some(Event::Question(QuestionEvent::ImportAnswer {
                    child,
                    qid,
                    epoch,
                    via,
                    answer,
                })))
            }
            OrchBody::Closed { .. } => Ok(Some(Event::Question(QuestionEvent::ImportClosed {
                child,
                qid,
                epoch,
                to,
            }))),
            _ => Ok(None),
        }
    }
}

impl OrchWire {
    pub fn native_tasks(&self, parent_chat: &str) -> Result<Vec<serde_json::Value>, DriverError> {
        let Some(parent) = self.known_thread(parent_chat)? else {
            return Ok(Vec::new());
        };
        let s = self.driver.state()?;
        s.natives.iter().filter(|((p,_),_)|*p==parent).map(|((_,key),n)|{
            let tool=crate::orchestrator::native::tool_id(&self.driver,*key)?;
            let binding=crate::orchestrator::native::binding(&self.driver,*key)?;
            let doc=binding.as_ref().map(|b|b.doc_id.clone()).unwrap_or_else(||crate::orchestrator::native::doc_id(parent_chat,&tool));
            let status=match n.status{zeron_orch::native::NativeStatus::Running=>"running",zeron_orch::native::NativeStatus::Completed=>"completed",zeron_orch::native::NativeStatus::Failed|zeron_orch::native::NativeStatus::Orphaned=>"failed"};
            Ok(serde_json::json!({"taskId":doc,"childThreadId":doc,"origin":"native","status":status,"background":n.background,"nativePath":binding.as_ref().map(|b|&b.path),"ownBackground":binding.as_ref().map(|b|b.own_background),"delivery":n.delivery,"run":n.attempt.run.ord,"attempt":n.attempt.n,"sendable":false}))
        }).collect()
    }
}

#[cfg(test)]
mod native_delivery_tests {
    use super::*;
    static SAVE_FAILURES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    struct Engine;
    #[async_trait::async_trait]
    impl RunEngine for Engine {
        async fn start(&self, _: &Pending) -> Result<String, EngineError> {
            unreachable!()
        }
        async fn steer(&self, _: &Pending) -> Result<Ran, EngineError> {
            unreachable!()
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
        fn last_done(&self, _: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)> {
            None
        }
        fn journal_seq(&self, _: &str) -> u64 {
            0
        }
        fn create_child(&self, _: &str, _: &ChildSpec) -> Result<(), EngineError> {
            unreachable!()
        }
        fn save_native_snapshot(&self, _: &str, snapshot: &[u8]) -> Result<(), EngineError> {
            if SAVE_FAILURES
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |n| n.checked_sub(1),
                )
                .is_ok()
            {
                return Err(EngineError::Other("one snapshot save fault".into()));
            }
            assert_eq!(snapshot, b"FINAL_UNIQUE_NATIVE_TAIL");
            Ok(())
        }
        fn result_text(&self, _: &str) -> Option<String> {
            Some("final native result".into())
        }
        fn reports_mcp_results(&self, _: &str) -> bool {
            false
        }
        fn task_results(
            &self,
            _: &str,
            _: &str,
            _: Vec<zeron_doc::TaskResultPart>,
        ) -> Result<(), EngineError> {
            Ok(())
        }
        fn kick_drain(&self, _: &str) {}
    }
    #[test]
    fn full_native_queue_retains_delivery_and_retries_once() {
        use zeron_orch::{ProviderEvent, native::NativeEvent};
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let mut opts = OrchOptions::default();
        opts.config.max_queue = 1;
        let wire = OrchWire::open(store, "device", opts).unwrap();
        let t = wire.thread("parent").unwrap();
        let apply = |key: &str, event| {
            assert!(matches!(
                wire.driver.apply(key, event).unwrap().applied,
                Applied::Accepted { .. }
            ));
        };
        apply("send", Event::Run(RunEvent::ServerEnqueue { t, msg: 1 }));
        apply("start", Event::Run(RunEvent::StartFromQueue { t }));
        wire.driver.put_meta("native-tool:42", "tool").unwrap();
        apply(
            "native",
            Event::Native(NativeEvent::Start {
                parent: t,
                key: 42,
                background: true,
            }),
        );
        let th = wire.driver.state().unwrap().thread(t).unwrap().clone();
        let ev = ProviderEvent {
            node: Node::Root,
            run: th.rord(),
            attempt: th.attempt.n,
            outcome: Outcome::Completed,
        };
        apply(
            "terminal",
            Event::Run(RunEvent::ProviderTerminal {
                t,
                run: ev.run,
                attempt: ev.attempt,
                outcome: ev.outcome,
            }),
        );
        apply("end", Event::Run(RunEvent::IngestEnd { t, ev }));
        apply("fill", Event::Run(RunEvent::ServerEnqueue { t, msg: 2 }));
        apply(
            "native-end",
            Event::Native(NativeEvent::End {
                parent: t,
                key: 42,
                failed: false,
            }),
        );
        let generation = wire
            .driver
            .state()
            .unwrap()
            .natives
            .get(&(t, 42))
            .unwrap()
            .generation;
        assert!(wire.deliver_native(&Engine, t, 42, generation).is_err());
        assert_eq!(
            wire.driver
                .state()
                .unwrap()
                .natives
                .get(&(t, 42))
                .unwrap()
                .delivery,
            zeron_orch::Delivery::Claimed
        );
        assert!(
            !wire
                .driver
                .trace_lines()
                .unwrap()
                .iter()
                .any(|line| line.contains("native-queue:")),
            "full queue must not consume a receipt"
        );
        apply("drain", Event::Run(RunEvent::StartFromQueue { t }));
        wire.deliver_native(&Engine, t, 42, generation).unwrap();
        wire.deliver_native(&Engine, t, 42, generation).unwrap();
        let s = wire.driver.state().unwrap();
        assert_eq!(
            s.natives.get(&(t, 42)).unwrap().delivery,
            zeron_orch::Delivery::Delivered
        );
        assert_eq!(s.thread(t).unwrap().queue.len(), 1);
    }
    #[test]
    fn native_terminal_snapshot_failure_recovers_after_reopen() {
        for status in [DoneStatus::Completed, DoneStatus::Interrupted] {
            replay_terminal_after_fault(status);
        }
    }
    fn replay_terminal_after_fault(status: DoneStatus) {
        use zeron_orch::{ProviderEvent, native::NativeEvent};
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let wire = OrchWire::open(store.clone(), "device", OrchOptions::default()).unwrap();
        let t = wire.thread("parent").unwrap();
        let apply = |key: &str, event| {
            assert!(matches!(
                wire.driver.apply(key, event).unwrap().applied,
                Applied::Accepted { .. }
            ));
        };
        apply("send", Event::Run(RunEvent::ServerEnqueue { t, msg: 1 }));
        apply("start", Event::Run(RunEvent::StartFromQueue { t }));
        wire.driver.put_meta("native-tool:42", "tool").unwrap();
        wire.driver
            .put_meta(&format!("native-key:{t:?}:tool"), "42")
            .unwrap();
        apply(
            "native",
            Event::Native(NativeEvent::Start {
                parent: t,
                key: 42,
                background: true,
            }),
        );
        let th = wire.driver.state().unwrap().thread(t).unwrap().clone();
        let ev = ProviderEvent {
            node: Node::Root,
            run: th.rord(),
            attempt: th.attempt.n,
            outcome: Outcome::Completed,
        };
        apply(
            "terminal",
            Event::Run(RunEvent::ProviderTerminal {
                t,
                run: ev.run,
                attempt: ev.attempt,
                outcome: ev.outcome,
            }),
        );
        apply("end", Event::Run(RunEvent::IngestEnd { t, ev }));
        wire.stage_native_terminal(
            "parent",
            "tool",
            "parent--sub--tool",
            b"FINAL_UNIQUE_NATIVE_TAIL".to_vec(),
            status != DoneStatus::Completed,
        )
        .unwrap();
        SAVE_FAILURES.store(1, std::sync::atomic::Ordering::SeqCst);
        assert!(wire.retry_native_terminals(&Engine).is_err());
        assert_eq!(
            wire.driver.state().unwrap().natives[&(t, 42)].status,
            zeron_orch::native::NativeStatus::Running
        );
        assert!(
            wire.driver
                .meta(&format!("native-terminal:{t:?}:42"))
                .unwrap()
                .is_some()
        );
        drop(wire);
        let wire = OrchWire::open(store, "device", OrchOptions::default()).unwrap();
        wire.retry_native_terminals(&Engine).unwrap();
        wire.retry_native_terminals(&Engine).unwrap();
        assert!(
            wire.driver
                .meta(&format!("native-terminal:{t:?}:42"))
                .unwrap()
                .is_none()
        );
        let state = wire.driver.state().unwrap();
        assert_eq!(
            state.natives[&(t, 42)].status,
            if status == DoneStatus::Completed {
                zeron_orch::native::NativeStatus::Completed
            } else {
                zeron_orch::native::NativeStatus::Failed
            }
        );
        assert_eq!(
            state.natives[&(t, 42)].delivery,
            zeron_orch::Delivery::Claimed
        );
        let generation = state.natives[&(t, 42)].generation;
        wire.deliver_native(&Engine, t, 42, generation).unwrap();
        wire.deliver_native(&Engine, t, 42, generation).unwrap();
        assert_eq!(
            wire.driver.state().unwrap().natives[&(t, 42)].delivery,
            zeron_orch::Delivery::Delivered
        );
        let accepted = wire
            .driver
            .trace_lines()
            .unwrap()
            .iter()
            .filter(|line| {
                line.contains("NativeEnd")
                    && line.contains("\"ok\":true")
                    && line.contains("\"dup\":false")
            })
            .count();
        assert_eq!(accepted, 1);
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NativeTerminal {
    doc: String,
    snapshot: Vec<u8>,
    failed: bool,
}
impl OrchWire {
    pub(crate) fn stage_native_terminal(
        &self,
        chat: &str,
        tool: &str,
        doc: &str,
        snapshot: Vec<u8>,
        failed: bool,
    ) -> Result<(), DriverError> {
        self.stage_native_terminal_path(chat, &[tool.to_owned()], doc, snapshot, failed)
    }
    pub(crate) fn stage_native_terminal_path(
        &self,
        chat: &str,
        path: &[String],
        doc: &str,
        snapshot: Vec<u8>,
        failed: bool,
    ) -> Result<(), DriverError> {
        let Some(parent) = self.known_thread(chat)? else {
            return Ok(());
        };
        let Some(key_id) = crate::orchestrator::native::path_key(&self.driver, parent, path)?
        else {
            return Ok(());
        };
        let key = key_id.to_string();
        if !self
            .driver
            .state()?
            .natives
            .get(&(parent, key_id))
            .is_some_and(|n| n.status == zeron_orch::native::NativeStatus::Running)
        {
            return Ok(());
        }
        if self
            .driver
            .meta(&format!("native-terminal:{parent:?}:{key}"))?
            .is_some()
        {
            self.wake.notify_one();
            return Ok(());
        }
        self.driver.put_meta(
            &format!("native-terminal:{parent:?}:{key}"),
            &serde_json::to_string(&NativeTerminal {
                doc: doc.into(),
                snapshot,
                failed,
            })?,
        )?;
        self.wake.notify_one();
        Ok(())
    }
    pub(super) fn retry_native_terminals(&self, engine: &dyn RunEngine) -> Result<(), DriverError> {
        for ((parent, key), _) in self.driver.state()?.natives.iter() {
            let receipt = format!("native-terminal:{parent:?}:{key}");
            let Some(raw) = self.driver.meta(&receipt)? else {
                continue;
            };
            let terminal: NativeTerminal = serde_json::from_str(&raw)?;
            engine
                .save_native_snapshot(&terminal.doc, &terminal.snapshot)
                .map_err(|e| DriverError::Effect("NativeTerminal".into(), e.to_string()))?;
            if self
                .driver
                .state()?
                .natives
                .get(&(*parent, *key))
                .is_some_and(|n| n.status != zeron_orch::native::NativeStatus::Running)
            {
                self.driver.delete_meta(&receipt)?;
                continue;
            }
            let event = Event::Native(zeron_orch::native::NativeEvent::End {
                parent: *parent,
                key: *key,
                failed: terminal.failed,
            });
            let applied = self
                .driver
                .apply(&format!("native-end:{parent:?}:{key}"), event)?;
            if !matches!(applied.applied, Applied::Accepted { .. }) {
                return Err(DriverError::Effect(
                    "NativeTerminal".into(),
                    "native terminal was rejected".into(),
                ));
            }
            self.driver.delete_meta(&receipt)?;
        }
        Ok(())
    }
}

impl OrchWire {
    pub fn native_binding(
        &self,
        key: u64,
    ) -> Result<Option<crate::orchestrator::native::NativeBinding>, DriverError> {
        crate::orchestrator::native::binding(&self.driver, key)
    }
    pub(crate) fn native_path_running(
        &self,
        chat: &str,
        path: &[String],
    ) -> Result<bool, DriverError> {
        let Some(parent) = self.known_thread(chat)? else {
            return Ok(false);
        };
        let Some(key) = crate::orchestrator::native::path_key(&self.driver, parent, path)? else {
            return Ok(false);
        };
        if self
            .driver
            .meta(&format!("native-terminal:{parent:?}:{key}"))?
            .is_some()
        {
            return Ok(false);
        }
        Ok(self
            .driver
            .state()?
            .natives
            .get(&(parent, key))
            .is_some_and(|n| n.status == zeron_orch::native::NativeStatus::Running))
    }
    pub(crate) fn native_path_known(
        &self,
        chat: &str,
        path: &[String],
    ) -> Result<bool, DriverError> {
        let Some(parent) = self.known_thread(chat)? else {
            return Ok(false);
        };
        Ok(crate::orchestrator::native::path_key(&self.driver, parent, path)?.is_some())
    }
}

impl OrchWire {
    pub(crate) fn native_activate_path(
        &self,
        chat: &str,
        path: &[String],
    ) -> Result<(), DriverError> {
        if let Some(parent) = self.known_thread(chat)? {
            for end in 1..=path.len() {
                crate::orchestrator::native::activate(&self.driver, parent, &path[..end])?;
            }
        }
        Ok(())
    }
}

impl OrchWire {
    pub(super) fn reconcile_absent_natives(
        &self,
        engine: &dyn RunEngine,
        chat: &str,
    ) -> Result<(), EngineError> {
        let Some(parent) = self
            .known_thread(chat)
            .map_err(|e| EngineError::Other(e.to_string()))?
        else {
            return Ok(());
        };
        let state = self
            .driver
            .state()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        for ((owner, key), task) in state.natives.iter() {
            if *owner != parent || task.status != zeron_orch::native::NativeStatus::Running {
                continue;
            }
            let Some(binding) = self
                .native_binding(*key)
                .map_err(|e| EngineError::Other(e.to_string()))?
            else {
                continue;
            };
            let snapshot = engine.native_snapshot(&binding.doc_id)?;
            let raw = loro::LoroDoc::new();
            raw.import(&snapshot)
                .map_err(|e| EngineError::Other(e.to_string()))?;
            let doc = zeron_doc::SessionDoc::from_doc(raw);
            for entry in doc.read_entries()? {
                if entry.status == Some(zeron_doc::MessageStatus::Streaming) {
                    doc.set_message_status(&entry.id, zeron_doc::MessageStatus::Aborted)?;
                }
            }
            let snapshot = doc.export_snapshot()?;
            self.stage_native_terminal_path(chat, &binding.path, &binding.doc_id, snapshot, true)
                .map_err(|e| EngineError::Other(e.to_string()))?;
        }
        self.retry_native_terminals(engine)
            .map_err(|e| EngineError::Other(e.to_string()))
    }
}

impl OrchWire {
    pub(crate) fn native_documents_ready(&self) {
        self.wake.notify_one();
    }
}
