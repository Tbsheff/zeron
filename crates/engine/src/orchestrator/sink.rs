use std::time::Duration;

use zeron_proto::{DoneStatus, FailureClass};

use super::wire::{ChildSpec, Pending, Ran, RunEngine, Via};
use crate::EngineError;
use crate::sessions::{SessionsEngine, SteerOutcome};

const SETTLE: Duration = Duration::from_secs(3);

impl SessionsEngine {
    async fn settle_turn(&self, chat: &str) {
        let deadline = tokio::time::Instant::now() + SETTLE;
        while self.turn_in_flight(chat) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn capture_access_config(
        &self, p: &Pending, access: zeron_proto::AccessMode, source: &str, origin: Option<&str>,
    ) -> Result<(), EngineError> {
        let Some(wire) = self.orchestrator().filter(|wire| wire.driver().mechanism_capture_enabled()) else {
            return Ok(());
        };
        let host = self.host();
        let Some(workspace) = host.as_ref().and_then(|host| host.workspace()) else {
            return Ok(());
        };
        let row = workspace.chat(&p.chat)?;
        let configured = row.as_ref().and_then(|row| row.config.as_ref()).map(|config| config.access);
        let policy_source = if configured.is_some() { "workspace_row" }
            else if source == "request_default" { "request_default" } else { "unknown" };
        let configured = configured.or_else(|| (source == "request_default").then(zeron_proto::AccessMode::default));
        let selected = super::wire::access_level(access);
        let configured = configured.map(super::wire::access_level);
        let submitted = matches!(source, "dispatch" | "prior_request").then_some(selected);
        wire.driver().observe_mechanism(
            &format!("access-config:{}:{}:{source}:{}:{selected}:{configured:?}", p.chat, p.message_id, origin.unwrap_or("unknown")),
            "access_config", serde_json::json!({
                "chat":p.chat, "access":selected, "configured_access":configured,
                "submitted_access":submitted, "config_source":source, "policy_source":policy_source,
                "workspace_row_present":row.is_some(), "origin_id":origin,
                "message_id":p.message_id, "validated":true,
            }),
        ).map_err(|error| EngineError::Other(format!("orchestrator: {error}")))
    }

    async fn fresh_start(
        &self,
        p: &Pending,
        prompt: &str,
        attachments: &[String],
        worktree: Option<zeron_proto::WorktreeSpec>,
    ) -> Result<String, EngineError> {
        let host = self.host();
        let (mut request, source, origin) = if let Some((request, origin)) = self.last_request_source(&p.chat) {
            (request, "prior_request", Some(origin))
        } else {
            let (request, configured) = host.as_ref().and_then(|h| h.request_from_chat_row_source(&p.chat, prompt))
                .ok_or_else(|| EngineError::Other("no run config for this chat".into()))?;
            (request, if configured { "workspace_row" } else { "request_default" }, Some(p.message_id.clone()))
        };
        self.capture_access_config(p, request.access, source, origin.as_deref())?;
        if let Some(th) = self
            .orchestrator()
            .and_then(|wire| wire.thread_state(&p.chat))
        {
            if super::wire::access_level(request.access) > th.access {
                request.access = zeron_proto::AccessMode::ALL[usize::from(th.access)];
            }
        }
        request.prompt = prompt.to_owned();
        request.attachments = attachments.to_vec();
        request.resume = None;
        request.worktree = None;
        if let Some(spec) = worktree {
            let host = host
                .as_ref()
                .ok_or_else(|| EngineError::Other("no host for task worktree".into()))?;
            request.cwd = host
                .prepare_task_worktree(&p.chat, &p.message_id, &spec)
                .await?;
        }
        let harness = match (&host, request.harness) {
            (Some(h), _) => h.harness_for_request(&p.chat, &request),
            (None, Some(h)) => h,
            (None, None) => return Err(EngineError::Other("no harness for this chat".into())),
        };
        self.dispatch_with(
            &p.chat,
            harness,
            request,
            Some(p.message_id.clone()),
            false,
            0,
            None,
        )
        .await
    }

    async fn direct_steer(
        &self,
        p: &Pending,
        prompt: &str,
        attachments: &[String],
        issued_at: i64,
    ) -> Result<SteerOutcome, EngineError> {
        self.steer_now_direct(
            &p.chat,
            prompt,
            attachments.to_vec(),
            Some(p.message_id.clone()),
            issued_at,
        )
        .await
    }
}

#[async_trait::async_trait]
impl RunEngine for SessionsEngine {
    fn effective_access(&self, chat: &str) -> Option<u8> {
        self.last_request(chat)
            .map(|request| super::wire::access_level(request.access))
    }

    async fn start(&self, p: &Pending) -> Result<String, EngineError> {
        self.settle_turn(&p.chat).await;
        match &p.via {
            Via::Dispatch { harness, request } => {
                self.capture_access_config(p, request.access, "dispatch", Some(&p.message_id))?;
                self.dispatch_with(
                    &p.chat,
                    *harness,
                    (**request).clone(),
                    Some(p.message_id.clone()),
                    false,
                    0,
                    None,
                )
                .await
            }
            Via::Steer {
                prompt,
                attachments,
                issued_at,
            } => match self
                .direct_steer(p, prompt, attachments, *issued_at)
                .await?
            {
                SteerOutcome::Accepted => Ok(String::new()),
                _ => self.fresh_start(p, prompt, attachments, None).await,
            },
            Via::Resume { prompt, worktree } => {
                self.fresh_start(p, prompt, &[], worktree.clone()).await
            }
        }
    }

    async fn steer(&self, p: &Pending) -> Result<Ran, EngineError> {
        match &p.via {
            Via::Steer {
                prompt,
                attachments,
                issued_at,
            } => Ok(Ran::Steered(
                self.direct_steer(p, prompt, attachments, *issued_at)
                    .await?,
            )),
            Via::Dispatch { harness, request } => self
                .dispatch_with(
                    &p.chat,
                    *harness,
                    (**request).clone(),
                    Some(p.message_id.clone()),
                    false,
                    0,
                    None,
                )
                .await
                .map(Ran::Started),
            Via::Resume { .. } => Ok(Ran::Nothing),
        }
    }

    async fn stop_attempt(&self, chat: &str) -> bool {
        self.stop_turn(chat).await.unwrap_or(false)
    }

    async fn interrupt_run(&self, chat: &str) -> bool {
        self.terminate(chat).await.unwrap_or(false)
    }

    fn agent_attempt(&self, chat: &str) -> Option<super::wire::AgentRuntime> {
        self.agent_attempt(chat)
    }

    fn owns_run(&self, chat: &str) -> bool {
        self.has_run(chat) || self.reviving_any(chat)
    }

    fn last_done(&self, chat: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)> {
        self.journal_last_done(chat)
    }

    fn journal_seq(&self, chat: &str) -> u64 {
        self.journal_seq(chat)
    }

    fn orch_route(&self, chat: &str) -> Option<super::placement::ChatRoute> {
        self.orch_route(chat)
    }

    fn orch_supports(&self, device: &str) -> bool {
        self.orch_supports(device)
    }

    fn orch_send(&self, message: &zeron_doc::OrchMessage) -> Result<bool, String> {
        self.host()
            .ok_or_else(|| "no document host".to_owned())?
            .send_orch(message)
    }

    fn create_child(&self, parent: &str, spec: &ChildSpec) -> Result<(), EngineError> {
        self.orch_create_child(parent, spec)
    }

    fn result_text(&self, chat: &str) -> Option<String> {
        self.orch_result_text(chat)
    }

    fn task_results(
        &self,
        chat: &str,
        message_id: &str,
        results: Vec<zeron_doc::TaskResultPart>,
    ) -> Result<(), EngineError> {
        self.orch_task_results(chat, message_id, results)
    }

    fn reports_mcp_results(&self, chat: &str) -> bool {
        self.orch_reports_mcp_results(chat)
    }

    fn respond_question(
        &self,
        chat: &str,
        request: &str,
        answers: Vec<zeron_proto::UserInputAnswer>,
    ) -> Result<bool, EngineError> {
        self.orch_respond_question(chat, request, answers)
    }

    fn native_snapshot(&self, doc: &str) -> Result<Vec<u8>, EngineError> {
        let host=self.host().ok_or_else(||EngineError::Other("native document host unavailable".into()))?;
        Ok(host.open(doc)?.doc().export_snapshot()?)
    }
    fn save_native_snapshot(&self, doc: &str, snapshot: &[u8]) -> Result<(), EngineError> {
        let host = self
            .host()
            .ok_or_else(|| EngineError::Other("native document host unavailable".into()))?;
        let handle = host.open(doc)?;
        handle
            .doc()
            .doc()
            .import(snapshot)
            .map_err(|error| EngineError::Other(error.to_string()))?;
        host.persist_fork(&handle)
    }
    fn native_changed_task(&self, chat:&str,key:u64,tool:&str,status:&str)->Result<(),EngineError>{
        self.orch_native_changed_task(chat,key,tool,status)
    }
    fn native_changed(&self, chat: &str, tool: &str, status: &str) -> Result<(), EngineError> {
        self.orch_native_changed(chat, tool, status)
    }
    fn notify_question(&self, chat: &str, _request: &str) {
        self.orch_notify_question(chat);
    }

    fn kick_drain(&self, _chat: &str) {
        if let Some(h) = self.host() {
            h.kick_drains();
        }
    }
}
