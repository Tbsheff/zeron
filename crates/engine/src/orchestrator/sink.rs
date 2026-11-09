use std::time::Duration;

use zeron_proto::{DoneStatus, FailureClass};

use super::wire::{Pending, Ran, RunEngine, Via};
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
    ) -> Result<String, EngineError> {
        let host = self.host();
        let request = self.last_request(&p.chat).or_else(|| {
            host.as_ref()
                .and_then(|h| h.request_from_chat_row(&p.chat, prompt))
        });
        let Some(mut request) = request else {
            return Err(EngineError::Other("no run config for this chat".into()));
        };
        request.prompt = prompt.to_owned();
        request.attachments = attachments.to_vec();
        request.resume = None;
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
                _ => self.fresh_start(p, prompt, attachments).await,
            },
            Via::Resume { prompt } => self.fresh_start(p, prompt, &[]).await,
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

    fn kick_drain(&self, _chat: &str) {
        if let Some(h) = self.host() {
            h.kick_drains();
        }
    }
}
