use super::*;
use crate::plugins::Plugins;
use crate::plugins::hooks::{self, Dispatch};

const MIN_WAKE_MS: i64 = 1_000;

pub(super) enum RowHook {
    Go,
    Hold,
    Again,
}

impl DocHost {
    fn dispatch_deciders(&self) -> Option<Plugins> {
        self.sessions()?
            .plugins()
            .filter(|p| p.hooks(hooks::DISPATCH))
    }

    pub(super) async fn dispatch_hook(
        &self,
        handle: &Arc<ChatDocHandle>,
        head: &QueuedMessage,
    ) -> RowHook {
        let Some(plugins) = self.dispatch_deciders() else {
            return RowHook::Go;
        };
        let plugin = lock(&self.inner.plugin_rows).get(&head.id).cloned();
        let source = if plugin.is_some() { "plugin" } else { "queue" };
        match self
            .decide_dispatch(&plugins, handle, source, plugin.as_deref(), &head.text)
            .await
        {
            Dispatch::Proceed => {}
            Dispatch::Wait { .. } => return RowHook::Hold,
            Dispatch::Reject { plugin, message } => {
                if let Ok(Some(row)) = handle.doc.take_queued(&head.id) {
                    lock(&self.inner.plugin_rows).remove(&row.id);
                    handle.publish_queue();
                    self.refuse_send(handle, &row.id, &row.text, row.issued_at, &plugin, &message);
                }
                return RowHook::Again;
            }
        }
        let busy_now = self
            .sessions()
            .is_some_and(|s| s.turn_in_flight(&handle.chat_id))
            || handle.queue_paused.load(Ordering::Acquire);
        if busy_now {
            return RowHook::Hold;
        }
        let head_now = handle
            .doc
            .read_queue()
            .ok()
            .and_then(|q| q.into_iter().next());
        if head_now.as_ref() != Some(head) {
            return RowHook::Again;
        }
        lock(&self.inner.plugin_rows).remove(&head.id);
        RowHook::Go
    }

    pub(super) async fn run_hook(
        &self,
        handle: &Arc<ChatDocHandle>,
        entry: &SessionCommandEntry,
        disposition: &CommandDisposition,
    ) -> RowHook {
        let (
            CommandDisposition::Execute,
            SessionCommandPayload::Run {
                request,
                message_id,
            },
        ) = (disposition, &entry.payload)
        else {
            return RowHook::Go;
        };
        let Some(plugins) = self.dispatch_deciders() else {
            return RowHook::Go;
        };
        match self
            .decide_dispatch(&plugins, handle, "person", None, &request.prompt)
            .await
        {
            Dispatch::Proceed => RowHook::Go,
            Dispatch::Wait { .. } => RowHook::Hold,
            Dispatch::Reject { plugin, message } => {
                if let Err(err) = self.inner.store.mark_processed(&entry.id) {
                    tracing::error!(chat = %handle.chat_id, error = %err, "processed-ledger write failed; halting drain");
                    return RowHook::Hold;
                }
                let said = format!("{plugin}: {message}");
                self.resolve_command(
                    handle,
                    &entry.id,
                    SessionCommandStatus::Rejected,
                    Some(&said),
                );
                self.refuse_send(
                    handle,
                    message_id,
                    &request.prompt,
                    entry.issued_at,
                    &plugin,
                    &message,
                );
                RowHook::Again
            }
        }
    }

    async fn decide_dispatch(
        &self,
        plugins: &Plugins,
        handle: &Arc<ChatDocHandle>,
        source: &str,
        plugin: Option<&str>,
        text: &str,
    ) -> Dispatch {
        let ctx = hooks::dispatch_ctx(&handle.chat_id, source, plugin, text);
        let decided = plugins.decide_dispatch(ctx).await;
        if let Dispatch::Wait {
            plugin,
            reason,
            until,
        } = &decided
        {
            tracing::info!(chat = %handle.chat_id, %plugin, %reason, ?until, "queued send held by a plugin");
            if let Some(until) = until {
                self.arm_dispatch_wake(handle, until.saturating_mul(1000));
            }
        }
        decided
    }

    fn arm_dispatch_wake(&self, handle: &Arc<ChatDocHandle>, until_ms: i64) {
        let at = until_ms.max(now_ms() + MIN_WAKE_MS);
        {
            let mut wakes = lock(&self.inner.dispatch_wakes);
            if wakes.get(&handle.chat_id).is_some_and(|armed| *armed <= at) {
                return;
            }
            wakes.insert(handle.chat_id.clone(), at);
        }
        let (host, weak, chat) = (self.clone(), Arc::downgrade(handle), handle.chat_id.clone());
        self.spawn_worker(async move {
            let wait = u64::try_from(at.saturating_sub(now_ms())).unwrap_or(0);
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            {
                let mut wakes = lock(&host.inner.dispatch_wakes);
                if wakes.get(&chat) == Some(&at) {
                    wakes.remove(&chat);
                }
            }
            let Some(handle) = weak.upgrade() else { return };
            host.drain_commands(&handle).await;
            host.drain_queue(&handle).await;
        });
    }

    fn refuse_send(
        &self,
        handle: &Arc<ChatDocHandle>,
        message_id: &str,
        text: &str,
        issued_at: i64,
        plugin: &str,
        message: &str,
    ) {
        let said = match message {
            "" => format!("Not sent: {plugin} held it back"),
            why => format!("Not sent: {plugin} held it back ({why})"),
        };
        let notice = SessionMessageEntry {
            id: format!("refused:{message_id}"),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Error {
                id: "e0".into(),
                message: said,
            }],
            created_at: now_ms(),
            device_id: handle.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        let written = handle
            .write_user_message(message_id, text, issued_at)
            .and_then(|()| handle.doc.push_message(&notice));
        if let Err(err) = written {
            tracing::warn!(chat = %handle.chat_id, error = %err, "refused send write failed");
        }
        self.save_snapshot(handle);
    }
}
