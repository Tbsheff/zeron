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

use super::DriverError;
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
    fn last_done(&self, chat: &str) -> Option<(u64, DoneStatus, Option<FailureClass>)>;
    fn journal_seq(&self, chat: &str) -> u64;
    fn kick_drain(&self, chat: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    Person,
    Redeliver,
    SendNow,
}

enum Decision {
    Refused(&'static str),
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
    let head_ok = th
        .queue
        .front()
        .is_some_and(|m| !matches!(m, Msg::Wake { .. }));
    (th.held == Hold::None && head_ok).then(|| {
        let key = format!("start:{chat}:{}", th.rord() + 1);
        (key, Event::Run(RunEvent::StartFromQueue { t }))
    })
}

fn enabled(ev: Event) -> impl Fn(&OrchState) -> bool {
    move |s| step(s, ev).is_ok()
}

pub struct OrchWire {
    me: std::sync::Weak<OrchWire>,
    driver: Driver,
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
    wake: tokio::sync::Notify,
}

impl OrchWire {
    pub fn open(
        store: Arc<DocsStore>,
        device: &str,
        opts: OrchOptions,
    ) -> Result<Arc<Self>, DriverError> {
        let driver = Driver::open(store, device, opts.config)?;
        let boot_live = live_attempts(&driver.state()?);
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            driver,
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
            wake: tokio::sync::Notify::new(),
        }))
    }

    pub fn driver(&self) -> &Driver {
        &self.driver
    }

    pub fn trace_lines(&self) -> Result<Vec<String>, DriverError> {
        self.driver.trace_lines()
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn attach(self: &Arc<Self>, engine: Arc<dyn RunEngine>) {
        *lock(&self.engine) = Some(engine);
        let wire = self.clone();
        spawn_task("worker", async move { wire.worker().await });
    }

    pub fn detach(&self) {
        lock(&self.engine).take();
        self.wake.notify_one();
    }

    fn engine(&self) -> Option<Arc<dyn RunEngine>> {
        lock(&self.engine).clone()
    }

    async fn worker(self: Arc<Self>) {
        loop {
            let tick = tokio::select! {
                () = self.wake.notified() => false,
                () = tokio::time::sleep(self.opts.reconcile_every) => true,
            };
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
            self.engine_interrupt(at.run.thread, chat, at)?;
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

    fn thread(&self, chat: &str) -> Result<ThreadId, DriverError> {
        let (n, fresh) = self.driver.intern(&format!("chat:{chat}"), "n:thread")?;
        let n = u32::try_from(n).map_err(|_| DriverError::Corrupt("thread id overflow".into()))?;
        if fresh {
            self.driver.put_meta(&format!("thread:{n}"), chat)?;
        }
        let t = ThreadId(n);
        let ev = Event::Del(DelEvent::OpenRoot {
            t,
            access: 2,
            reconfigure: true,
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
        self.note_command(cmd.as_deref())?;
        out
    }

    fn note_command(&self, cmd: Option<&str>) -> Result<(), DriverError> {
        match cmd {
            Some(id) => self.driver.put_meta(&command_key(id), "1"),
            None => Ok(()),
        }
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
        let n = self.msg(&p.message_id)?;
        if kind != SendKind::Redeliver && self.ran(&p.message_id)? {
            return Ok(Decision::Refused("R17: this message id already ran"));
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
                "R17: this message id is already in flight",
            ));
        }
        self.stash(n, p)?;
        let (key, ev) = self.send_event(th, p, n, kind, steerable)?;
        let key = cmd.map_or(key, command_key);
        let r = self.driver.apply_command(&key, cmd, ev)?;
        let mut fx = accepted_ids(&r);
        if let Applied::Rejected { reason, .. } = r.applied {
            tracing::info!(chat = %p.chat, ?reason, "orchestrator refused the send");
            return Ok(Decision::Refused("orchestrator refused the send"));
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
        match self
            .decide(&p, kind, steerable)
            .map_err(|e| self.to_engine(e))?
        {
            Decision::Refused(why) => {
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
        let r = self.driver.apply_command(&key, cmd.as_deref(), ev)?;
        self.note_command(cmd.as_deref())?;
        let mut fx = accepted_ids(&r);
        self.pump(t, chat, &mut fx)?;
        Ok(fx)
    }

    pub fn provider_done(&self, chat: &str, status: DoneStatus, failure: Option<FailureClass>) {
        if self.is_dead() || lock(&self.lose).remove(chat) {
            return;
        }
        let res = self
            .note_reset(chat, status, failure)
            .and_then(|()| self.done_locked(chat, outcome_of(status, failure)));
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

    fn done_locked(&self, chat: &str, outcome: Outcome) -> Result<bool, DriverError> {
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
        let at = lock(&self.expect)
            .get(chat)
            .copied()
            .unwrap_or(th.attempt_id());
        let ev = Event::Run(RunEvent::ProviderTerminal {
            t,
            run: at.run.ord,
            attempt: at.n,
            outcome,
        });
        let key = format!("done:{chat}:{}:{}", at.run.ord, at.n);
        let applied = self.driver.apply_if(&key, ev, &enabled(ev))?;
        if applied.is_none() && self.unrequested_stop(th, at, outcome) {
            self.engine_interrupt(t, chat, at)?;
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

    fn engine_interrupt(&self, t: ThreadId, chat: &str, at: AttemptId) -> Result<(), DriverError> {
        let key = format!("int:{chat}:{}:{}:engine", at.run.ord, at.n);
        let r = self
            .driver
            .apply(&key, Event::Del(DelEvent::InterruptChat { t }))?;
        for id in accepted_ids(&r) {
            self.driver.complete(&id)?;
        }
        Ok(())
    }

    pub async fn run_effects(
        &self,
        engine: &dyn RunEngine,
        only: Option<&[String]>,
    ) -> Vec<Result<Ran, EngineError>> {
        let rows = match self.driver.pending() {
            Ok(rows) => rows,
            Err(err) => return vec![Err(self.to_engine(err))],
        };
        let mut out = Vec::new();
        let mut busy = HashSet::new();
        for row in rows {
            if only.is_some_and(|ids| !ids.contains(&row.effect_id)) {
                continue;
            }
            let t = order_key(&row.effect);
            if t.is_some_and(|t| busy.contains(&t)) {
                continue;
            }
            if self.is_dead() || !lock(&self.claimed).insert(row.effect_id.clone()) {
                busy.extend(t);
                continue;
            }
            let res = self.execute(engine, &row, false).await;
            if let Err(err) = self.driver.complete(&row.effect_id) {
                out.push(Err(self.to_engine(err)));
                return out;
            }
            lock(&self.claimed).remove(&row.effect_id);
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
        match &row.effect {
            Effect::StartRun { run, start, .. } => {
                let at = AttemptId { run: *run, n: 1 };
                let p = self.start_pending(*run, *start).map_err(wrap)?;
                self.start(engine, &row.effect_id, at, p, replay).await
            }
            Effect::StartAttempt {
                attempt,
                msg,
                reason,
                ..
            } => {
                self.back_off(*reason, attempt.n).await;
                let p = self.attempt_pending(*attempt, *msg).map_err(wrap)?;
                self.start(engine, &row.effect_id, *attempt, p, replay)
                    .await
            }
            Effect::SteerRun { msg, .. } => {
                let p = self.pending_of(*msg).map_err(wrap)?;
                self.steer(engine, p).await
            }
            Effect::InterruptAttempt { attempt } => {
                let chat = self.chat_of(attempt.run.thread).map_err(wrap)?;
                self.quietly(&chat, engine.stop_attempt(&chat)).await;
                Ok(Ran::Nothing)
            }
            Effect::InterruptRun { run } => {
                let chat = self.chat_of(run.thread).map_err(wrap)?;
                engine.interrupt_run(&chat).await;
                Ok(Ran::Nothing)
            }
            Effect::ArmTimer { run } => {
                self.arm_timer(*run).map_err(wrap)?;
                Ok(Ran::Nothing)
            }
            _ => Ok(Ran::Nothing),
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

    fn start_pending(&self, run: RunId, start: Start) -> Result<Pending, DriverError> {
        match start {
            Start::Queued(Msg::Person(n) | Msg::Server(n)) => self.pending_of(n),
            Start::Resume => self.resume_pending(run),
            Start::Queued(other) => Err(DriverError::Corrupt(format!(
                "slice 3 cannot start {other:?}"
            ))),
        }
    }

    fn attempt_pending(&self, at: AttemptId, msg: Option<u64>) -> Result<Pending, DriverError> {
        if let Some(n) = msg {
            return self.pending_of(n);
        }
        let s = self.driver.state()?;
        let th = s
            .thread(at.run.thread)
            .map_err(|r| DriverError::Corrupt(format!("{r:?}")))?;
        let start = th
            .run(at.run.ord)
            .map(|r| r.start)
            .ok_or_else(|| DriverError::Corrupt(format!("no run {}", at.run.ord)))?;
        self.start_pending(at.run, start)
    }

    fn resume_pending(&self, run: RunId) -> Result<Pending, DriverError> {
        let chat = self.chat_of(run.thread)?;
        Ok(Pending {
            message_id: format!("orch-resume-{}", run.ord),
            via: Via::Resume {
                prompt: RESUME_PROMPT.to_owned(),
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
        replay: bool,
    ) -> Result<Ran, EngineError> {
        let marker = format!("fx:{effect_id}");
        if replay && self.driver.meta(&marker).ok().flatten().is_some() {
            tracing::info!(chat = %p.chat, message = %p.message_id,
                "outbox replay: this attempt already started; journal recovery owns it");
            return Ok(Ran::Nothing);
        }
        lock(&self.expect).insert(p.chat.clone(), at);
        let jseq = format!("{}:{}:{}", at.run.ord, at.n, engine.journal_seq(&p.chat));
        if let Err(err) = self.driver.put_meta(&format!("jseq:{}", p.chat), &jseq) {
            tracing::warn!(error = %err, "orchestrator journal mark failed");
        }
        match self.quietly(&p.chat, engine.start(&p)).await {
            Ok(id) => {
                if let Err(err) = self.driver.put_meta(&marker, "1") {
                    tracing::warn!(error = %err, "orchestrator start marker failed");
                }
                self.mark_ran(&p.message_id);
                Ok(Ran::Started(id))
            }
            Err(err) => {
                lock(&self.claimed_msgs).remove(&p.message_id);
                self.start_failed(&p.chat, at);
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

    fn end_unstarted(&self, chat: &str, at: AttemptId) -> Result<(), DriverError> {
        let _g = lock(&self.gate);
        let ev = Event::Run(RunEvent::ProviderTerminal {
            t: at.run.thread,
            run: at.run.ord,
            attempt: at.n,
            outcome: Outcome::Failed,
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
        let rows = self.driver.pending().map_err(|e| self.to_engine(e))?;
        for row in rows {
            if !lock(&self.claimed).insert(row.effect_id.clone()) {
                continue;
            }
            if let Err(err) = self.execute(engine, &row, true).await {
                tracing::warn!(effect = %row.effect_id, error = %err, "outbox replay failed");
            }
            self.driver
                .complete(&row.effect_id)
                .map_err(|e| self.to_engine(e))?;
            lock(&self.claimed).remove(&row.effect_id);
        }
        self.reconcile(engine).map_err(|e| self.to_engine(e))?;
        self.rearm().map_err(|e| self.to_engine(e))?;
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
        Effect::StartRun { run, .. }
        | Effect::InterruptRun { run }
        | Effect::ArmTimer { run } => Some(run.thread),
        Effect::StartAttempt { attempt, .. }
        | Effect::SteerRun { attempt, .. }
        | Effect::InterruptAttempt { attempt } => Some(attempt.run.thread),
        Effect::CompleteWait { .. } | Effect::Reconfigure { .. } | Effect::Send(_) => None,
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
