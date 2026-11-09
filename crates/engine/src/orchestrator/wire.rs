use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeron_orch::{
    AttemptId, Config, DelEvent, Effect, Event, Hold, Mode, Msg, Node, OrchState, Outcome, Reason,
    RunEvent, RunId, RunKind, Start, Thread, ThreadId, step,
};
use zeron_proto::{DoneStatus, FailureClass, HarnessId, RunRequest};
use zeron_sync::DocsStore;

#[path = "wire/questions.rs"]
mod question_wire;
#[path = "remote.rs"]
pub mod remote;
mod tasks;

pub use super::runtime_binding::AgentRuntime;
pub use tasks::{ChildSpec, DelegateRequest, OrchError, RUNNING_NEXT, TaskView, access_level};

use super::{CommandDisposition, CommandReceipt, CommandRefusal, CommandSource, DriverError};
use super::driver::{ApplyResult, Driver};
use super::store::{Applied, OutboxRow};
use crate::EngineError;
use crate::sessions::SteerOutcome;

pub const FLAG_ENV: &str = "ZERON_ORCH_V2";
pub const RESUME_PROMPT: &str = "The usage limit has reset. Continue where you left off.";

pub fn flag_from_env() -> bool {
    std::env::var(FLAG_ENV).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on"))
}

#[derive(Debug, Clone)]
pub struct OrchOptions {
    pub config: Config,
    pub limit_delays: Vec<Duration>,
    pub retry_delays: Vec<Duration>,
    pub reconcile_every: Duration,
}

impl Default for OrchOptions {
    fn default() -> Self {
        Self {
            config: Config::default(),
            limit_delays: vec![Duration::from_secs(15 * 60), Duration::from_secs(60 * 60)],
            retry_delays: vec![
                Duration::from_millis(250),
                Duration::from_secs(1),
                Duration::from_secs(3),
            ],
            reconcile_every: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Via {
    Dispatch {
        harness: HarnessId,
        request: Box<RunRequest>,
    },
    Steer {
        prompt: String,
        attachments: Vec<String>,
        issued_at: i64,
    },
    Resume {
        prompt: String,
        #[serde(default)]
        worktree: Option<zeron_proto::WorktreeSpec>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub chat: String,
    pub message_id: String,
    pub via: Via,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    Started(String),
    Steered(SteerOutcome),
    Bypass,
    Nothing,
}

#[async_trait::async_trait]
pub trait RunEngine: Send + Sync {
    async fn start(&self, p: &Pending) -> Result<String, EngineError>;
    async fn steer(&self, p: &Pending) -> Result<Ran, EngineError>;
    async fn stop_attempt(&self, chat: &str) -> bool;
    async fn interrupt_run(&self, chat: &str) -> bool;
    fn owns_run(&self, chat: &str) -> bool;
    fn agent_attempt(&self, _chat: &str) -> Option<AgentRuntime> {
        None
    }
    fn last_done(&self, chat: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)>;
    fn journal_seq(&self, chat: &str) -> u64;
    fn effective_access(&self, _chat: &str) -> Option<u8> {
        None
    }
    fn create_child(&self, parent: &str, spec: &ChildSpec) -> Result<(), EngineError>;
    fn result_text(&self, chat: &str) -> Option<String>;
    fn reports_mcp_results(&self, chat: &str) -> bool;
    fn task_results(
        &self,
        chat: &str,
        message_id: &str,
        results: Vec<zeron_doc::TaskResultPart>,
    ) -> Result<(), EngineError>;
    fn kick_drain(&self, chat: &str);
    fn respond_question(
        &self,
        _chat: &str,
        _request: &str,
        _answers: Vec<zeron_proto::UserInputAnswer>,
    ) -> Result<bool, EngineError> {
        Ok(false)
    }
    fn notify_question(&self, _chat: &str, _request: &str) {}
    fn native_snapshot(&self, _doc: &str) -> Result<Vec<u8>, EngineError> {
        Err(EngineError::Other("native snapshot reader unavailable".into()))
    }
    fn save_native_snapshot(&self, _doc: &str, _snapshot: &[u8]) -> Result<(), EngineError> {
        Err(EngineError::Other(
            "native snapshot writer unavailable".into(),
        ))
    }
    fn native_changed_task(&self, chat: &str, _key: u64, tool: &str, status: &str) -> Result<(),EngineError> {
        self.native_changed(chat, tool, status)
    }
    fn native_changed(&self, _chat: &str, _tool: &str, _status: &str) -> Result<(), EngineError> {
        Ok(())
    }

    fn orch_route(&self, _chat: &str) -> Option<super::placement::ChatRoute> {
        None
    }
    fn orch_supports(&self, _device: &str) -> bool {
        false
    }
    fn orch_send(&self, _message: &zeron_doc::OrchMessage) -> Result<bool, String> {
        Err("remote orchestration unavailable".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    Person,
    Redeliver,
    SendNow,
}

enum Decision {
    Refused(CommandRefusal, &'static str),
    Bypass,
    Effects(Vec<String>),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn accepted_ids(r: &ApplyResult) -> Vec<String> {
    match (&r.applied, r.duplicate) {
        (Applied::Accepted { effects, .. }, false) => effects.clone(),
        _ => Vec::new(),
    }
}

pub fn outcome_of(status: DoneStatus, failure: Option<FailureClass>) -> Outcome {
    match (status, failure) {
        (DoneStatus::Completed, _) => Outcome::Completed,
        (DoneStatus::Interrupted, _) => Outcome::Interrupted,
        (DoneStatus::Errored, Some(FailureClass::UsageLimit { .. })) => Outcome::UsageLimit,
        (DoneStatus::Errored, Some(FailureClass::TransportError | FailureClass::ProviderError)) => {
            Outcome::Transient
        }
        (DoneStatus::Errored, _) => Outcome::Failed,
    }
}

fn in_flight(th: &Thread, n: u64) -> bool {
    let queued = th.queue.iter().any(|m| *m == Msg::Person(n));
    let running = th.is_live()
        && th
            .runs
            .last()
            .is_some_and(|r| r.start == Start::Queued(Msg::Person(n)));
    queued || running
}

fn auto_step(s: &OrchState, th: &Thread, chat: &str) -> Option<(String, Event)> {
    let t = th.id;
    if let Some(ev) = th.inbox.iter().next().copied() {
        let tag = format!("{chat}:{}:{}", ev.run, ev.attempt);
        let (key, e) = if ev.node == Node::Child {
            (format!("child:{tag}"), RunEvent::IngestChild { t, ev })
        } else if !th.is_current(&ev) {
            (
                format!("stale:{tag}:{:?}", ev.outcome),
                RunEvent::IngestStale { t, ev },
            )
        } else if ev.outcome.retryable() && th.may_retry(s.config.max_retry) {
            (format!("retry:{tag}"), RunEvent::RetryAttempt { t, ev })
        } else if ev.outcome == Outcome::Completed && th.wait_on.is_some() {
            let key = format!("abandon:{tag}");
            return Some((key, Event::Del(DelEvent::WaitAbandon { t })));
        } else {
            (format!("end:{tag}"), RunEvent::IngestEnd { t, ev })
        };
        return Some((key, Event::Run(e)));
    }
    if th.is_live() {
        return None;
    }
    if th.ul.armed == Some(th.rord()) && th.ul.due {
        let key = format!("ulr:{chat}:{}", th.rord());
        return Some((key, Event::Run(RunEvent::UsageLimitRecovery { t })));
    }
    if let Some(ev) = super::questions::next(s, t) {
        return Some((format!("question-start:{chat}:{}", th.rord() + 1), ev));
    }
    if matches!(th.queue.front(),Some(Msg::Wake{..})) || (th.held==Hold::Fail && th.queue.iter().any(|m|matches!(m,Msg::Wake{..}))) {
        let pass = matches!(th.held, Hold::None | Hold::Fail);
        let key = format!("wakestart:{chat}:{}", th.rord() + 1);
        return pass.then_some((key, Event::Del(DelEvent::StartFromQueueWake { t })));
    }
    let head_ok = th.queue.front().is_some();
    (th.held == Hold::None && head_ok).then(|| {
        let key = format!("start:{chat}:{}", th.rord() + 1);
        (key, Event::Run(RunEvent::StartFromQueue { t }))
    })
}

fn enabled(ev: Event) -> impl Fn(&OrchState) -> bool {
    move |s| step(s, ev).is_ok()
}

type PendingCall = (AttemptId, String, Option<serde_json::Value>);

pub struct OrchWire {
    me: std::sync::Weak<OrchWire>,
    driver: Driver,
    engine_instance_id: String,
    runtime_source_seq: Mutex<u64>,
    runtime_connections: Mutex<HashMap<String, (bool, u64, u64)>>,
    opts: OrchOptions,
    gate: Mutex<()>,
    engine: Mutex<Option<Arc<dyn RunEngine>>>,
    expect: Mutex<HashMap<String, AttemptId>>,
    claimed: Mutex<HashSet<String>>,
    claimed_msgs: Mutex<HashSet<String>>,
    redeliver: Mutex<HashSet<(String, String)>>,
    quiet: Mutex<HashMap<String, u32>>,
    suspect: Mutex<HashMap<String, AttemptId>>,
    boot_live: HashSet<AttemptId>,
    lose: Mutex<HashSet<String>>,
    dead: std::sync::atomic::AtomicBool,
    recovering: std::sync::atomic::AtomicBool,
    wake: tokio::sync::Notify,
    task_events: tokio::sync::Notify,
    calls: Mutex<HashMap<(String, String), PendingCall>>,
}

impl OrchWire {
    pub fn open(
        store: Arc<DocsStore>,
        device: &str,
        opts: OrchOptions,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Driver::open(store, device, opts.config)?;
        let boot_live = live_attempts(&driver.state()?);
        let wire = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            driver,
            engine_instance_id: crate::new_id(),
            runtime_source_seq: Mutex::new(0),
            runtime_connections: Mutex::new(HashMap::new()),
            opts,
            gate: Mutex::new(()),
            engine: Mutex::new(None),
            expect: Mutex::new(HashMap::new()),
            claimed: Mutex::new(HashSet::new()),
            claimed_msgs: Mutex::new(HashSet::new()),
            redeliver: Mutex::new(HashSet::new()),
            quiet: Mutex::new(HashMap::new()),
            suspect: Mutex::new(HashMap::new()),
            boot_live,
            lose: Mutex::new(HashSet::new()),
            dead: std::sync::atomic::AtomicBool::new(false),
            recovering: std::sync::atomic::AtomicBool::new(false),
            wake: tokio::sync::Notify::new(),
            task_events: tokio::sync::Notify::new(),
            calls: Mutex::new(HashMap::new()),
        });
        wire.crash_waits()?;
        Ok(wire)
    }

    pub fn driver(&self) -> &Driver {
        &self.driver
    }

    fn observe_runtime(&self, kind: &str, state: &str) -> Result<(), DriverError> {
        if !self.driver.mechanism_capture_enabled() {
            return Ok(());
        }
        self.observe_runtime_data(kind, serde_json::json!({"state":state}))
    }

    fn observe_runtime_data(&self, kind: &str, mut data: serde_json::Value) -> Result<(), DriverError> {
        if !self.driver.mechanism_capture_enabled() {
            return Ok(());
        }
        let mut source_seq = lock(&self.runtime_source_seq);
        *source_seq += 1;
        let id = format!("runtime:{}:{}", self.engine_instance_id, *source_seq);
        data["device"] = serde_json::json!(self.driver.device());
        data["engine_instance_id"] = serde_json::json!(self.engine_instance_id);
        data["generation"] = serde_json::json!(self.engine_instance_id);
        data["origin_id"] = serde_json::json!(id);
        data["source_seq"] = serde_json::json!(*source_seq);
        self.driver.observe_mechanism(&id, kind, data)
    }

    pub(crate) fn engine_started(&self) -> Result<(), DriverError> {
        self.observe_runtime("engine_lifecycle", "started")
    }

    pub(crate) fn engine_stopping(&self) -> Result<(), DriverError> {
        self.observe_runtime("engine_lifecycle", "stopping")
    }

    pub(crate) fn engine_stopped(&self) -> Result<(), DriverError> {
        self.observe_runtime("engine_lifecycle", "stopped")
    }

    pub(crate) fn admission_open(&self) -> Result<(), DriverError> {
        self.observe_runtime("admission_state", "open")
    }

    pub(crate) fn admission_quiesced(&self) -> Result<(), DriverError> {
        self.observe_runtime("admission_state", "quiesced")
    }

    pub(crate) fn admission_held(&self) -> Result<(), DriverError> {
        self.observe_runtime("admission_state", "held")
    }

    pub(crate) fn edge_disconnected(&self) -> Result<(), DriverError> {
        self.observe_runtime_data("transport_connection", serde_json::json!({
            "connection_id":format!("{}:edge", self.engine_instance_id),
            "scope":"edge","source":"disconnect_edge","state":"disconnected"
        }))
    }

    pub(crate) fn observe_room_transport(&self, chat: &str, stats: &zeron_sync::ChatStatsSnapshot) -> Result<(), DriverError> {
        if !self.driver.mechanism_capture_enabled() {
            return Ok(());
        }
        let value = (stats.connected, stats.rejoins, stats.disconnects);
        let mut seen = lock(&self.runtime_connections);
        if seen.get(chat) == Some(&value) {
            return Ok(());
        }
        self.observe_runtime_data("transport_connection", serde_json::json!({
            "connection_id":format!("{}:chat-room:{chat}", self.engine_instance_id),
            "scope":"chat_room","chat":chat,"source":"room_stats",
            "state":if stats.connected {"connected"} else {"disconnected"},
            "connected":stats.connected,"rejoins":stats.rejoins,"disconnects":stats.disconnects
        }))?;
        seen.insert(chat.to_string(), value);
        Ok(())
    }

    pub fn trace_lines(&self) -> Result<Vec<String>, DriverError> {
        self.driver.trace_lines()
    }

    pub fn recovery_pending(&self) -> bool {
        self.recovering.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn attach(self: &Arc<Self>, engine: Arc<dyn RunEngine>) {
        *lock(&self.engine) = Some(engine);
        let wire = self.clone();
        spawn_task("worker", async move { wire.worker().await });
    }

    pub(crate) fn attach_recovering(self: &Arc<Self>, engine: Arc<dyn RunEngine>) {
        self.recovering.store(true, std::sync::atomic::Ordering::Release);
        self.attach(engine);
    }

    pub fn detach(&self) {
        lock(&self.engine).take();
        self.wake.notify_one();
    }

    fn engine(&self) -> Option<Arc<dyn RunEngine>> {
        lock(&self.engine).clone()
    }

    async fn worker(self: Arc<Self>) {
        let mut first = self.recovering.load(std::sync::atomic::Ordering::Acquire);
        loop {
            if self.recovering.load(std::sync::atomic::Ordering::Acquire) {
                if self.engine().is_none() || self.is_dead() { return; }
                self.wake.notified().await;
                continue;
            }
            let tick = if std::mem::take(&mut first) { false } else { tokio::select! {
                () = self.wake.notified() => false,
                () = tokio::time::sleep(self.opts.reconcile_every) => true,
            }};
            let Some(engine) = self.engine() else { return };
            if self.is_dead() {
                return;
            }
            if tick && let Err(err) = self.check_stuck(engine.as_ref()) {
                self.fail(err);
            }
            self.run_effects(engine.as_ref(), None).await;
        }
    }

    pub fn lose_next_done(&self, chat: &str) {
        lock(&self.lose).insert(chat.to_owned());
    }

    fn check_stuck(&self, engine: &dyn RunEngine) -> Result<(), DriverError> {
        let s = self.driver.state()?;
        let pending = self.driver.pending()?;
        let mut seen = HashMap::new();
        for th in s.threads.values().filter(|th| th.is_live()) {
            let chat = self.chat_of(th.id)?;
            if !self.may_be_stuck(engine, th, &chat, &pending) {
                continue;
            }
            let at = th.attempt_id();
            if lock(&self.suspect).get(&chat) == Some(&at) {
                self.end_stuck(engine, &chat, at)?;
            } else {
                seen.insert(chat, at);
            }
        }
        *lock(&self.suspect) = seen;
        Ok(())
    }

    fn may_be_stuck(
        &self,
        engine: &dyn RunEngine,
        th: &Thread,
        chat: &str,
        pending: &[OutboxRow],
    ) -> bool {
        let starting = pending.iter().any(|r| starts(&r.effect, th.id));
        let quiet = lock(&self.quiet).get(chat).is_some_and(|n| *n > 0);
        !th.attempt.emitted && !starting && !quiet && !engine.owns_run(chat)
    }

    fn end_stuck(
        &self,
        engine: &dyn RunEngine,
        chat: &str,
        at: AttemptId,
    ) -> Result<(), DriverError> {
        let outcome = self.stuck_outcome(engine, chat, at);
        tracing::warn!(
            chat,
            run = at.run.ord,
            attempt = at.n,
            ?outcome,
            "orchestrator run is live but the engine has none; ending it"
        );
        let _g = lock(&self.gate);
        let s = self.driver.state()?;
        let th = s
            .thread(at.run.thread)
            .map_err(|r| DriverError::Corrupt(format!("{r:?}")))?;
        if th.attempt_id() != at || !th.is_live() {
            return Ok(());
        }
        if self.unrequested_stop(th, at, outcome) {
            self.engine_interrupt(chat, at)?;
        }
        let ev = Event::Run(RunEvent::ProviderTerminal {
            t: at.run.thread,
            run: at.run.ord,
            attempt: at.n,
            outcome,
        });
        let key = format!("done:{chat}:{}:{}", at.run.ord, at.n);
        self.driver.apply_if(&key, ev, &enabled(ev))?;
        self.pump(at.run.thread, chat, &mut Vec::new())?;
        if let Some(e) = self.engine() {
            e.kick_drain(chat);
        }
        Ok(())
    }

    fn stuck_outcome(&self, engine: &dyn RunEngine, chat: &str, at: AttemptId) -> Outcome {
        let since = self
            .driver
            .meta(&format!("jseq:{chat}"))
            .ok()
            .flatten()
            .and_then(|v| parse_jseq(&v))
            .filter(|(ord, n, _)| *ord == at.run.ord && *n == at.n)
            .map(|(_, _, seq)| seq);
        match (since, engine.last_done(chat)) {
            (Some(start), Some((seq, status, failure))) if seq > start => {
                outcome_of(status, failure)
            }
            _ => Outcome::Crash,
        }
    }

    fn fail(&self, err: DriverError) -> DriverError {
        if matches!(err, DriverError::Crashed(_)) {
            self.dead.store(true, std::sync::atomic::Ordering::Release);
        }
        tracing::warn!(error = %err, "orchestrator step failed");
        err
    }

    fn alive(&self) -> Result<(), DriverError> {
        if self.is_dead() {
            return Err(DriverError::Corrupt(
                "orchestrator stopped by a fault".into(),
            ));
        }
        Ok(())
    }

    fn known_thread(&self, chat: &str) -> Result<Option<ThreadId>, DriverError> {
        let Some(v) = self.driver.meta(&format!("chat:{chat}"))? else {
            return Ok(None);
        };
        let n = v
            .parse::<u32>()
            .map_err(|_| DriverError::Corrupt(format!("chat id {v:?}")))?;
        Ok(Some(ThreadId(n)))
    }

    fn intern_thread(&self, chat: &str) -> Result<ThreadId, DriverError> {
        let (n, fresh) = self.driver.intern(&format!("chat:{chat}"), "n:thread")?;
        let n = u32::try_from(n).map_err(|_| DriverError::Corrupt("thread id overflow".into()))?;
        if fresh {
            self.driver.put_meta(&format!("thread:{n}"), chat)?;
        }
        let engine = lock(&self.engine).clone();
        if let Some(route) = engine.and_then(|engine| engine.orch_route(chat)) {
            let key = format!("remote-host:{chat}");
            if self.driver.meta(&key)?.is_some_and(|host| host != route.host) {
                return Err(DriverError::Corrupt("registry host changed without handoff".into()));
            }
            self.driver.put_meta(&key, &route.host)?;
            self.driver.observe_mechanism(
                &format!("registry-host:{chat}"),
                "registry_host",
                serde_json::json!({"chat":chat,"host":route.host,"parent":route.parent,"validated":true}),
            )?;
        }
        Ok(ThreadId(n))
    }

    fn thread(&self, chat: &str) -> Result<ThreadId, DriverError> {
        let t = self.intern_thread(chat)?;
        if self.driver.state()?.thread(t).is_ok() {
            return Ok(t);
        }
        let ev = Event::Del(DelEvent::OpenRoot {
            t,
            access: 2,
            reconfigure: false,
        });
        self.driver
            .apply_if(&format!("open:{chat}"), ev, &enabled(ev))?;
        Ok(t)
    }

    fn chat_of(&self, t: ThreadId) -> Result<String, DriverError> {
        self.driver
            .meta(&format!("thread:{}", t.0))?
            .ok_or_else(|| DriverError::Corrupt(format!("no chat for thread {}", t.0)))
    }

    fn msg(&self, message_id: &str) -> Result<u64, DriverError> {
        Ok(self.driver.intern(&format!("msg:{message_id}"), "n:msg")?.0)
    }

    fn stash(&self, n: u64, p: &Pending) -> Result<(), DriverError> {
        self.driver
            .put_meta(&format!("req:{n}"), &serde_json::to_string(p)?)?;
        if matches!(p.via, Via::Dispatch { .. }) {
            self.driver
                .put_meta(&format!("last:{}", p.chat), &n.to_string())?;
        }
        Ok(())
    }

    fn pending_of(&self, n: u64) -> Result<Pending, DriverError> {
        let json = self
            .driver
            .meta(&format!("req:{n}"))?
            .ok_or_else(|| DriverError::Corrupt(format!("no request for message {n}")))?;
        Ok(serde_json::from_str(&json)?)
    }

    fn ran(&self, message_id: &str) -> Result<bool, DriverError> {
        Ok(self.driver.meta(&format!("ran:{message_id}"))?.is_some())
    }

    fn mark_ran(&self, message_id: &str) {
        if let Err(err) = self.driver.put_meta(&format!("ran:{message_id}"), "1") {
            tracing::warn!(error = %err, "orchestrator ran marker failed");
        }
        lock(&self.claimed_msgs).remove(message_id);
    }

    pub fn thread_state(&self, chat: &str) -> Option<Thread> {
        let t = self.known_thread(chat).ok()??;
        self.driver.state().ok()?.thread(t).ok().cloned()
    }

    pub fn holds_queue(&self, chat: &str, rows: &[String]) -> bool {
        let Ok(Some(t)) = self.known_thread(chat) else {
            return false;
        };
        let Ok(s) = self.driver.state() else {
            return false;
        };
        let Ok(th) = s.thread(t) else {
            return false;
        };
        if th.is_live() {
            return true;
        }
        if th.held == Hold::None {
            return false;
        }
        let key = format!("hold:{chat}:{}:{:?}:{}", th.rord(), th.held, s.stops);
        let before = self.rows_at_hold(&key, rows);
        rows.iter().all(|r| before.contains(r))
    }

    fn rows_at_hold(&self, key: &str, rows: &[String]) -> Vec<String> {
        if let Some(seen) = self
            .driver
            .meta(key)
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        {
            return seen;
        }
        let snap = serde_json::to_string(rows).unwrap_or_else(|_| "[]".into());
        if let Err(err) = self.driver.put_meta(key, &snap) {
            tracing::warn!(error = %err, "orchestrator hold snapshot failed");
        }
        rows.to_vec()
    }

    pub fn allow_redelivery(&self, chat: &str, message_id: &str) {
        lock(&self.redeliver).insert((chat.to_owned(), message_id.to_owned()));
    }

    fn take_redelivery(&self, chat: &str, message_id: &str) -> bool {
        lock(&self.redeliver).remove(&(chat.to_owned(), message_id.to_owned()))
    }

    fn pump(&self, t: ThreadId, chat: &str, fx: &mut Vec<String>) -> Result<(), DriverError> {
        if self.remote_shadow(chat)? {
            return self.pump_shadow(chat, t, fx);
        }
        for _ in 0..32 {
            let s = self.driver.state()?;
            let Ok(th) = s.thread(t) else { return Ok(()) };
            let Some((key, ev)) = auto_step(&s, th, chat) else {
                return Ok(());
            };
            match self.driver.apply_if(&key, ev, &enabled(ev))? {
                Some(r) if !r.duplicate => fx.extend(accepted_ids(&r)),
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn decide(
        &self,
        p: &Pending,
        kind: SendKind,
        steerable: bool,
    ) -> Result<Decision, DriverError> {
        self.alive()?;
        let _g = lock(&self.gate);
        let cmd = take_command();
        let out = self.decide_send(p, kind, steerable, cmd.as_deref());
        let (disposition, effect_ids, refusal) = match &out {
            Ok(Decision::Effects(ids)) => (CommandDisposition::Effects, ids.clone(), None),
            Ok(Decision::Refused(reason, _)) => (CommandDisposition::Refused, Vec::new(), Some(*reason)),
            Ok(Decision::Bypass) => (CommandDisposition::Bypass, Vec::new(), None),
            Err(_) => (CommandDisposition::Error, Vec::new(), None),
        };
        self.note_command(cmd.as_deref(), CommandReceipt {
            source:CommandSource::WireSend, disposition, message_id:Some(p.message_id.clone()),
            effect_ids, refusal,
        })?;
        out
    }

    fn note_command(&self, cmd: Option<&str>, receipt: CommandReceipt) -> Result<(), DriverError> {
        if let Some(id) = cmd {
            self.driver.record_command_decision(id, receipt)?;
        }
        Ok(())
    }

    pub fn command_receipt(&self, id: &str) -> bool {
        let key = command_key(id);
        self.driver.meta(&key).ok().flatten().is_some()
            || self.driver.has_receipt(&key).unwrap_or(false)
    }

    pub fn finish_command(&self, id: &str) -> Result<(), DriverError> {
        self.alive()?;
        self.driver.mark_command(id).map_err(|e| self.fail(e))
    }

    fn decide_send(
        &self,
        p: &Pending,
        kind: SendKind,
        steerable: bool,
        cmd: Option<&str>,
    ) -> Result<Decision, DriverError> {
        let t = self.thread(&p.chat)?;
        if let Via::Dispatch { request, .. } = &p.via {
            let access = tasks::access_level(request.access);
            let ev = Event::Del(DelEvent::NarrowAccess { t, access });
            let key = format!("access:{}:{}", p.chat, access);
            if let Some(result) = self.driver.apply_if(&key, ev, &enabled(ev))?
                && matches!(result.applied, Applied::Accepted { .. })
            {
                self.record_local_ceiling(&p.chat, access, &key)?;
            }
        }
        let n = self.msg(&p.message_id)?;
        if kind != SendKind::Redeliver && self.ran(&p.message_id)? {
            return Ok(Decision::Refused(CommandRefusal::AlreadyRan, "R17: this message id already ran"));
        }
        let s = self.driver.state()?;
        let th = s
            .thread(t)
            .map_err(|r| DriverError::Corrupt(format!("{r:?}")))?;
        if kind == SendKind::Redeliver && in_flight(th, n) {
            return Ok(Decision::Bypass);
        }
        if in_flight(th, n) || lock(&self.claimed_msgs).contains(&p.message_id) {
            return Ok(Decision::Refused(
                CommandRefusal::InFlight, "R17: this message id is already in flight",
            ));
        }
        self.stash(n, p)?;
        let (key, ev) = self.send_event(th, p, n, kind, steerable)?;
        let key = cmd.map_or(key, command_key);
        let r = self.driver.apply_command(&key, cmd, ev)?;
        let mut fx = accepted_ids(&r);
        if let Applied::Rejected { reason, .. } = r.applied {
            tracing::info!(chat = %p.chat, ?reason, "orchestrator refused the send");
            return Ok(Decision::Refused(CommandRefusal::ReducerRejected, "orchestrator refused the send"));
        }
        lock(&self.claimed_msgs).insert(p.message_id.clone());
        self.pump(t, &p.chat, &mut fx)?;
        Ok(Decision::Effects(fx))
    }

    fn send_event(
        &self,
        th: &Thread,
        p: &Pending,
        n: u64,
        kind: SendKind,
        steerable: bool,
    ) -> Result<(String, Event), DriverError> {
        let t = th.id;
        let m = &p.message_id;
        let busy = th.int_req || th.attempt.emitted;
        let now = kind == SendKind::SendNow;
        if !th.is_live() || busy || !(now || steerable) {
            let k = self.driver.bump(&format!("sends:{m}"))?;
            let ev = RunEvent::PersonSend {
                t,
                mode: Mode::Queue,
                msg: n,
            };
            return Ok((format!("send:{m}:{k}"), Event::Run(ev)));
        }
        if now {
            let ev = RunEvent::RestartActive {
                t,
                mode: Mode::Restart,
                msg: n,
            };
            return Ok((format!("restart:{m}"), Event::Run(ev)));
        }
        Ok((
            format!("steer:{m}"),
            Event::Run(RunEvent::Steer { t, msg: n }),
        ))
    }

    pub async fn send(
        &self,
        engine: &dyn RunEngine,
        mut p: Pending,
        kind: SendKind,
        steerable: bool,
    ) -> Result<Ran, EngineError> {
        let kind = if self.take_redelivery(&p.chat, &p.message_id) {
            SendKind::Redeliver
        } else {
            kind
        };
        if p.message_id.is_empty() {
            p.message_id = crate::new_id();
        }
        let decision = self
            .decide(&p, kind, steerable)
            .map_err(|e| self.to_engine(e))?;
        self.record_root_ceiling(engine, &p.chat)
            .map_err(|e| self.to_engine(e))?;
        match decision {
            Decision::Refused(_, why) => {
                tracing::info!(chat = %p.chat, message = %p.message_id, why, "dispatch refused");
                Ok(Ran::Nothing)
            }
            Decision::Bypass => Ok(Ran::Bypass),
            Decision::Effects(ids) => {
                let ran = self.run_effects(engine, Some(&ids)).await;
                self.wake.notify_one();
                ran.into_iter()
                    .find(|r| !matches!(r, Ok(Ran::Nothing)))
                    .unwrap_or(Ok(Ran::Nothing))
            }
        }
    }

    fn to_engine(&self, err: DriverError) -> EngineError {
        EngineError::Other(format!("orchestrator: {}", self.fail(err)))
    }

    pub async fn interrupt(
        &self,
        engine: &dyn RunEngine,
        chat: &str,
    ) -> Result<Option<bool>, EngineError> {
        let ids = self
            .request_interrupt(chat)
            .map_err(|e| self.to_engine(e))?;
        if ids.is_empty() {
            return Ok(None);
        }
        let ran = self.run_effects(engine, Some(&ids)).await;
        self.wake.notify_one();
        for r in ran {
            r?;
        }
        Ok(Some(true))
    }

    fn request_interrupt(&self, chat: &str) -> Result<Vec<String>, DriverError> {
        self.alive()?;
        let _g = lock(&self.gate);
        let t = self.thread(chat)?;
        let cmd = take_command();
        let key = match cmd.as_deref() {
            Some(id) => command_key(id),
            None => format!("int:{chat}:{}", self.driver.bump(&format!("ints:{chat}"))?),
        };
        let ev = Event::Del(DelEvent::Stop { t });
        let result = self.driver.apply_command(&key, cmd.as_deref(), ev);
        let (disposition, effect_ids, refusal) = match &result {
            Ok(r) if matches!(r.applied, Applied::Rejected { .. }) => (CommandDisposition::Refused, Vec::new(), Some(CommandRefusal::ReducerRejected)),
            Ok(r) => (CommandDisposition::Effects, accepted_ids(r), None),
            Err(_) => (CommandDisposition::Error, Vec::new(), None),
        };
        self.note_command(cmd.as_deref(), CommandReceipt {
            source:CommandSource::WireInterrupt, disposition, message_id:None, effect_ids, refusal,
        })?;
        let r = result?;
        let mut fx = accepted_ids(&r);
        self.pump(t, chat, &mut fx)?;
        Ok(fx)
    }

    pub fn provider_done(&self, chat: &str, status: DoneStatus, failure: Option<FailureClass>) {
        self.provider_done_context(chat, status, failure, None);
    }

    pub(crate) fn provider_done_at(&self, chat: &str, status: DoneStatus, failure: Option<FailureClass>, at: AttemptId) {
        self.provider_done_context(chat, status, failure, Some(at));
    }

    fn provider_done_context(&self, chat: &str, status: DoneStatus, failure: Option<FailureClass>, source: Option<AttemptId>) {
        if self.is_dead() || lock(&self.lose).remove(chat) {
            return;
        }
        let res = self.done_locked(chat, status, failure, source);
        match res {
            Ok(changed) => {
                self.wake.notify_one();
                if changed && let Some(e) = self.engine() {
                    e.kick_drain(chat);
                }
            }
            Err(err) => {
                self.fail(err);
            }
        }
    }

    fn note_reset(
        &self,
        chat: &str,
        status: DoneStatus,
        failure: Option<FailureClass>,
    ) -> Result<(), DriverError> {
        let (DoneStatus::Errored, Some(FailureClass::UsageLimit { reset_at })) = (status, failure)
        else {
            return Ok(());
        };
        if self.known_thread(chat)?.is_none() {
            return Ok(());
        }
        let key = format!("limit_at:{chat}");
        match reset_at {
            Some(at) => self.driver.put_meta(&key, &at.to_string()),
            None => self.driver.delete_meta(&key),
        }
    }

    fn done_locked(&self, chat: &str, status: DoneStatus, failure: Option<FailureClass>, source: Option<AttemptId>) -> Result<bool, DriverError> {
        let outcome = outcome_of(status, failure);
        let _g = lock(&self.gate);
        let Some(t) = self.known_thread(chat)? else {
            return Ok(false);
        };
        let s = self.driver.state()?;
        let Ok(th) = s.thread(t) else {
            return Ok(false);
        };
        if !th.is_live() {
            return Ok(false);
        }
        let quiet = lock(&self.quiet).get(chat).is_some_and(|n| *n > 0);
        if outcome == Outcome::Interrupted && quiet && !th.int_req {
            return Ok(false);
        }
        let held = th.held;
        let at = source.unwrap_or_else(|| lock(&self.expect)
            .get(chat)
            .copied()
            .unwrap_or(th.attempt_id()));
        if at == th.attempt_id() { self.note_reset(chat, status, failure)?; }
        let ev = Event::Run(RunEvent::ProviderTerminal {
            t,
            run: at.run.ord,
            attempt: at.n,
            outcome,
        });
        let key = format!("done:{chat}:{}:{}", at.run.ord, at.n);
        let applied = self.driver.apply_if(&key, ev, &enabled(ev))?;
        if applied.is_none() && self.unrequested_stop(th, at, outcome) {
            self.engine_interrupt(chat, at)?;
            self.driver.apply_if(&key, ev, &enabled(ev))?;
        }
        let mut fx = Vec::new();
        self.pump(t, chat, &mut fx)?;
        let after = self.driver.state()?;
        Ok(after
            .thread(t)
            .is_ok_and(|th| th.held != held || !th.is_live()))
    }

    fn unrequested_stop(&self, th: &Thread, at: AttemptId, outcome: Outcome) -> bool {
        outcome == Outcome::Interrupted
            && at == th.attempt_id()
            && !th.int_req
            && !th.attempt.emitted
    }

    fn engine_interrupt(&self, chat: &str, at: AttemptId) -> Result<(), DriverError> {
        let key = format!("int:{chat}:{}:{}:engine", at.run.ord, at.n);
        self.driver.record_engine_interrupt(&key, at)
    }

    pub(crate) fn recover_start(&self, engine: &dyn RunEngine, mut pending: Pending) -> Result<String, EngineError> {
        let _g = lock(&self.gate);
        let saved = self.driver.meta(&format!("jseq:{}", pending.chat)).map_err(|e| self.to_engine(e))?
            .and_then(|value| parse_jseq(&value));
        let Some((run, attempt, _)) = saved else { return Ok(String::new()) };
        let input = self.driver.meta(&format!("start-input:{}", pending.chat)).map_err(|e| self.to_engine(e))?
            .and_then(|value| serde_json::from_str::<(u32, u32, String)>(&value).ok());
        if input != Some((run, attempt, pending.message_id.clone())) { return Ok(String::new()) }
        let at = {
            let Some(t) = self.known_thread(&pending.chat).map_err(|e| self.to_engine(e))? else { return Ok(String::new()) };
            let at = AttemptId { run: RunId { thread: t, ord: run }, n: attempt };
            let state = self.driver.state().map_err(|e| self.to_engine(e))?;
            let Ok(thread) = state.thread(t) else { return Ok(String::new()) };
            if !thread.is_live() || thread.int_req || thread.attempt.emitted || thread.attempt_id() != at || !self.boot_live.contains(&at)
                || engine.owns_run(&pending.chat)
            { return Ok(String::new()) }
            tasks::clamp_access(&mut pending, thread.access);
            at
        };
        let retry_input = serde_json::to_string(&pending).map_err(|error| EngineError::Other(error.to_string()))?;
        self.driver.put_meta(&format!("retry-input:{}:{}:{}", pending.chat, run, attempt.saturating_add(1)), &retry_input)
            .map_err(|error| self.to_engine(error))?;
        self.terminal_unstarted_locked(&pending.chat, at, Outcome::Crash).map_err(|error| self.to_engine(error))?;
        self.wake.notify_one();
        Ok(String::new())
    }

    pub(crate) fn with_runtime_admission<T>(&self, at: AttemptId, access: u8, f: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
        let _g = lock(&self.gate);
        let state = self.driver.state().map_err(|e| self.to_engine(e))?;
        let admitted = state.thread(at.run.thread).is_ok_and(|thread|
            thread.is_live() && !thread.int_req && !thread.attempt.emitted
                && thread.attempt_id() == at && access <= thread.access);
        if !admitted { return Err(EngineError::Other("orchestrator attempt no longer admits a turn".into())) }
        f()
    }

    pub(crate) fn runtime_attempt_current(&self, at: AttemptId) -> bool {
        self.driver.state().is_ok_and(|state| state.thread(at.run.thread).is_ok_and(|thread|
            thread.is_live() && !thread.int_req && thread.attempt_id() == at))
    }

    fn claim_pending(&self, effect_id: &str) -> Result<bool, DriverError> {
        if !lock(&self.claimed).insert(effect_id.to_owned()) {
            return Ok(false);
        }
        match self.driver.effect_pending(effect_id) {
            Ok(true) => Ok(true),
            result => {
                lock(&self.claimed).remove(effect_id);
                result.map(|_| false)
            }
        }
    }

    pub async fn run_effects(
        &self,
        engine: &dyn RunEngine,
        only: Option<&[String]>,
    ) -> Vec<Result<Ran, EngineError>> {
        if self.recovering.load(std::sync::atomic::Ordering::Acquire) {
            return vec![Err(EngineError::Other("orchestrator recovery is pending or blocked; check the engine log".into()))];
        }
        if let Err(err) = self.retry_native_terminals(engine) {
            tracing::warn!(error = %err, "native terminal save remains pending");
        }
        let rows = match self.driver.pending() {
            Ok(rows) => rows,
            Err(err) => return vec![Err(self.to_engine(err))],
        };
        let mut out = Vec::new();
        let mut busy = HashSet::new();
        for row in rows {
            let t = order_key(&row.effect);
            if only.is_some_and(|ids| !ids.contains(&row.effect_id)) {
                busy.extend(t);
                continue;
            }
            if t.is_some_and(|t| busy.contains(&t)) {
                continue;
            }
            let claimed = match self.claim_pending(&row.effect_id) {
                Ok(claimed) => claimed,
                Err(err) => { out.push(Err(self.to_engine(err))); return out; }
            };
            if self.is_dead() || !claimed {
                if claimed { lock(&self.claimed).remove(&row.effect_id); }
                if lock(&self.claimed).contains(&row.effect_id) { busy.extend(t); }
                continue;
            }
            let res = self.execute(engine, &row, false).await;
            if res.is_err()
                && !matches!(
                    row.effect,
                    Effect::StartRun { .. } | Effect::StartAttempt { .. } | Effect::SteerRun { .. }
                )
            {
                busy.extend(t);
                lock(&self.claimed).remove(&row.effect_id);
                out.push(res);
                continue;
            }
            if let Err(err) = self.driver.complete(&row.effect_id) {
                lock(&self.claimed).remove(&row.effect_id);
                out.push(Err(self.to_engine(err)));
                return out;
            }
            lock(&self.claimed).remove(&row.effect_id);
            self.wake.notify_one();
            out.push(res);
        }
        out
    }

    async fn execute(
        &self,
        engine: &dyn RunEngine,
        row: &OutboxRow,
        replay: bool,
    ) -> Result<Ran, EngineError> {
        let wrap = |e: DriverError| EngineError::Other(format!("orchestrator: {e}"));
        if self
            .remote_effect(engine, &row.effect, &row.effect_id)
            .map_err(wrap)?
        {
            return Ok(Ran::Nothing);
        }
        match &row.effect {
            Effect::StartRun {
                run,
                start,
                delivered,
                access,
            } => {
                let at = AttemptId { run: *run, n: 1 };
                if !self.may_start(at).map_err(wrap)? {
                    return Ok(Ran::Nothing);
                }
                self.stash_delivered(*run, delivered).map_err(wrap)?;
                let mut p = self.start_pending(engine, *run, *start).map_err(wrap)?;
                tasks::clamp_access(&mut p, *access);
                self.start(engine, &row.effect_id, at, p, replay).await
            }
            Effect::StartAttempt {
                attempt,
                msg,
                reason,
                access,
            } => {
                if !self.may_start(*attempt).map_err(wrap)? {
                    return Ok(Ran::Nothing);
                }
                self.back_off(*reason, attempt.n).await;
                if !self.may_start(*attempt).map_err(wrap)? {
                    return Ok(Ran::Nothing);
                }
                let mut p = self.attempt_pending(engine, *attempt, *msg).map_err(wrap)?;
                tasks::clamp_access(&mut p, *access);
                self.start(engine, &row.effect_id, *attempt, p, replay)
                    .await
            }
            Effect::SteerRun { msg, attempt } => {
                let p = self.pending_of(*msg).map_err(wrap)?;
                STARTING_ATTEMPT.scope(*attempt, self.steer(engine, p)).await
            }
            Effect::InterruptAttempt { attempt } => {
                let chat = self.chat_of(attempt.run.thread).map_err(wrap)?;
                self.quietly(&chat, engine.stop_attempt(&chat)).await;
                Ok(Ran::Nothing)
            }
            Effect::InterruptRun { run } => {
                let chat = self.chat_of(run.thread).map_err(wrap)?;
                engine.interrupt_run(&chat).await;
                if !engine.owns_run(&chat) { self.reconcile_absent_natives(engine,&chat)?; }
                Ok(Ran::Nothing)
            }
            Effect::ArmTimer { run } => {
                self.arm_timer(*run).map_err(wrap)?;
                Ok(Ran::Nothing)
            }
            Effect::CreateChild { parent, child, .. } => {
                self.create_child(engine, *parent, *child).map_err(wrap)?;
                Ok(Ran::Nothing)
            }
            Effect::RespondInput { child, .. } => {
                let chat = self.chat_of(*child).map_err(wrap)?;
                if let Some((request, answers)) =
                    super::questions::response(&self.driver, &row.effect).map_err(wrap)?
                {
                    if !engine.respond_question(&chat, &request, answers)? {
                        return Err(EngineError::Other(
                            "Input resolver is unavailable; durable answer remains pending".into(),
                        ));
                    }
                }
                Ok(Ran::Nothing)
            }
            Effect::QuestionView { .. } => {
                self.task_events.notify_waiters();
                self.pump_all().map_err(wrap)?;
                self.wake.notify_one();
                Ok(Ran::Nothing)
            }
            Effect::NotifyQuestion { child, qid, epoch } => {
                let s = self.driver.state().map_err(wrap)?;
                if s.questions
                    .get(&(*child, *qid))
                    .is_some_and(|q| q.pending && q.epoch == *epoch && q.at_person)
                {
                    let chat = self.chat_of(*child).map_err(wrap)?;
                    let request = self
                        .driver
                        .meta(&format!("question-id:{qid}"))
                        .map_err(wrap)?
                        .unwrap_or_default();
                    engine.notify_question(&chat, &request);
                }
                Ok(Ran::Nothing)
            }
            Effect::NativeChanged { parent, key } => {
                let state = self.driver.state().map_err(wrap)?;
                if let Some(n) = state.natives.get(&(*parent, *key)) {
                    let chat = self.chat_of(*parent).map_err(wrap)?;
                    let tool = super::native::tool_id(&self.driver, *key).map_err(wrap)?;
                    let status = match n.status {
                        zeron_orch::native::NativeStatus::Running => "running",
                        zeron_orch::native::NativeStatus::Completed => "done",
                        _ => "failed",
                    };
                    engine.native_changed_task(&chat, *key, &tool, status)?;
                }
                self.task_events.notify_waiters();
                Ok(Ran::Nothing)
            }
            Effect::DeliverNative {
                parent,
                key,
                generation,
            } => {
                self.deliver_native(engine, *parent, *key, *generation)
                    .map_err(wrap)?;
                Ok(Ran::Nothing)
            }
            Effect::Send(m) => {
                self.deliver(engine, *m).map_err(wrap)?;
                Ok(Ran::Nothing)
            }
            Effect::CompleteWait { .. } | Effect::Reconfigure { .. } => {
                self.task_events.notify_waiters();
                Ok(Ran::Nothing)
            }
        }
    }

    async fn back_off(&self, reason: Reason, n: u32) {
        if !matches!(reason, Reason::Retry | Reason::ProviderRecovery) {
            return;
        }
        let d = &self.opts.retry_delays;
        let i = usize::try_from(n.saturating_sub(2)).unwrap_or(usize::MAX);
        if let Some(wait) = d.get(i).or(d.last()) {
            tokio::time::sleep(*wait).await;
        }
    }

    async fn quietly<F: std::future::Future>(&self, chat: &str, f: F) -> F::Output {
        *lock(&self.quiet).entry(chat.to_owned()).or_default() += 1;
        let out = f.await;
        if let Some(n) = lock(&self.quiet).get_mut(chat) {
            *n = n.saturating_sub(1);
        }
        out
    }

    fn start_pending(
        &self,
        engine: &dyn RunEngine,
        run: RunId,
        start: Start,
    ) -> Result<Pending, DriverError> {
        match start {
            Start::Queued(Msg::Person(n) | Msg::Server(n)) => self.pending_of(n),
            Start::Resume => self.resume_pending(run),
            Start::Questions => {
                let chat = self.chat_of(run.thread)?;
                let prompt =
                    super::questions::prompt(&self.driver, &self.driver.state()?, run.thread)?;
                Ok(Pending {
                    chat,
                    message_id: format!("orch-question-{}", run.ord),
                    via: Via::Resume {
                        prompt,
                        worktree: None,
                    },
                })
            }
            Start::Queued(Msg::Task) => self.task_pending(run),
            Start::Queued(Msg::Wake { run: r, generation }) => {
                self.wake_pending(engine, run, r, generation)
            }
        }
    }

    fn attempt_pending(
        &self,
        engine: &dyn RunEngine,
        at: AttemptId,
        msg: Option<u64>,
    ) -> Result<Pending, DriverError> {
        if let Some(n) = msg {
            return self.pending_of(n);
        }
        let chat = self.chat_of(at.run.thread)?;
        if let Some(input) = self.driver.meta(&format!("retry-input:{chat}:{}:{}", at.run.ord, at.n))? {
            return Ok(serde_json::from_str(&input)?);
        }
        let s = self.driver.state()?;
        let th = s
            .thread(at.run.thread)
            .map_err(|r| DriverError::Corrupt(format!("{r:?}")))?;
        let start = th
            .run(at.run.ord)
            .map(|r| r.start)
            .ok_or_else(|| DriverError::Corrupt(format!("no run {}", at.run.ord)))?;
        self.start_pending(engine, at.run, start)
    }

    fn resume_pending(&self, run: RunId) -> Result<Pending, DriverError> {
        let chat = self.chat_of(run.thread)?;
        Ok(Pending {
            message_id: format!("orch-resume-{}", run.ord),
            via: Via::Resume {
                prompt: RESUME_PROMPT.to_owned(),
                worktree: None,
            },
            chat,
        })
    }

    async fn start(
        &self,
        engine: &dyn RunEngine,
        effect_id: &str,
        at: AttemptId,
        p: Pending,
        _replay: bool,
    ) -> Result<Ran, EngineError> {
        let marker = format!("fx:{effect_id}");
        if let Some(route) = engine.orch_route(&p.chat) {
            let key = format!("remote-host:{}", p.chat);
            if self.driver.meta(&key).map_err(|error| self.to_engine(error))?.is_some_and(|host| host != route.host) {
                return Err(EngineError::Other("registry host changed without handoff".into()));
            }
            self.driver.put_meta(&key, &route.host).map_err(|error| self.to_engine(error))?;
            self.driver.observe_mechanism(
                &format!("registry-host:{}", p.chat), "registry_host",
                serde_json::json!({"chat":p.chat,"host":route.host,"parent":route.parent,"validated":true}),
            ).map_err(|error| self.to_engine(error))?;
        }
        let access_ceiling = self.driver.state().ok().and_then(|state| {
            state.thread(at.run.thread).ok().map(|thread| thread.access)
        });
        let dispatch_kind = match &p.via {
            Via::Dispatch { .. } => "dispatch",
            Via::Steer { .. } => "steer",
            Via::Resume { .. } => "resume",
        };
        let observe = |outcome: &str, provider_return_id: Option<&str>, before: u64,
                       after: u64, effective_access: Option<u8>| -> Result<(), EngineError> {
            let data = serde_json::json!({
                "outcome": outcome, "dispatch_kind": dispatch_kind,
                "chat": p.chat, "run": at.run.ord, "attempt": at.n,
                "effect_id": effect_id, "message_id": p.message_id,
                "provider_return_id": provider_return_id,
                "journal_seq_before": before, "journal_seq_after": after,
                "journal_change": if after > before { "advanced" } else if after == before { "unchanged" } else { "reset" },
                "effective_access": effective_access,
                "access_ceiling": if outcome == "already_done" { None } else { access_ceiling },
            });
            self.driver.observe_mechanism(
                &if outcome == "already_done" {
                    format!("provider-start:{effect_id}:{outcome}:{after}")
                } else { format!("provider-start:{effect_id}:{outcome}") }, "provider_start", data,
            ).map_err(|error| self.to_engine(error))?;
            if after > before {
                self.driver.observe_mechanism(
                    &format!("journal:{}:{after}", p.chat), "journal_observed",
                    serde_json::json!({
                        "chat": p.chat, "run": at.run.ord, "attempt": at.n,
                        "effect_id": effect_id, "message_id": p.message_id,
                        "journal_seq_before": before, "journal_seq_after": after,
                    }),
                ).map_err(|error| self.to_engine(error))?;
            }
            Ok(())
        };
        let intent = format!("start-intent:{effect_id}");
        if self.driver.meta(&marker).map_err(|error| self.to_engine(error))?.is_some() {
            tracing::info!(chat = %p.chat, message = %p.message_id,
                "outbox replay: this attempt already started; journal recovery owns it");
            let seq = engine.journal_seq(&p.chat);
            observe("already_done", None, seq, seq, None)?;
            return Ok(Ran::Nothing);
        }
        if let Some(baseline) = self.driver.meta(&intent).map_err(|error| self.to_engine(error))? {
            let (run, attempt, before) = parse_jseq(&baseline)
                .ok_or_else(|| EngineError::Other("invalid provider start intent".into()))?;
            if (run, attempt) != (at.run.ord, at.n) {
                return Err(EngineError::Other("provider start intent attempt mismatch".into()));
            }
            self.driver.put_meta(&format!("jseq:{}", p.chat), &baseline)
                .map_err(|error| self.to_engine(error))?;
            let after = engine.journal_seq(&p.chat);
            let owned = engine.owns_run(&p.chat);
            if owned || after > before {
                self.driver.put_meta(&marker, "1").map_err(|error| self.to_engine(error))?;
                observe("already_done", None, after, after, None)?;
            } else {
                observe("uncertain", None, before, after, None)?;
            }
            if !owned {
                self.end_stuck(engine, &p.chat, at).map_err(|error| self.to_engine(error))?;
            }
            return Ok(Ran::Nothing);
        }
        lock(&self.expect).insert(p.chat.clone(), at);
        let before = engine.journal_seq(&p.chat);
        let jseq = format!("{}:{}:{}", at.run.ord, at.n, before);
        let input = serde_json::to_string(&(at.run.ord, at.n, &p.message_id))
            .map_err(|error| EngineError::Other(error.to_string()))?;
        if let Err(error) = self.driver.put_meta(&format!("jseq:{}", p.chat), &jseq)
            .and_then(|()| self.driver.put_meta(&format!("start-input:{}", p.chat), &input))
            .and_then(|()| self.driver.put_meta(&intent, &jseq))
        {
            self.start_failed(&p.chat, at);
            return Err(self.to_engine(error));
        }
        let result = STARTING_ATTEMPT.scope(at, self.quietly(&p.chat, engine.start(&p))).await;
        let after = engine.journal_seq(&p.chat);
        match result {
            Ok(id) => {
                let marked = self.driver.put_meta(&marker, "1");
                self.mark_ran(&p.message_id);
                observe("engine_start_ok", Some(&id), before, after, engine.effective_access(&p.chat))?;
                marked.map_err(|error| self.to_engine(error))?;
                Ok(Ran::Started(id))
            }
            Err(err) => {
                let captured = observe("failed", None, before, after, None);
                lock(&self.claimed_msgs).remove(&p.message_id);
                self.start_failed(&p.chat, at);
                captured?;
                Err(err)
            }
        }
    }

    fn start_failed(&self, chat: &str, at: AttemptId) {
        if let Err(err) = self.end_unstarted(chat, at) {
            self.fail(err);
        }
        if let Some(e) = self.engine() {
            e.kick_drain(chat);
        }
    }

    fn may_start(&self, at: AttemptId) -> Result<bool, DriverError> {
        let state = self.driver.state()?;
        let Ok(th) = state.thread(at.run.thread) else {
            return Ok(false);
        };
        if !th.is_live() || th.attempt_id() != at {
            return Ok(false);
        }
        if th.int_req || th.held == Hold::Stop {
            let chat = self.chat_of(at.run.thread)?;
            self.terminal_unstarted(&chat, at, Outcome::Interrupted)?;
            return Ok(false);
        }
        Ok(true)
    }

    fn end_unstarted(&self, chat: &str, at: AttemptId) -> Result<(), DriverError> {
        self.terminal_unstarted(chat, at, Outcome::Failed)
    }

    fn terminal_unstarted(
        &self,
        chat: &str,
        at: AttemptId,
        outcome: Outcome,
    ) -> Result<(), DriverError> {
        let _g = lock(&self.gate);
        self.terminal_unstarted_locked(chat, at, outcome)
    }

    fn terminal_unstarted_locked(&self, chat: &str, at: AttemptId, outcome: Outcome) -> Result<(), DriverError> {
        let ev = Event::Run(RunEvent::ProviderTerminal {
            t: at.run.thread,
            run: at.run.ord,
            attempt: at.n,
            outcome,
        });
        let key = format!("startfail:{chat}:{}:{}", at.run.ord, at.n);
        self.driver.apply_if(&key, ev, &enabled(ev))?;
        self.pump(at.run.thread, chat, &mut Vec::new())
    }

    async fn steer(&self, engine: &dyn RunEngine, p: Pending) -> Result<Ran, EngineError> {
        let res = self.quietly(&p.chat, engine.steer(&p)).await;
        match &res {
            Ok(Ran::Steered(SteerOutcome::Accepted) | Ran::Started(_)) => {
                self.mark_ran(&p.message_id)
            }
            _ => {
                lock(&self.claimed_msgs).remove(&p.message_id);
            }
        }
        res
    }

    fn delay_for(&self, run: RunId) -> Duration {
        let resumed = self.driver.state().ok().and_then(|s| {
            s.thread(run.thread)
                .ok()
                .and_then(|th| th.run(run.ord).map(|r| r.start.kind()))
        }) == Some(RunKind::Resume);
        let d = &self.opts.limit_delays;
        let pick = if resumed {
            d.get(1).or(d.first())
        } else {
            d.first()
        };
        pick.copied().unwrap_or(Duration::from_secs(15 * 60))
    }

    fn first_due(&self, run: RunId, reset_at: Option<String>) -> i64 {
        let now = now_ms();
        let reset_at = reset_at.and_then(|v| v.parse::<i64>().ok());
        reset_at.filter(|at| *at > now).unwrap_or_else(|| {
            let ms = i64::try_from(self.delay_for(run).as_millis()).unwrap_or(i64::MAX);
            now.saturating_add(ms)
        })
    }

    fn arm_timer(&self, run: RunId) -> Result<(), DriverError> {
        let chat = self.chat_of(run.thread)?;
        let key = format!("timer:{chat}:{}", run.ord);
        let due = match self.driver.meta(&key)? {
            Some(v) => v
                .parse::<i64>()
                .map_err(|_| DriverError::Corrupt(format!("{key} = {v:?}")))?,
            None => {
                let reset_key = format!("limit_at:{chat}");
                let due = self.first_due(run, self.driver.meta(&reset_key)?);
                self.driver.put_meta(&key, &due.to_string())?;
                self.driver.delete_meta(&reset_key)?;
                due
            }
        };
        let Some(wire) = self.self_arc() else {
            return Ok(());
        };
        spawn_task("usage-limit timer", async move {
            let wait = u64::try_from(due.saturating_sub(now_ms())).unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(wait)).await;
            wire.timer_fired(run);
        });
        Ok(())
    }

    fn self_arc(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    fn timer_fired(&self, run: RunId) {
        if self.is_dead() {
            return;
        }
        let res = self.fire_locked(run);
        match res {
            Ok(chat) => {
                self.wake.notify_one();
                if let Some(e) = self.engine() {
                    e.kick_drain(&chat);
                }
            }
            Err(err) => {
                self.fail(err);
            }
        }
    }

    fn fire_locked(&self, run: RunId) -> Result<String, DriverError> {
        let _g = lock(&self.gate);
        let chat = self.chat_of(run.thread)?;
        let ev = Event::Run(RunEvent::ResetPasses {
            t: run.thread,
            run: run.ord,
        });
        self.driver
            .apply_if(&format!("reset:{chat}:{}", run.ord), ev, &enabled(ev))?;
        self.driver
            .delete_meta(&format!("timer:{chat}:{}", run.ord))?;
        self.pump(run.thread, &chat, &mut Vec::new())?;
        Ok(chat)
    }

    pub async fn recover(&self, engine: &dyn RunEngine) -> Result<(), EngineError> {
        self.retry_native_terminals(engine)
            .map_err(|e| self.to_engine(e))?;
        super::native::recover(&self.driver).map_err(|e| self.to_engine(e))?;
        let rows = self.driver.pending().map_err(|e| self.to_engine(e))?;
        let mut busy = HashSet::new();
        for row in rows {
            let t = order_key(&row.effect);
            if t.is_some_and(|t| busy.contains(&t)) {
                continue;
            }
            if !self.claim_pending(&row.effect_id).map_err(|e| self.to_engine(e))? {
                if lock(&self.claimed).contains(&row.effect_id) { busy.extend(t); }
                continue;
            }
            if let Err(err) = self.execute(engine, &row, true).await {
                tracing::warn!(effect = %row.effect_id, error = %err, "outbox replay failed");
                if !matches!(
                    row.effect,
                    Effect::StartRun { .. } | Effect::StartAttempt { .. } | Effect::SteerRun { .. }
                ) {
                    busy.extend(t);
                    lock(&self.claimed).remove(&row.effect_id);
                    continue;
                }
            }
            let completed = self.driver.complete(&row.effect_id);
            lock(&self.claimed).remove(&row.effect_id);
            completed.map_err(|e| self.to_engine(e))?;
        }
        self.reconcile(engine).map_err(|e| self.to_engine(e))?;
        self.pump_all().map_err(|e| self.to_engine(e))?;
        self.rearm().map_err(|e| self.to_engine(e))?;
        self.recovering.store(false, std::sync::atomic::Ordering::Release);
        self.wake.notify_one();
        Ok(())
    }

    fn reconcile(&self, engine: &dyn RunEngine) -> Result<(), DriverError> {
        let s = self.driver.state()?;
        let pending = self.driver.pending()?;
        let left = s
            .threads
            .values()
            .filter(|th| th.is_live() && self.boot_live.contains(&th.attempt_id()));
        for th in left {
            let chat = self.chat_of(th.id)?;
            if self.may_be_stuck(engine, th, &chat, &pending) {
                self.end_stuck(engine, &chat, th.attempt_id())?;
            }
        }
        Ok(())
    }

    fn pump_all(&self) -> Result<(), DriverError> {
        let _g = lock(&self.gate);
        let ids: Vec<ThreadId> = self.driver.state()?.threads.keys().copied().collect();
        for t in ids {
            let chat = self.chat_of(t)?;
            self.pump(t, &chat, &mut Vec::new())?;
        }
        Ok(())
    }

    fn rearm(&self) -> Result<(), DriverError> {
        let s = self.driver.state()?;
        for th in s.threads.values() {
            if let Some(ord) = th.ul.armed.filter(|_| !th.ul.due) {
                self.arm_timer(RunId { thread: th.id, ord })?;
            }
        }
        Ok(())
    }
}

fn order_key(effect: &Effect) -> Option<ThreadId> {
    match effect {
        Effect::CreateChild { child, .. } => Some(*child),
        Effect::StartRun { run, .. } | Effect::InterruptRun { run } | Effect::ArmTimer { run } => {
            Some(run.thread)
        }
        Effect::StartAttempt { attempt, .. }
        | Effect::SteerRun { attempt, .. }
        | Effect::InterruptAttempt { attempt } => Some(attempt.run.thread),
        Effect::RespondInput { child, .. } => Some(*child),
        Effect::NativeChanged { parent, .. } | Effect::DeliverNative { parent, .. } => {
            Some(*parent)
        }
        Effect::Reconfigure { t, .. } => Some(*t),
        Effect::Send(zeron_orch::Message::Offer { parent, .. }) => Some(*parent),
        Effect::Send(
            zeron_orch::Message::Notice { child, .. }
            | zeron_orch::Message::Stop { child, .. }
            | zeron_orch::Message::QuestionUp { child, .. }
            | zeron_orch::Message::AnswerDown { child, .. }
            | zeron_orch::Message::QuestionClosed { child, .. }
            | zeron_orch::Message::QuestionRelease { child, .. },
        ) => Some(*child),
        Effect::QuestionView { .. }
        | Effect::NotifyQuestion { .. }
        | Effect::CompleteWait { .. } => None,
    }
}

fn live_attempts(s: &OrchState) -> HashSet<AttemptId> {
    s.threads
        .values()
        .filter(|th| th.is_live())
        .map(Thread::attempt_id)
        .collect()
}

fn starts(effect: &Effect, t: ThreadId) -> bool {
    match effect {
        Effect::StartRun { run, .. } => run.thread == t,
        Effect::StartAttempt { attempt, .. } => attempt.run.thread == t,
        _ => false,
    }
}

fn parse_jseq(v: &str) -> Option<(u32, u32, u64)> {
    let mut it = v.split(':');
    let ord = it.next()?.parse().ok()?;
    let n = it.next()?.parse().ok()?;
    let seq = it.next()?.parse().ok()?;
    Some((ord, n, seq))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

tokio::task_local! {
    static COMMAND: std::cell::RefCell<Option<String>>;
}

pub async fn with_command<F: std::future::Future>(id: &str, fut: F) -> F::Output {
    COMMAND
        .scope(std::cell::RefCell::new(Some(id.to_owned())), fut)
        .await
}

fn take_command() -> Option<String> {
    COMMAND.try_with(|c| c.borrow_mut().take()).ok().flatten()
}

fn command_key(id: &str) -> String {
    format!("cmd:{id}")
}

pub fn spawn_task<F>(what: &'static str, fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
            true
        }
        Err(_) => {
            tracing::error!(
                task = what,
                "orchestrator task not started: no tokio runtime"
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "wire/provider_mechanism_tests.rs"]
mod provider_mechanism_tests;

#[cfg(test)]
#[path = "wire/stop_tests.rs"]
mod stop_tests;

tokio::task_local! {
    static STARTING_ATTEMPT: AttemptId;
}

pub(crate) fn starting_attempt() -> Option<AttemptId> {
    STARTING_ATTEMPT.try_with(|at| *at).ok()
}
