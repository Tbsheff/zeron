use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeron_orch::{
    DelEvent, DelegateMode, Delivery, Event, Hold, How, Message, OrchState, OutboxEvent, Reject,
    RunId, RunStatus, Task, TaskId, TaskSt, Thread, ThreadId, WakeRule,
};
use zeron_proto::{AccessMode, AgentEvent, ChatConfig, ToolCall, WorktreeSpec};

use super::{OrchWire, Pending, RunEngine, Via, accepted_ids, enabled, lock};
use crate::orchestrator::DriverError;
use crate::orchestrator::store::Applied;

pub const RUNNING_NEXT: &str = "Results arrive in your next turn as a task-results message. End your turn or keep working; do not poll.";
const INLINE_MAX: usize = 16 * 1024;
const TIMEOUT_NEXT: &str =
    "Still running. The result is pushed to you when it settles; do not poll.";
const WAIT_DEFAULT: Duration = Duration::from_secs(600);
const WAIT_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSpec {
    pub chat_id: String,
    #[serde(default)]
    pub space_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub config: Option<ChatConfig>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub task: String,
    #[serde(default)]
    pub worktree: Option<WorktreeSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegateRequest {
    #[serde(default)]
    pub origin_chat_id: String,
    pub task: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub client_request_id: Option<String>,
    #[serde(default)]
    pub observed_input: Option<serde_json::Value>,
    #[serde(default)]
    pub followup: bool,
    pub child: ChildSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub origin: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    pub task_id: String,
    pub child_thread_id: String,
    pub status: &'static str,
    pub summary: Option<String>,
    pub has_pending_question: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<serde_json::Value>,
    pub wait_timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for OrchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

fn err(code: &'static str, message: impl Into<String>) -> OrchError {
    OrchError {
        code,
        message: message.into(),
    }
}

impl From<DriverError> for OrchError {
    fn from(e: DriverError) -> Self {
        err("orchestration_error", e.to_string())
    }
}

enum PlaceError {
    Refused(OrchError),
    Driver(DriverError),
}

impl From<OrchError> for PlaceError {
    fn from(e: OrchError) -> Self {
        Self::Refused(e)
    }
}

impl From<DriverError> for PlaceError {
    fn from(e: DriverError) -> Self {
        Self::Driver(e)
    }
}

fn refused(reason: Reject) -> OrchError {
    match reason {
        Reject::DepthCap => err(
            "capability_denied",
            "Delegation is capped at depth 2 (root, child, grandchild). Do this work in this chat.",
        ),
        Reject::FanoutCap => err(
            "capability_denied",
            "8 delegated tasks are still running. Wait for one to finish (its result wakes you) or task_cancel one.",
        ),
        Reject::NotLive | Reject::UnknownThread => err(
            "parent_not_active",
            "Delegate from a running turn of a chat this device hosts.",
        ),
        Reject::InterruptRequested => err(
            "parent_not_active",
            "This chat was stopped; delegation is refused until the person sends again.",
        ),
        Reject::Waiting => err(
            "invalid_request",
            "A wait is already open in this turn. Wait for it, or delegate with mode async.",
        ),
        Reject::AccessWider => err(
            "runtime_mode_escalation_denied",
            "A child cannot have wider access than this chat.",
        ),
        other => err("orchestration_error", format!("refused: {other:?}")),
    }
}

pub fn access_level(access: AccessMode) -> u8 {
    let i = AccessMode::ALL
        .iter()
        .position(|m| *m == access)
        .unwrap_or(0);
    u8::try_from(i).unwrap_or(0)
}

pub(super) fn level_mode(level: u8) -> AccessMode {
    AccessMode::ALL
        .get(usize::from(level))
        .copied()
        .unwrap_or(AccessMode::Supervised)
}

pub(super) fn clamp_access(p: &mut Pending, level: u8) {
    if let Via::Dispatch { request, .. } = &mut p.via
        && access_level(request.access) > level
    {
        request.access = level_mode(level);
    }
}

fn status_word(th: &Thread, task: &Task) -> &'static str {
    if task.st == TaskSt::Settled {
        let out = th.run(task.run).map_or(th.last_out(), |r| Some(r.status));
        return match out {
            Some(RunStatus::Interrupted) if th.held == Hold::Stop => "cancelled",
            Some(RunStatus::Interrupted) => "interrupted",
            Some(RunStatus::Failed | RunStatus::UsageLimit) => "failed",
            _ => "completed",
        };
    }
    if task.delivery == Delivery::Disposed {
        "cancelled"
    } else if th.rord() < task.run {
        "queued"
    } else {
        "running"
    }
}

fn wakes_parent(task: &Task) -> bool {
    task.st == TaskSt::Running && !task.delivery.handled() && task.wake == WakeRule::Always
}

fn cap(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn split_task(task_id: &str) -> Option<(&str, u32)> {
    let (chat, run) = task_id.rsplit_once(':')?;
    Some((chat, run.parse().ok()?))
}

fn is_task_tool(tool: &str) -> bool {
    matches!(
        tool,
        "delegate_task" | "task_status" | "mcp__zeron__delegate_task" | "mcp__zeron__task_status"
    )
}

struct WaitGuard<'a> {
    wire: &'a OrchWire,
    origin: String,
    child: String,
    armed: bool,
    attempt: Option<zeron_orch::AttemptId>,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.wire
                .abandon_wait(&self.origin, &self.child, self.attempt);
        }
    }
}

impl OrchWire {
    pub fn narrow_access_now(
        &self,
        chat: &str,
        access: zeron_proto::AccessMode,
    ) -> Result<(), OrchError> {
        let Some(t) = self.known_thread(chat)? else {
            return Ok(());
        };
        let _g = lock(&self.gate);
        let ev = Event::Del(DelEvent::NarrowAccess {
            t,
            access: access_level(access),
        });
        let key = format!("narrow:{chat}:{}", self.driver.bump("narrows")?);
        if let Some(result) = self.driver.apply_if(&key, ev, &enabled(ev))?
            && matches!(result.applied, Applied::Accepted { .. })
        {
            self.record_local_ceiling(chat, access_level(access), &key)?;
        }
        self.wake.notify_one();
        Ok(())
    }

    pub async fn narrow_access(
        &self,
        engine: &dyn RunEngine,
        chat: &str,
        access: zeron_proto::AccessMode,
    ) -> Result<(), OrchError> {
        self.narrow_access_now(chat, access)?;
        let _ = self.run_effects(engine, None).await;
        Ok(())
    }

    pub async fn followup(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        child: &str,
        prompt: &str,
        request_id: &str,
    ) -> Result<TaskView, OrchError> {
        self.alive()?;
        let key = format!("mcp:{origin}:followup:{request_id}");
        let fx = {
            let _g = lock(&self.gate);
            self.admit_agent(engine, origin)?;
            if self.driver.has_receipt(&key)? {
                let first = self
                    .driver
                    .meta(&format!("task:{key}"))?
                    .ok_or_else(|| err("orchestration_error", "No task for receipt."))?;
                return self.task_status_view(engine, origin, &first);
            }
            let (p, c) = self.ids(origin, child)?;
            let state = self.driver.state()?;
            let access = state
                .thread(c)
                .map_err(|_| refused(Reject::UnknownThread))?
                .access;
            let ev = Event::Del(DelEvent::Followup {
                parent: p,
                child: c,
                access,
            });
            if let Err(reason) = zeron_orch::step(&state, ev) {
                return Err(refused(reason));
            }
            let next = state
                .thread(c)
                .map_err(|_| refused(Reject::UnknownThread))?
                .rord()
                + 1;
            let old_view = self.view_of(engine, origin, child, false)?;
            self.driver.put_meta(
                &format!("task-view:{origin}:{}", old_view.task_id),
                &serde_json::to_string(&old_view).map_err(DriverError::from)?,
            )?;
            let mut spec = self.spec_of(child)?;
            spec.task = prompt.to_owned();
            self.driver.put_meta(
                &format!("child:{child}"),
                &serde_json::to_string(&spec).map_err(DriverError::from)?,
            )?;
            self.driver
                .put_meta(&format!("task:{key}"), &format!("{child}:{next}"))?;
            let r = self.driver.apply_command(&key, None, ev)?;
            if let Applied::Rejected { reason, .. } = r.applied {
                return Err(refused(reason));
            }
            let mut fx = accepted_ids(&r);
            self.pump(c, child, &mut fx)?;
            fx
        };
        let _ = self.run_effects(engine, Some(&fx)).await;
        self.wake.notify_one();
        self.view_of(engine, origin, child, false)
    }

    pub(crate) fn with_agent_admission<T>(
        &self,
        engine: &dyn RunEngine,
        chat: &str,
        on_error: impl FnOnce(OrchError) -> T,
        f: impl FnOnce() -> T,
    ) -> T {
        let _g = lock(&self.gate);
        match self.admit_agent(engine, chat) {
            Ok(()) => f(),
            Err(error) => on_error(error),
        }
    }

    pub(crate) fn admit_agent(&self, engine: &dyn RunEngine, chat: &str) -> Result<(), OrchError> {
        if !matches!(zeron_rpc::caller(), zeron_rpc::Caller::Agent { .. }) {
            return Ok(());
        }
        let current = self.thread_state(chat).filter(|th| th.is_live()).map(|th| th.attempt_id());
        let handle = engine.agent_attempt(chat);
        let claimed = zeron_rpc::agent_run();
        let accepted = handle.as_ref().is_some_and(|handle| {
            claimed.as_ref() == Some(&handle.runtime_id) && current == Some(handle.attempt)
        });
        if self.driver.mechanism_capture_enabled() {
            self.driver.observe_mechanism(&format!("agent-admission:{}", uuid::Uuid::new_v4()), "agent_admission", serde_json::json!({
                "chat":chat,"tool_call_id":null,"runtime_id":claimed,
                "provider_run_id":handle.as_ref().map(|h| &h.runtime_id),
                "handle_source_id":handle.as_ref().and_then(|h|h.source_id.as_deref()),
                "handle_source_revision":handle.as_ref().and_then(|h|h.source_revision),
                "handle_run":handle.as_ref().map(|h|h.attempt.run.ord),"handle_attempt":handle.as_ref().map(|h|h.attempt.n),
                "current_run":current.map(|at|at.run.ord),"current_attempt":current.map(|at|at.n),
                "effective_access":handle.as_ref().and_then(|h|h.effective_access),"origin_id":handle.as_ref().and_then(|h|h.origin_id.as_deref()),
                "disposition":if accepted {"accepted"} else {"refused"},"reason":if accepted {"current_runtime_attempt"} else {"runtime_attempt_mismatch"},"validated":true
            }))?;
        }
        if !accepted {
            return Err(err("parent_not_active", "This MCP handle does not own the current attempt."));
        }
        Ok(())
    }

    pub async fn delegate(
        &self,
        engine: &dyn RunEngine,
        req: DelegateRequest,
    ) -> Result<TaskView, OrchError> {
        if !matches!(req.mode.as_deref(), None | Some("wait" | "async")) {
            return Err(err("invalid_request", "mode must be async or wait."));
        }
        let wait = req.mode.as_deref() == Some("wait");
        self.validate_placement(engine, &req)?;
        let (child, fx) = self.place(engine, &req, wait).map_err(|e| match e {
            PlaceError::Refused(e) => e,
            PlaceError::Driver(e) => self.fail(e).into(),
        })?;
        let _ = self.run_effects(engine, Some(&fx)).await;
        self.wake.notify_one();
        if !wait || fx.is_empty() {
            let id = req
                .client_request_id
                .as_deref()
                .unwrap_or(&req.child.chat_id);
            let key = format!("mcp:{}:delegate:{id}", req.origin_chat_id);
            if let Some(task) = self.driver.meta(&format!("task:{key}"))? {
                return self.task_status_view(engine, &req.origin_chat_id, &task);
            }
            return self.view_of(engine, &req.origin_chat_id, &child, false);
        }
        let timeout = req.timeout_ms.map_or(WAIT_DEFAULT, Duration::from_millis);
        self.await_wait(engine, &req.origin_chat_id, &child, timeout)
            .await
    }

    fn place(
        &self,
        engine: &dyn RunEngine,
        req: &DelegateRequest,
        wait: bool,
    ) -> Result<(String, Vec<String>), PlaceError> {
        self.alive()?;
        let _g = lock(&self.gate);
        let origin = &req.origin_chat_id;
        self.admit_agent(engine, origin).map_err(PlaceError::Refused)?;
        let id = req
            .client_request_id
            .as_deref()
            .unwrap_or(&req.child.chat_id);
        let key = format!("mcp:{origin}:delegate:{id}");
        if self.driver.has_receipt(&key)?
            && let Some(child) = self.driver.meta(&format!("crid:{key}"))?
        {
            return Ok((child, Vec::new()));
        }
        let t = self
            .known_thread(origin)?
            .ok_or_else(|| refused(Reject::UnknownThread))?;
        let child = req.child.chat_id.clone();
        if child.trim().is_empty() || child.len() > 256 {
            return Err(err("invalid_request", "Child chatId must have 1 to 256 bytes.").into());
        }
        let c = self.intern_thread(&child)?;
        if self.driver.state()?.thread(c).is_ok() {
            return Err(err(
                "invalid_request",
                "A delegated task needs a new child chatId.",
            )
            .into());
        }
        let spec = ChildSpec {
            task: req.task.clone(),
            title: req.title.clone().or_else(|| req.child.title.clone()),
            device_id: Some(
                req.child
                    .device_id
                    .clone()
                    .unwrap_or_else(|| self.driver.device().to_owned()),
            ),
            ..req.child.clone()
        };
        if spec.device_id.as_deref() != Some(self.driver.device()) {
            self.driver.put_meta(
                &format!("remote-host:{child}"),
                spec.device_id.as_deref().unwrap_or(""),
            )?;
        }
        self.driver.put_meta(
            &format!("child:{child}"),
            &serde_json::to_string(&spec).map_err(DriverError::from)?,
        )?;
        self.driver.put_meta(&format!("crid:{key}"), &child)?;
        self.driver
            .put_meta(&format!("task:{key}"), &format!("{child}:1"))?;
        if let Some(input) = &req.observed_input {
            self.driver.put_meta(
                &format!("input:{key}"),
                &serde_json::to_string(input).map_err(DriverError::from)?,
            )?;
        }
        let ev = Event::Del(DelEvent::Delegate {
            t,
            child: c,
            mode: if wait {
                DelegateMode::Wait
            } else {
                DelegateMode::Async
            },
            access: spec.config.as_ref().map_or(2, |c| access_level(c.access)),
            reconfigure: false,
        });
        let r = self.driver.apply_command(&key, None, ev)?;
        if let Applied::Rejected { reason, .. } = r.applied {
            self.driver.delete_meta(&format!("crid:{key}"))?;
            return Err(refused(reason).into());
        }
        if wait {
            let run = self.driver.state()?.thread(t).map_or(0, |th| th.rord());
            self.driver
                .put_meta(&format!("waitcall:{origin}:{run}"), &key)?;
        }
        let mut fx = accepted_ids(&r);
        self.pump(c, &child, &mut fx)?;
        Ok((child, fx))
    }

    async fn await_wait(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        child: &str,
        timeout: Duration,
    ) -> Result<TaskView, OrchError> {
        let mut guard = WaitGuard {
            wire: self,
            origin: origin.to_owned(),
            child: child.to_owned(),
            armed: true,
            attempt: self.thread_state(origin).map(|th| th.attempt_id()),
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let (settled, waiting) = self.wait_state(origin, child)?;
            if settled || !waiting {
                guard.armed = false;
                if settled && waiting && !engine.reports_mcp_results(origin) {
                    self.apply_wait(origin, child, guard.attempt, "return")?;
                }
                let view = self.view_of(engine, origin, child, false)?;
                self.record_tool_reply(origin, "delegate_task", &view)?;
                return Ok(view);
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                guard.armed = false;
                self.apply_wait(origin, child, guard.attempt, "timeout")?;
                let view = self.view_of(engine, origin, child, true)?;
                self.record_tool_reply(origin, "delegate_task", &view)?;
                return Ok(view);
            }
            let nap = WAIT_POLL.min(deadline - now);
            let _ = tokio::time::timeout(nap, self.task_events.notified()).await;
        }
    }

    fn ids(&self, origin: &str, child: &str) -> Result<(ThreadId, ThreadId), OrchError> {
        let t = self
            .known_thread(origin)?
            .ok_or_else(|| err("task_not_found", "No such task for this chat."))?;
        let c = self
            .known_thread(child)?
            .ok_or_else(|| err("task_not_found", "No such task for this chat."))?;
        Ok((t, c))
    }

    fn wait_state(&self, origin: &str, child: &str) -> Result<(bool, bool), OrchError> {
        let (t, c) = self.ids(origin, child)?;
        let s = self.driver.state()?;
        let settled = s.task(c).is_ok_and(|k| k.st == TaskSt::Settled)
            || s.questions
                .values()
                .any(|q| q.child == c && q.pending && q.holder == Some(t));
        let waiting = s
            .thread(t)
            .is_ok_and(|th| th.is_live() && th.wait_on == Some(c));
        Ok((settled, waiting))
    }

    fn apply_on(&self, key: &str, ev: Event, chat: &str, t: ThreadId) -> Result<(), DriverError> {
        let _g = lock(&self.gate);
        self.driver.apply_if(key, ev, &enabled(ev))?;
        self.pump(t, chat, &mut Vec::new())?;
        drop(_g);
        self.wake.notify_one();
        self.task_events.notify_waiters();
        Ok(())
    }

    fn apply_wait(
        &self,
        origin: &str,
        child: &str,
        attempt: Option<zeron_orch::AttemptId>,
        kind: &str,
    ) -> Result<(), DriverError> {
        let _g = lock(&self.gate);
        let (Some(t), Some(c), Some(at)) = (
            self.known_thread(origin)?,
            self.known_thread(child)?,
            attempt,
        ) else {
            return Ok(());
        };
        let s = self.driver.state()?;
        if !s.thread(t).is_ok_and(|th| {
            th.is_live() && !th.attempt.emitted && th.wait_on == Some(c) && th.attempt_id() == at
        }) {
            return Ok(());
        }
        let ev = Event::Del(match kind {
            "return" => DelEvent::WaitReturn { t },
            "timeout" => DelEvent::WaitTimeout { t },
            _ => DelEvent::WaitAbandon { t },
        });
        let key = format!("wait-{kind}:{origin}:{}:{}:{child}", at.run.ord, at.n);
        self.driver.apply_if(&key, ev, &enabled(ev))?;
        self.pump(t, origin, &mut Vec::new())?;
        self.wake.notify_one();
        self.task_events.notify_waiters();
        Ok(())
    }

    fn abandon_wait(&self, origin: &str, child: &str, attempt: Option<zeron_orch::AttemptId>) {
        if let Err(e) = self.apply_wait(origin, child, attempt, "abandon") {
            tracing::warn!(error = %e, "orchestrator wait abandon failed");
        }
    }

    pub fn task_status(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        task_id: &str,
    ) -> Result<TaskView, OrchError> {
        {
            let _g = lock(&self.gate);
            self.admit_agent(engine, origin)?;
        }
        let view = self.task_status_view(engine, origin, task_id)?;
        self.record_tool_reply(origin, "task_status", &view)?;
        Ok(view)
    }

    fn task_status_view(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        task_id: &str,
    ) -> Result<TaskView, OrchError> {
        if task_id.contains("--sub--") || task_id.contains("--native-path--") {
            return self.native_view(engine, origin, task_id);
        }
        let (child, _) =
            split_task(task_id).ok_or_else(|| err("invalid_request", "taskId is <chat>:<run>."))?;
        let view = self.view_of(engine, origin, child, false)?;
        if view.task_id != task_id {
            let json = self
                .driver
                .meta(&format!("task-view:{origin}:{task_id}"))?
                .ok_or_else(|| err("task_not_found", "No such task for this chat."))?;
            let v: serde_json::Value = serde_json::from_str(&json).map_err(DriverError::from)?;
            let status = match v["status"].as_str() {
                Some("failed") => "failed",
                Some("cancelled") => "cancelled",
                Some("interrupted") => "interrupted",
                _ => "completed",
            };
            let archived = TaskView {
                origin: "app",
                background: None,
                task_id: task_id.to_owned(),
                child_thread_id: child.to_owned(),
                status,
                summary: v["summary"].as_str().map(str::to_owned),
                has_pending_question: false,
                question: None,
                wait_timed_out: false,
                next: None,
            };
            return Ok(archived);
        }
        Ok(view)
    }

    pub async fn task_cancel(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        task_id: &str,
        request: Option<&str>,
    ) -> Result<(String, &'static str), OrchError> {
        if task_id.contains("--sub--") || task_id.contains("--native-path--") {
            return Err(err(
                "task_not_cancellable",
                "The harness owns native task cancellation.",
            ));
        }
        let key = format!("mcp:{origin}:cancel:{}", request.unwrap_or(task_id));
        let view = {
            let _g = lock(&self.gate);
            self.admit_agent(engine, origin)?;
            let target = self
                .driver
                .meta(&format!("target:{key}"))?
                .unwrap_or_else(|| task_id.to_owned());
            let view = self.task_status_view(engine, origin, &target)?;
            self.driver.put_meta(&format!("target:{key}"), &target)?;
            view
        };
        if !matches!(view.status, "running" | "queued" | "waiting") {
            self.record_tool_reply(origin, "task_cancel", &view)?;
            return Ok((view.task_id, view.status));
        }
        let (_, c) = self.ids(origin, &view.child_thread_id)?;
        let fx = {
            let _g = lock(&self.gate);
            self.admit_agent(engine, origin)?;
            let r = self
                .driver
                .apply_command(
                    &key,
                    None,
                    if self.remote_shadow(&view.child_thread_id)? {
                        Event::Remote(zeron_orch::remote::RemoteEvent::RemoteCancel { child: c })
                    } else {
                        Event::Del(DelEvent::Stop { t: c })
                    },
                )
                .map_err(|e| OrchError::from(self.fail(e)))?;
            accepted_ids(&r)
        };
        let _ = self.run_effects(engine, Some(&fx)).await;
        self.wake.notify_one();
        self.record_tool_reply(origin, "task_cancel", &view)?;
        Ok((view.task_id, "cancel_requested"))
    }

    fn view_of(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        child: &str,
        timed_out: bool,
    ) -> Result<TaskView, OrchError> {
        let (t, c) = self.ids(origin, child)?;
        let s = self.driver.state()?;
        let missing = || err("task_not_found", "No such task for this chat.");
        let th = s.thread(c).map_err(|_| missing())?;
        let task = th
            .task
            .filter(|_| th.parent == Some(t))
            .ok_or_else(missing)?;
        let pending_question = s.questions.values().find(|q| q.child == c && q.pending);
        let has_pending_question = pending_question.is_some();
        let question = pending_question
            .map(|q| -> Result<serde_json::Value, DriverError> {
                let body = self
                    .driver
                    .meta(&format!("question-body:{}", q.qid))?
                    .unwrap_or_default();
                let body: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                Ok(serde_json::json!({"questionId":q.qid,"epoch":q.epoch,"body":body}))
            })
            .transpose()?;
        let summary = (task.st == TaskSt::Settled)
            .then(|| {
                self.remote_result(engine, child)
                    .map(|r| cap(r, INLINE_MAX))
            })
            .flatten();
        Ok(TaskView {
            origin: "app",
            background: None,
            task_id: format!("{child}:{}", task.run),
            child_thread_id: child.to_owned(),
            status: if has_pending_question {
                "waiting"
            } else {
                status_word(th, &task)
            },
            summary,
            has_pending_question,
            question,
            wait_timed_out: timed_out,
            next: if timed_out {
                Some(TIMEOUT_NEXT)
            } else {
                wakes_parent(&task).then_some(RUNNING_NEXT)
            },
        })
    }

    pub fn tool_event(&self, chat: &str, event: &AgentEvent) {
        match event {
            AgentEvent::ToolCall {
                id,
                call:
                    ToolCall::Mcp {
                        server,
                        tool,
                        input,
                    },
            } if server == "zeron"
                && (is_task_tool(tool)
                    || (self.driver.mechanism_capture_enabled()
                        && tool.ends_with("task_cancel"))) =>
            {
                if let Some(th) = self
                    .thread_state(chat)
                    .filter(|th| th.is_live() && !th.attempt.emitted)
                {
                    lock(&self.calls).insert(
                        (chat.to_owned(), id.clone()),
                        (th.attempt_id(), tool.clone(), input.clone()),
                    );
                }
            }
            AgentEvent::ToolResult { id, is_error, .. } => {
                if !is_error {
                    if let Some(th) = self
                        .thread_state(chat)
                        .filter(|th| th.is_live() && !th.attempt.emitted)
                    {
                        if let Err(error) = self.record_tool_observed(chat, th.attempt_id(), id) {
                            tracing::warn!(chat, %error, "tool observation capture failed");
                        }
                    }
                }
                let call = lock(&self.calls).remove(&(chat.to_owned(), id.clone()));
                if let Some((at, tool, input)) = call.filter(|(at, _, _)| {
                    !is_error
                        && self.thread_state(chat).is_some_and(|th| {
                            th.is_live() && !th.attempt.emitted && th.attempt_id() == *at
                        })
                }) && let Err(e) = self.observed(chat, at, &tool, input.as_ref())
                {
                    tracing::warn!(chat, error = %e, "orchestrator ack failed");
                }
            }
            _ => {}
        }
    }

    fn observed(
        &self,
        chat: &str,
        at: zeron_orch::AttemptId,
        tool: &str,
        input: Option<&serde_json::Value>,
    ) -> Result<(), DriverError> {
        if tool.ends_with("task_cancel") {
            return Ok(());
        }
        let field = |k: &str| input.and_then(|i| i.get(k)).and_then(|v| v.as_str());
        if tool.ends_with("delegate_task") && field("mode") == Some("wait") {
            let run = self.thread_state(chat).map_or(0, |th| th.rord());
            let Some(key) = self.driver.meta(&format!("waitcall:{chat}:{run}"))? else {
                return Ok(());
            };
            if let Some(id) = field("clientRequestId") {
                if key != format!("mcp:{chat}:delegate:{id}") {
                    return Ok(());
                }
            }
            if let Some(expected) = self.driver.meta(&format!("input:{key}"))? {
                let expected: serde_json::Value = serde_json::from_str(&expected)?;
                if input != Some(&expected) {
                    return Ok(());
                }
            }
            let Some(child) = self.driver.meta(&format!("crid:{key}"))? else {
                return Ok(());
            };
            let Some(c) = self.known_thread(&child)? else {
                return Ok(());
            };
            if self
                .thread_state(chat)
                .is_some_and(|th| th.wait_on == Some(c))
            {
                return self.apply_wait(chat, &child, Some(at), "return");
            }
            return Ok(());
        }
        let Some((child, task_run)) = field("taskId").and_then(split_task) else {
            return Ok(());
        };
        let (Some(t), Some(c)) = (self.known_thread(chat)?, self.known_thread(child)?) else {
            return Ok(());
        };
        let _g = lock(&self.gate);
        let s = self.driver.state()?;
        if !s
            .thread(t)
            .is_ok_and(|th| th.is_live() && !th.attempt.emitted && th.attempt_id() == at)
            || !s.task(c).is_ok_and(|task| task.run == task_run)
        {
            return Ok(());
        }
        let run = at.run.ord;
        let ev = Event::Del(DelEvent::ReadTaskResult {
            p: t,
            c,
            run,
            task_run,
        });
        self.driver.apply_if(
            &format!("read:{chat}:{run}:{}:{child}:{task_run}", at.n),
            ev,
            &enabled(ev),
        )?;
        self.pump(t, chat, &mut Vec::new())?;
        self.wake.notify_one();
        self.task_events.notify_waiters();
        Ok(())
    }

    pub(super) fn create_child(
        &self,
        engine: &dyn RunEngine,
        parent: ThreadId,
        child: ThreadId,
    ) -> Result<(), DriverError> {
        let parent = self.chat_of(parent)?;
        let spec = self.spec_of(&self.chat_of(child)?)?;
        engine
            .create_child(&parent, &spec)
            .map_err(|e| DriverError::Corrupt(format!("create child: {e}")))
    }

    pub(super) fn spec_of(&self, chat: &str) -> Result<ChildSpec, DriverError> {
        let json = self
            .driver
            .meta(&format!("child:{chat}"))?
            .ok_or_else(|| DriverError::Corrupt(format!("no child spec for {chat}")))?;
        Ok(serde_json::from_str(&json)?)
    }

    pub(super) fn deliver(&self, engine: &dyn RunEngine, m: Message) -> Result<(), DriverError> {
        if matches!(
            m,
            Message::QuestionUp { .. }
                | Message::AnswerDown { .. }
                | Message::QuestionClosed { .. }
                | Message::QuestionRelease { .. }
        ) {
            return self.deliver_question(engine, m);
        }
        let s = self.driver.state()?;
        let (key, ev, t) = match m {
            Message::Notice { child, run } => {
                let p = s
                    .thread(child)
                    .ok()
                    .and_then(|th| th.parent)
                    .unwrap_or(child);
                let how = How::Ack;
                let ev = OutboxEvent::DoNotice { child, run, how };
                (format!("notice:{}:{run}", self.chat_of(child)?), ev, p)
            }
            Message::Offer {
                parent,
                run,
                generation,
            } => {
                let ev = OutboxEvent::DoOffer {
                    parent,
                    run,
                    generation,
                    how: How::Ack,
                };
                (
                    format!("offer:{}:{run}:{generation}", self.chat_of(parent)?),
                    ev,
                    parent,
                )
            }
            Message::Stop { child, stop } => {
                let ev = OutboxEvent::DoStop {
                    child,
                    stop,
                    how: How::Ack,
                };
                (format!("stop:{}:{stop}", self.chat_of(child)?), ev, child)
            }
            Message::QuestionUp { .. }
            | Message::AnswerDown { .. }
            | Message::QuestionClosed { .. }
            | Message::QuestionRelease { .. } => unreachable!("routed above"),
        };
        let chat = self.chat_of(t)?;
        self.apply_on(&key, Event::Outbox(ev), &chat, t)?;
        engine.kick_drain(&chat);
        Ok(())
    }

    pub(super) fn crash_waits(&self) -> Result<(), DriverError> {
        let boot = self.driver.bump("boots")?;
        let ev = Event::Outbox(OutboxEvent::CrashEngine);
        let _g = lock(&self.gate);
        self.driver
            .apply_if(&format!("boot:{boot}"), ev, &enabled(ev))?;
        Ok(())
    }

    pub(super) fn stash_delivered(
        &self,
        run: RunId,
        delivered: &[TaskId],
    ) -> Result<(), DriverError> {
        if delivered.is_empty() {
            return Ok(());
        }
        let key = format!("delivered:{}:{}", run.thread.0, run.ord);
        self.driver
            .put_meta(&key, &serde_json::to_string(delivered)?)
    }

    fn delivered_of(&self, run: RunId) -> Result<Vec<TaskId>, DriverError> {
        let key = format!("delivered:{}:{}", run.thread.0, run.ord);
        match self.driver.meta(&key)? {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    pub(super) fn task_pending(&self, run: RunId) -> Result<Pending, DriverError> {
        let chat = self.chat_of(run.thread)?;
        let spec = self.spec_of(&chat)?;
        Ok(Pending {
            message_id: format!("task-{chat}-{}", run.ord),
            via: Via::Resume {
                prompt: spec.task,
                worktree: spec.worktree.filter(|_| run.ord == 1),
            },
            chat,
        })
    }

    pub(super) fn wake_pending(
        &self,
        engine: &dyn RunEngine,
        run: RunId,
        cohort: u32,
        generation: u32,
    ) -> Result<Pending, DriverError> {
        let chat = self.chat_of(run.thread)?;
        let s = self.driver.state()?;
        let delivered = self.delivered_of(run)?;
        let prompt = self.results_text(engine, &s, &delivered)?;
        let message_id = format!("wake-{chat}-{cohort}-{generation}");
        let mut remaining = INLINE_MAX;
        let results = delivered
            .iter()
            .map(|id| {
                let child = self.chat_of(id.child)?;
                let body = self.remote_result(engine, &child).unwrap_or_default();
                let status = s
                    .thread(id.child)
                    .ok()
                    .and_then(|th| th.task.map(|k| status_word(th, &k)))
                    .unwrap_or("completed");
                let truncated = body.len() > remaining.saturating_sub(3);
                let text = if remaining < 3 {
                    String::new()
                } else {
                    cap(body, remaining - 3)
                };
                remaining = remaining.saturating_sub(text.len());
                Ok(zeron_doc::TaskResultPart {
                    task_id: format!("{child}:{}", id.run),
                    chat_id: child,
                    status: status.to_owned(),
                    truncated,
                    text,
                })
            })
            .collect::<Result<Vec<_>, DriverError>>()?;
        engine
            .task_results(&chat, &message_id, results)
            .map_err(|e| DriverError::Corrupt(format!("record task results: {e}")))?;
        Ok(Pending {
            message_id,
            via: Via::Resume {
                prompt,
                worktree: None,
            },
            chat,
        })
    }

    fn results_text(
        &self,
        engine: &dyn RunEngine,
        s: &OrchState,
        delivered: &[TaskId],
    ) -> Result<String, DriverError> {
        let mut out = String::from(
            "Task results: delegated tasks you started have finished. Each result follows.\n",
        );
        for id in delivered {
            let chat = self.chat_of(id.child)?;
            let title = self
                .spec_of(&chat)
                .ok()
                .and_then(|sp| sp.title)
                .unwrap_or_else(|| chat.clone());
            let status = s
                .thread(id.child)
                .ok()
                .and_then(|th| th.task.map(|k| status_word(th, &k)))
                .unwrap_or("completed");
            let task_id = format!("{chat}:{}", id.run);
            let body = self.remote_result(engine, &chat).unwrap_or_default();
            if !append_result(&mut out, &title, status, &task_id, &body) {
                break;
            }
        }
        Ok(out)
    }
}

const OMITTED_RESULTS: &str = "\nMore results do not fit here. Read them with task_status using the taskIds from delegate_task.\n";

fn append_result(out: &mut String, title: &str, status: &str, task_id: &str, body: &str) -> bool {
    let title = cap(title.to_owned(), 256);
    let head = format!("\n## {title} ({status})\ntaskId: {task_id}\n");
    let block = format!("{head}\n{body}\n");
    let limit = INLINE_MAX - OMITTED_RESULTS.len();
    if out.len() + block.len() <= limit {
        out.push_str(&block);
        return true;
    }
    let fallback = format!(
        "{head}Read the full result with task_status {{\"taskId\":{}}}.\n",
        serde_json::to_string(task_id).unwrap_or_default()
    );
    if out.len() + fallback.len() <= limit {
        out.push_str(&fallback);
        return true;
    }
    out.push_str(OMITTED_RESULTS);
    false
}

#[cfg(test)]
mod result_budget_tests {
    use super::*;
    #[test]
    fn long_titles_and_many_siblings_share_one_inline_budget() {
        let mut out = String::from("Task results:\n");
        let title = "界".repeat(20_000);
        for n in 0..1_000 {
            if !append_result(
                &mut out,
                &title,
                "completed",
                &format!("child-{n}:1"),
                &"result".repeat(4_000),
            ) {
                break;
            }
        }
        assert!(out.len() <= INLINE_MAX, "{}", out.len());
        assert!(out.ends_with(OMITTED_RESULTS));
        assert!(out.contains("task_status"));
    }
}

impl OrchWire {
    pub fn task_list(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
    ) -> Result<Vec<TaskView>, OrchError> {
        let _g = lock(&self.gate);
        self.admit_agent(engine, origin)?;
        let Some(parent) = self.known_thread(origin)? else {
            return Ok(Vec::new());
        };
        let state = self.driver.state()?;
        let mut views = Vec::new();
        for child in state
            .threads
            .values()
            .filter(|t| t.parent == Some(parent) && t.task.is_some())
        {
            let chat = self.chat_of(child.id)?;
            views.push(self.view_of(engine, origin, &chat, false)?);
        }
        for native in self.native_tasks(origin)? {
            if let Some(id) = native["taskId"].as_str() {
                views.push(self.native_view(engine, origin, id)?);
            }
        }
        for view in &views {
            self.record_tool_reply(origin, "task_status", view)?;
        }
        Ok(views)
    }
    fn native_view(
        &self,
        engine: &dyn RunEngine,
        origin: &str,
        id: &str,
    ) -> Result<TaskView, OrchError> {
        let native = self
            .native_tasks(origin)?
            .into_iter()
            .find(|v| v["taskId"] == id)
            .ok_or_else(|| err("task_not_found", "No such native task for this chat."))?;
        let status = match native["status"].as_str() {
            Some("completed") => "completed",
            Some("failed") => "failed",
            _ => "running",
        };
        Ok(TaskView {
            origin: "native",
            background: native["background"].as_bool(),
            task_id: id.to_owned(),
            child_thread_id: id.to_owned(),
            status,
            summary: if status == "running" {
                None
            } else {
                engine.result_text(id)
            },
            has_pending_question: false,
            question: None,
            wait_timed_out: false,
            next: None,
        })
    }
}

impl OrchWire {
    fn record_tool_reply(
        &self,
        parent: &str,
        tool: &str,
        view: &TaskView,
    ) -> Result<(), DriverError> {
        if !self.driver.mechanism_capture_enabled() {
            return Ok(());
        }
        let Some(th) = self
            .thread_state(parent)
            .filter(|th| th.is_live() && !th.attempt.emitted)
        else {
            return Ok(());
        };
        let Some((_, task_run)) = split_task(&view.task_id) else {
            return Ok(());
        };
        let at = th.attempt_id();
        let calls: Vec<_> = lock(&self.calls)
            .iter()
            .filter(|((chat, _), (attempt, name, input))| {
                if chat != parent || *attempt != at || !name.ends_with(tool) {
                    return false;
                }
                if tool == "delegate_task" {
                    let request = input
                        .as_ref()
                        .and_then(|v| v.get("clientRequestId"))
                        .and_then(|v| v.as_str());
                    request.is_some_and(|request| {
                        self.driver
                            .meta(&format!("crid:mcp:{parent}:delegate:{request}"))
                            .ok()
                            .flatten()
                            .as_deref()
                            == Some(view.child_thread_id.as_str())
                    })
                } else {
                    input
                        .as_ref()
                        .and_then(|v| v.get("taskId"))
                        .and_then(|v| v.as_str())
                        .map_or(tool == "task_status", |id| id == view.task_id)
                }
            })
            .map(|((_, id), _)| id.clone())
            .collect();
        if calls.len() != 1 {
            return Ok(());
        }
        let reply_id = &calls[0];
        let data = serde_json::json!({"parent":parent,"parent_run":at.run.ord,"parent_attempt":at.n,"child":view.child_thread_id,"task_run":task_run,"reply_id":reply_id,"tool":tool});
        let key = format!("tool-reply:{parent}:{}:{}:{reply_id}", at.run.ord, at.n);
        let mut replies: Vec<serde_json::Value> = self
            .driver
            .meta(&key)?
            .map(|raw| serde_json::from_str(&raw))
            .transpose()?
            .unwrap_or_default();
        if !replies.contains(&data) {
            replies.push(data.clone());
        }
        self.driver
            .put_meta(&key, &serde_json::to_string(&replies)?)?;
        self.driver.observe_mechanism(
            &format!("{key}:{}:{task_run}", view.child_thread_id),
            "tool_reply",
            data,
        )
    }
    fn record_tool_observed(
        &self,
        parent: &str,
        at: zeron_orch::AttemptId,
        reply_id: &str,
    ) -> Result<(), DriverError> {
        if !self.driver.mechanism_capture_enabled() {
            return Ok(());
        }
        let key = format!("tool-reply:{parent}:{}:{}:{reply_id}", at.run.ord, at.n);
        let Some(raw) = self.driver.meta(&key)? else {
            return Ok(());
        };
        let replies: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
        for data in replies {
            let child = data["child"]
                .as_str()
                .ok_or_else(|| DriverError::Corrupt("missing reply child".into()))?;
            let task_run = data["task_run"]
                .as_u64()
                .ok_or_else(|| DriverError::Corrupt("missing reply task run".into()))?;
            self.driver.observe_mechanism(
                &format!(
                    "tool-observed:{parent}:{}:{}:{reply_id}:{child}:{task_run}",
                    at.run.ord, at.n
                ),
                "tool_observed",
                data,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tool_fact_tests {
    use super::*;
    use std::sync::Arc;
    use crate::orchestrator::{Driver, OrchOptions};
    use zeron_orch::{Config, RunEvent};
    use zeron_sync::DocsStore;
    #[test]
    fn tool_observed_requires_real_reply_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        drop(Driver::open_with_capture(store.clone(), "device", Config::default(), true).unwrap());
        let wire = OrchWire::open(store.clone(), "device", OrchOptions::default()).unwrap();
        let t = wire.thread("parent").unwrap();
        wire.driver
            .apply("send", Event::Run(RunEvent::ServerEnqueue { t, msg: 1 }))
            .unwrap();
        wire.driver
            .apply("start", Event::Run(RunEvent::StartFromQueue { t }))
            .unwrap();
        let at = wire.thread_state("parent").unwrap().attempt_id();
        let view = TaskView {
            origin: "app",
            background: None,
            task_id: "child:3".into(),
            child_thread_id: "child".into(),
            status: "completed",
            summary: Some("done".into()),
            has_pending_question: false,
            question: None,
            wait_timed_out: false,
            next: None,
        };
        wire.record_tool_reply("parent", "task_status", &view)
            .unwrap();
        assert!(
            wire.driver
                .meta(&format!(
                    "tool-reply:parent:{}:{}:real-call",
                    at.run.ord, at.n
                ))
                .unwrap()
                .is_none()
        );
        wire.tool_event(
            "parent",
            &AgentEvent::ToolCall {
                id: "real-call".into(),
                call: ToolCall::Mcp {
                    server: "zeron".into(),
                    tool: "task_status".into(),
                    input: Some(serde_json::json!({"taskId":"child:3"})),
                },
            },
        );
        wire.record_tool_reply("parent", "task_status", &view)
            .unwrap();
        drop(wire);
        let wire = OrchWire::open(store, "device", OrchOptions::default()).unwrap();
        for id in ["unknown-call", "real-call", "real-call"] {
            wire.tool_event(
                "parent",
                &AgentEvent::ToolResult {
                    id: id.into(),
                    is_error: false,
                    output: None,
                    diff: None,
                    exit_code: None,
                },
            );
        }
        let rows = wire.driver.mechanism_lines().unwrap();
        let row: serde_json::Value = serde_json::from_str(rows.last().unwrap()).unwrap();
        let facts = row["snapshot"]["facts"].as_array().unwrap();
        for kind in ["tool_reply", "tool_observed"] {
            let selected: Vec<_> = facts.iter().filter(|f| f["kind"] == kind).collect();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0]["data"]["reply_id"], "real-call");
            assert_eq!(selected[0]["data"]["parent"], "parent");
            assert_eq!(selected[0]["data"]["child"], "child");
            assert_eq!(selected[0]["data"]["task_run"], 3);
        }
    }
}
