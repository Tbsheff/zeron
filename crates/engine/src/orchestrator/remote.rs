use super::{ChildSpec, DelegateRequest, OrchError, OrchWire, RunEngine, accepted_ids, lock};
use crate::orchestrator::{
    DriverError,
    placement::{self, ChatRoute, Registry},
    store::Applied,
};
use zeron_doc::{OrchBody, OrchMessage};
use zeron_orch::remote::RemoteEvent;
use zeron_orch::{Effect, Event, Message, RunStatus};

struct Routes<'a>(&'a dyn RunEngine);
impl Registry for Routes<'_> {
    fn chat(&self, chat: &str) -> Option<ChatRoute> {
        self.0.orch_route(chat)
    }
    fn supports_orch(&self, device: &str) -> bool {
        self.0.orch_supports(device)
    }
}
fn error(message: impl Into<String>) -> DriverError {
    DriverError::Corrupt(message.into())
}
fn route(engine: &dyn RunEngine, chat: &str) -> Result<ChatRoute, DriverError> {
    engine
        .orch_route(chat)
        .ok_or_else(|| error(format!("no positive route for {chat}")))
}
fn body_status(status: &str) -> Result<RunStatus, DriverError> {
    match status {
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        "interrupted" => Ok(RunStatus::Interrupted),
        "usageLimit" => Ok(RunStatus::UsageLimit),
        _ => Err(error("invalid remote outcome")),
    }
}
fn status_body(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Interrupted => "interrupted",
        RunStatus::UsageLimit => "usageLimit",
        RunStatus::Live => "live",
    }
}
impl OrchWire {
    pub(super) fn validate_placement(
        &self,
        engine: &dyn RunEngine,
        req: &DelegateRequest,
    ) -> Result<(), OrchError> {
        let device = req
            .child
            .device_id
            .as_deref()
            .unwrap_or(self.driver.device());
        if device == self.driver.device() {
            return Ok(());
        }
        let denied = |message: &str| OrchError {
            code: "capability_denied",
            message: message.into(),
        };
        if !engine.orch_supports(device) {
            return Err(denied("Target device does not advertise orch-v2."));
        }
        if !engine
            .orch_route(&req.origin_chat_id)
            .is_some_and(|r| r.host == self.driver.device())
        {
            return Err(denied("Delegate only from a chat this device hosts."));
        }
        if let Some(existing) = engine.orch_route(&req.child.chat_id) {
            if existing.host != device || existing.parent.as_deref() != Some(&req.origin_chat_id) {
                return Err(denied("Child route conflicts with this task."));
            }
        }
        Ok(())
    }

    pub(super) fn remote_shadow(&self, chat: &str) -> Result<bool, DriverError> {
        Ok(self
            .driver
            .meta(&format!("remote-host:{chat}"))?
            .is_some_and(|host| host != self.driver.device()))
    }

    pub(super) fn remote_result(&self, engine: &dyn RunEngine, chat: &str) -> Option<String> {
        self.driver
            .meta(&format!("remote-result:{chat}"))
            .ok()
            .flatten()
            .or_else(|| engine.result_text(chat))
    }

    pub(super) fn pump_shadow(
        &self,
        chat: &str,
        t: zeron_orch::ThreadId,
        fx: &mut Vec<String>,
    ) -> Result<(), DriverError> {
        let s = self.driver.state()?;
        let Ok(th) = s.thread(t) else {
            return Ok(());
        };
        if th.task.is_some() && self.driver.meta(&format!("child:{chat}"))?.is_some() {
            let ev = Event::Remote(RemoteEvent::RemoteClamp {
                child: t,
                access: th.access,
            });
            let key = match th.task {
                Some(task) if task.run > 1 && task.run > th.rord() => {
                    format!("remote-followup:{chat}:{}", task.run)
                }
                _ => format!("remote-clamp:{chat}:{}", th.access),
            };
            let r = self.driver.apply(&key, ev)?;
            fx.extend(accepted_ids(&r));
        }
        Ok(())
    }

    pub(super) fn remote_message(
        &self,
        engine: &dyn RunEngine,
        sender: &str,
        target: &str,
        body: OrchBody,
    ) -> Result<OrchMessage, DriverError> {
        let source = route(engine, sender)?;
        if source.host != self.driver.device() {
            return Err(error("remote send on wrong host"));
        }
        let mut message = OrchMessage {
            id: String::new(),
            sender_chat: sender.into(),
            target_chat: target.into(),
            sender_device: self.driver.device().into(),
            body,
        };
        message.id = message.expected_id();
        Ok(message)
    }
    pub(super) fn transmit(
        &self,
        engine: &dyn RunEngine,
        message: &OrchMessage,
    ) -> Result<(), DriverError> {
        let target = route(engine, &message.target_chat)?;
        let room = if message.upward() {
            &message.sender_chat
        } else {
            &message.target_chat
        };
        placement::validate(&Routes(engine), &target.host, room, message)
            .map_err(|e| error(e.to_string()))?;
        placement::require_capability(&Routes(engine), &target.host)
            .map_err(|e| error(e.to_string()))?;
        engine.orch_send(message).map_err(error)?;
        let body = match &message.body {
            OrchBody::CreateChild {
                depth,
                access,
                cohort,
                ..
            } => {
                serde_json::json!({"type":"createChild","depth":depth,"access":access,"cohort":cohort})
            }
            OrchBody::FollowUp {
                run,
                access,
                cohort,
                ..
            } => serde_json::json!({"type":"followUp","run":run,"access":access,"cohort":cohort}),
            OrchBody::Notice { run, outcome, .. } => {
                serde_json::json!({"type":"notice","run":run,"outcome":outcome})
            }
            OrchBody::Stop { epoch } => serde_json::json!({"type":"stop","epoch":epoch}),
            OrchBody::Clamp { epoch, access } => {
                serde_json::json!({"type":"clamp","epoch":epoch,"access":access})
            }
            OrchBody::Question {
                question_id, epoch, ..
            } => serde_json::json!({"type":"question","question_id":question_id,"epoch":epoch}),
            OrchBody::Answer {
                question_id,
                epoch,
                payload,
            } => {
                serde_json::json!({"type":"answer","question_id":question_id,"epoch":epoch,"release":payload.get("release").and_then(|v|v.as_bool()).unwrap_or(false)})
            }
            OrchBody::Closed { question_id, epoch } => {
                serde_json::json!({"type":"closed","question_id":question_id,"epoch":epoch})
            }
            OrchBody::Offer { run, generation } => {
                serde_json::json!({"type":"offer","run":run,"generation":generation})
            }
        };
        self.driver.observe_mechanism(
            &format!("doc:{}", message.id),
            "doc_commit",
            serde_json::json!({
                "wire_id":message.id,"sender":message.sender_chat,"target":message.target_chat,
                "device":self.driver.device(),"target_host":target.host,"room":room,
                "upward":message.upward(),"body":body,"already_present":null,
            }),
        )?;
        let sender = self
            .known_thread(&message.sender_chat)?
            .ok_or_else(|| error("send has no sender partition"))?;
        self.driver.apply(
            &format!("remote-send:{}", message.id),
            Event::Remote(RemoteEvent::RemoteFence { t: sender }),
        )?;
        Ok(())
    }

    pub(super) fn remote_effect(
        &self,
        engine: &dyn RunEngine,
        effect: &Effect,
    ) -> Result<bool, DriverError> {
        match effect {
            Effect::CreateChild {
                parent,
                child,
                depth,
                access,
                ..
            } => {
                let c = self.chat_of(*child)?;
                if !self.remote_shadow(&c)? {
                    return Ok(false);
                }
                let p = self.chat_of(*parent)?;
                let spec = self.spec_of(&c)?;
                let host = spec
                    .device_id
                    .as_deref()
                    .ok_or_else(|| error("child has no host"))?;
                placement::require_capability(&Routes(engine), host)
                    .map_err(|e| error(e.to_string()))?;
                if route(engine, &p)?.host != self.driver.device() {
                    return Err(error("create on wrong parent host"));
                }
                engine
                    .create_child(&p, &spec)
                    .map_err(|e| error(e.to_string()))?;
                let cohort = self
                    .driver
                    .state()?
                    .task(*child)
                    .map_err(|_| error("no remote task"))?
                    .cohort;
                let message = self.remote_message(
                    engine,
                    &p,
                    &c,
                    OrchBody::CreateChild {
                        depth: *depth,
                        access: *access,
                        cohort,
                        spec: serde_json::to_value(spec)?,
                    },
                )?;
                self.transmit(engine, &message)?;
                Ok(true)
            }
            Effect::Send(m) => {
                if let Some(message) = self.remote_question_message(engine, *m)? {
                    self.transmit(engine, &message)?;
                    self.driver.apply(
                        &format!("remote-sent:{}", message.id),
                        Event::Remote(RemoteEvent::RemoteSent { message: *m }),
                    )?;
                    return Ok(true);
                }
                let s = self.driver.state()?;
                let (sender, target, body) = match *m {
                    Message::Notice { child, run } => {
                        let c = self.chat_of(child)?;
                        let parent = s
                            .thread(child)
                            .ok()
                            .and_then(|th| th.parent)
                            .ok_or_else(|| error("child has no parent"))?;
                        let p = self.chat_of(parent)?;
                        if !self.remote_shadow(&p)? {
                            return Ok(false);
                        }
                        let status = s
                            .thread(child)
                            .ok()
                            .and_then(|th| th.run(run))
                            .map(|r| r.status)
                            .ok_or_else(|| error("notice has no terminal run"))?;
                        if status == RunStatus::Live {
                            return Err(error("notice for live run"));
                        }
                        (
                            c.clone(),
                            p,
                            OrchBody::Notice {
                                run,
                                outcome: status_body(status).into(),
                                summary: engine.result_text(&c).unwrap_or_default(),
                            },
                        )
                    }
                    Message::Stop { child, stop } => {
                        let c = self.chat_of(child)?;
                        if !self.remote_shadow(&c)? {
                            return Ok(false);
                        }
                        let p = route(engine, &c)?
                            .parent
                            .ok_or_else(|| error("stop has no parent route"))?;
                        (p, c, OrchBody::Stop { epoch: stop })
                    }
                    _ => return Ok(false),
                };
                let message = self.remote_message(engine, &sender, &target, body)?;
                self.transmit(engine, &message)?;
                self.driver.apply(
                    &format!("remote-sent:{}", message.id),
                    Event::Remote(RemoteEvent::RemoteSent { message: *m }),
                )?;
                Ok(true)
            }
            Effect::Reconfigure { t, access } => {
                let c = self.chat_of(*t)?;
                if !self.remote_shadow(&c)? {
                    return Ok(false);
                }
                let p = route(engine, &c)?
                    .parent
                    .ok_or_else(|| error("clamp has no parent route"))?;
                let state = self.driver.state()?;
                let th = state
                    .thread(*t)
                    .map_err(|_| error("missing followup shadow"))?;
                let body = match th.task {
                    Some(task) if task.run > 1 && task.run > th.rord() => OrchBody::FollowUp {
                        run: task.run,
                        prompt: self.spec_of(&c)?.task,
                        access: *access,
                        cohort: task.cohort,
                    },
                    _ => OrchBody::Clamp {
                        epoch: u32::from(*access),
                        access: *access,
                    },
                };
                let message = self.remote_message(engine, &p, &c, body)?;
                self.transmit(engine, &message)?;
                Ok(true)
            }
            Effect::StartRun { run, .. }
            | Effect::InterruptRun { run }
            | Effect::ArmTimer { run } => {
                if self.remote_shadow(&self.chat_of(run.thread)?)? {
                    return Err(error("execution effect targets a foreign host"));
                }
                Ok(false)
            }
            Effect::StartAttempt { attempt, .. }
            | Effect::InterruptAttempt { attempt }
            | Effect::SteerRun { attempt, .. } => {
                if self.remote_shadow(&self.chat_of(attempt.run.thread)?)? {
                    return Err(error("attempt effect targets a foreign host"));
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    pub fn receive_remote(
        &self,
        engine: &dyn RunEngine,
        message: &OrchMessage,
    ) -> Result<(), DriverError> {
        self.alive()?;
        let room = if message.upward() {
            &message.sender_chat
        } else {
            &message.target_chat
        };
        placement::validate(&Routes(engine), self.driver.device(), room, message)
            .map_err(|e| error(e.to_string()))?;
        placement::require_capability(&Routes(engine), &message.sender_device)
            .map_err(|e| error(e.to_string()))?;
        let key = format!("remote-recv:{}", message.id);
        let _g = lock(&self.gate);
        if self.driver.has_receipt(&key)? {
            return Ok(());
        }
        let target = self.intern_thread(&message.target_chat)?;
        let ev = match &message.body {
            OrchBody::CreateChild {
                depth,
                access,
                cohort,
                spec,
            } => {
                let mut spec: ChildSpec = serde_json::from_value(spec.clone())?;
                if spec.chat_id != message.target_chat
                    || spec.device_id.as_deref() != Some(self.driver.device())
                {
                    return Err(error("CreateChild spec route mismatch"));
                }
                if let Some(config) = &mut spec.config {
                    config.access = super::tasks::level_mode(*access);
                }
                let p = self.intern_thread(&message.sender_chat)?;
                let parent = route(engine, &message.sender_chat)?;
                let ancestor = parent
                    .parent
                    .as_deref()
                    .map(|a| self.intern_thread(a))
                    .transpose()?;
                self.driver.put_meta(
                    &format!("remote-host:{}", message.sender_chat),
                    &message.sender_device,
                )?;
                if let Some(a) = parent.parent {
                    let h = route(engine, &a)?.host;
                    self.driver.put_meta(&format!("remote-host:{a}"), &h)?;
                }
                self.driver.put_meta(
                    &format!("child:{}", message.target_chat),
                    &serde_json::to_string(&spec)?,
                )?;
                Event::Remote(RemoteEvent::RemoteImport {
                    parent: p,
                    child: target,
                    ancestor,
                    depth: *depth,
                    access: *access,
                    cohort: *cohort,
                })
            }
            OrchBody::Notice {
                run,
                outcome,
                summary,
            } => {
                let child = self
                    .known_thread(&message.sender_chat)?
                    .ok_or_else(|| error("notice has no task shadow"))?;
                let state = self.driver.state()?;
                if !state.thread(child).is_ok_and(|th| {
                    th.parent == Some(target) && th.task.is_some_and(|task| task.run == *run)
                }) {
                    return Err(error("notice task/run mismatch"));
                }
                let status = body_status(outcome)?;
                self.driver
                    .put_meta(&format!("remote-result:{}", message.sender_chat), summary)?;
                Event::Remote(RemoteEvent::RemoteNotice {
                    child,
                    run: *run,
                    status,
                })
            }
            OrchBody::FollowUp {
                run,
                prompt,
                access,
                cohort,
            } => {
                let event = Event::Remote(RemoteEvent::RemoteFollowup {
                    child: target,
                    run: *run,
                    cohort: *cohort,
                    access: *access,
                });
                if zeron_orch::step(&self.driver.state()?, event).is_err() {
                    return Err(error("followup transition not ready"));
                }
                let mut spec = self.spec_of(&message.target_chat)?;
                spec.task = prompt.clone();
                self.driver.put_meta(
                    &format!("child:{}", message.target_chat),
                    &serde_json::to_string(&spec)?,
                )?;
                Event::Remote(RemoteEvent::RemoteFollowup {
                    child: target,
                    run: *run,
                    cohort: *cohort,
                    access: *access,
                })
            }
            OrchBody::Stop { epoch } => Event::Remote(RemoteEvent::RemoteStop {
                child: target,
                stop: *epoch,
            }),
            OrchBody::Clamp { access, .. } => {
                let state = self.driver.state()?;
                let current = state
                    .thread(target)
                    .map_err(|_| error("clamp before create"))?
                    .access;
                if *access < current {
                    Event::Del(zeron_orch::DelEvent::NarrowAccess {
                        t: target,
                        access: *access,
                    })
                } else {
                    Event::Remote(RemoteEvent::RemoteFence { t: target })
                }
            }
            _ => self
                .remote_question_event(engine, message)?
                .ok_or_else(|| error("unsupported remote orchestration body"))?,
        };
        let result = self
            .driver
            .apply_if(&key, ev, &super::enabled(ev))?
            .ok_or_else(|| error("remote transition not ready"))?;
        if let Applied::Rejected { reason, .. } = result.applied {
            return Err(error(format!("remote transition rejected: {reason:?}")));
        }
        let mut fx = accepted_ids(&result);
        self.pump(target, &message.target_chat, &mut fx)?;
        drop(_g);
        self.wake.notify_one();
        self.task_events.notify_waiters();
        engine.kick_drain(&message.target_chat);
        Ok(())
    }
}
