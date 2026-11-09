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

    async fn fresh_start(
        &self,
        p: &Pending,
        prompt: &str,
        attachments: &[String],
        worktree: Option<zeron_proto::WorktreeSpec>,
    ) -> Result<String, EngineError> {
        let host = self.host();
        let request = self.last_request(&p.chat).or_else(|| {
            host.as_ref()
                .and_then(|h| h.request_from_chat_row(&p.chat, prompt))
        });
        let Some(mut request) = request else {
            return Err(EngineError::Other("no run config for this chat".into()));
        };
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
