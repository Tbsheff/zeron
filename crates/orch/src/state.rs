use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::event::{Effect, Reject};
use crate::ids::{AttemptId, RunId, TaskId, ThreadId};
use crate::small::{VecMap, VecSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Config {
    pub max_depth: u8,
    pub max_fanout: usize,
    pub max_queue: usize,
    pub max_retry: u32,
    pub run_window: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_fanout: 8,
            max_queue: 64,
            max_retry: 2,
            run_window: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Hold {
    None,
    Stop,
    Fail,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Msg {
    Person(u64),
    Server(u64),
    Task,
    Wake { run: u32, generation: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Start {
    Queued(Msg),
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunKind {
    Person,
    Server,
    Task,
    Wake,
    Resume,
}

impl Start {
    pub fn kind(self) -> RunKind {
        match self {
            Start::Queued(Msg::Person(_)) => RunKind::Person,
            Start::Queued(Msg::Server(_)) => RunKind::Server,
            Start::Queued(Msg::Task) => RunKind::Task,
            Start::Queued(Msg::Wake { .. }) => RunKind::Wake,
            Start::Resume => RunKind::Resume,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunStatus {
    Live,
    Completed,
    Failed,
    Interrupted,
    UsageLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Reason {
    Initial,
    SteeringRestart,
    Retry,
    ProviderRecovery,
    AccessRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Node {
    Root,
    Child,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Outcome {
    Completed,
    Failed,
    Interrupted,
    UsageLimit,
    Transient,
    Crash,
}

impl Outcome {
    pub fn retryable(self) -> bool {
        matches!(self, Outcome::Transient | Outcome::Crash)
    }

    pub fn end_status(self) -> RunStatus {
        match self {
            Outcome::Completed => RunStatus::Completed,
            Outcome::Interrupted => RunStatus::Interrupted,
            Outcome::UsageLimit => RunStatus::UsageLimit,
            Outcome::Failed | Outcome::Transient | Outcome::Crash => RunStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProviderEvent {
    pub node: Node,
    pub run: u32,
    pub attempt: u32,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Disp {
    Open,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CohortSt {
    None,
    Offered,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cohort {
    pub disp: Disp,
    pub st: CohortSt,
    pub del: VecSet<ThreadId>,
    pub generation: u32,
}

impl Default for Cohort {
    fn default() -> Self {
        Self {
            disp: Disp::Open,
            st: CohortSt::None,
            del: VecSet::new(),
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Run {
    pub start: Start,
    pub status: RunStatus,
    pub cohort: Cohort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TaskSt {
    Running,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WakeRule {
    Always,
    SettledOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Delivery {
    Pending,
    Claimed,
    Delivered,
    Acknowledged,
    Disposed,
}

impl Delivery {
    pub fn handled(self) -> bool {
        matches!(
            self,
            Delivery::Delivered | Delivery::Acknowledged | Delivery::Disposed
        )
    }

    pub fn unhandled(self) -> bool {
        matches!(self, Delivery::Pending | Delivery::Claimed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Task {
    pub st: TaskSt,
    pub wake: WakeRule,
    pub delivery: Delivery,
    pub cohort: u32,
    pub run: u32,
    pub pushes: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attempt {
    pub n: u32,
    pub reason: Reason,
    pub emitted: bool,
    pub kid_open: bool,
    pub retries: u32,
    pub steers: u32,
}

impl Default for Attempt {
    fn default() -> Self {
        Self {
            n: 0,
            reason: Reason::Initial,
            emitted: false,
            kid_open: false,
            retries: 0,
            steers: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct UsageLimit {
    pub armed: Option<u32>,
    pub due: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub parent: Option<ThreadId>,
    pub depth: u8,
    pub access: u8,
    pub reconfigure: bool,
    pub queue: VecDeque<Msg>,
    pub held: Hold,
    pub run_base: u32,
    pub runs: Vec<Run>,
    pub int_req: bool,
    pub attempt: Attempt,
    pub inbox: VecSet<ProviderEvent>,
    pub ul: UsageLimit,
    pub wait_on: Option<ThreadId>,
    pub late_wait: Option<ThreadId>,
    pub task: Option<Task>,
}

impl Thread {
    pub fn new(
        id: ThreadId,
        parent: Option<ThreadId>,
        depth: u8,
        access: u8,
        reconfigure: bool,
    ) -> Self {
        Self {
            id,
            parent,
            depth,
            access,
            reconfigure,
            queue: VecDeque::new(),
            held: Hold::None,
            run_base: 0,
            runs: Vec::new(),
            int_req: false,
            attempt: Attempt::default(),
            inbox: VecSet::new(),
            ul: UsageLimit::default(),
            wait_on: None,
            late_wait: None,
            task: None,
        }
    }

    pub fn rord(&self) -> u32 {
        let n = u32::try_from(self.runs.len()).unwrap_or(u32::MAX);
        self.run_base.saturating_add(n)
    }

    pub fn ord_at(&self, i: usize) -> u32 {
        let n = u32::try_from(i).unwrap_or(u32::MAX);
        self.run_base.saturating_add(n).saturating_add(1)
    }

    fn index_of(&self, ord: u32) -> Option<usize> {
        let i = ord.checked_sub(1)?.checked_sub(self.run_base)?;
        usize::try_from(i).ok()
    }

    pub fn retired(&self, ord: u32) -> bool {
        ord >= 1 && ord <= self.run_base
    }

    pub fn live_count(&self) -> usize {
        self.runs
            .iter()
            .filter(|r| r.status == RunStatus::Live)
            .count()
    }

    pub fn is_live(&self) -> bool {
        self.live_count() > 0
    }

    pub fn run(&self, ord: u32) -> Option<&Run> {
        self.runs.get(self.index_of(ord)?)
    }

    pub fn run_mut(&mut self, ord: u32) -> Option<&mut Run> {
        let i = self.index_of(ord)?;
        self.runs.get_mut(i)
    }

    pub fn cohort(&self, ord: u32) -> Option<&Cohort> {
        self.run(ord).map(|r| &r.cohort)
    }

    pub fn cohort_mut(&mut self, ord: u32) -> Option<&mut Cohort> {
        self.run_mut(ord).map(|r| &mut r.cohort)
    }

    pub fn latest_kind(&self) -> Option<RunKind> {
        self.runs.last().map(|r| r.start.kind())
    }

    pub fn last_out(&self) -> Option<RunStatus> {
        self.runs
            .iter()
            .rev()
            .map(|r| r.status)
            .find(|s| *s != RunStatus::Live)
    }

    pub fn is_task_run(&self) -> bool {
        self.task.is_some() && self.latest_kind() == Some(RunKind::Task)
    }

    pub fn run_id(&self) -> RunId {
        RunId {
            thread: self.id,
            ord: self.rord(),
        }
    }

    pub fn attempt_id(&self) -> AttemptId {
        AttemptId {
            run: self.run_id(),
            n: self.attempt.n,
        }
    }

    pub fn is_current(&self, ev: &ProviderEvent) -> bool {
        self.is_live() && ev.run == self.rord() && ev.attempt == self.attempt.n
    }

    pub fn may_retry(&self, max_retry: u32) -> bool {
        self.attempt.retries < max_retry && !self.int_req
    }

    pub(crate) fn new_attempt(&mut self, n: u32, reason: Reason) {
        self.attempt.n = n;
        self.attempt.reason = reason;
        self.attempt.emitted = false;
        self.attempt.kid_open = false;
    }

    pub(crate) fn supersede_attempt(&mut self, reason: Reason) {
        if !self.attempt.emitted {
            self.inbox.insert(ProviderEvent {
                node: Node::Root,
                run: self.rord(),
                attempt: self.attempt.n,
                outcome: Outcome::Interrupted,
            });
        }
        self.new_attempt(self.attempt.n.saturating_add(1), reason);
    }

    pub(crate) fn mark_interrupt(&mut self) {
        self.int_req = self.int_req || self.is_live();
        self.held = Hold::Stop;
        self.ul = UsageLimit::default();
    }

    pub fn task_id(&self) -> Option<TaskId> {
        self.task.map(|t| TaskId {
            child: self.id,
            run: t.run,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Message {
    Notice {
        child: ThreadId,
        run: u32,
    },
    Offer {
        parent: ThreadId,
        run: u32,
        generation: u32,
    },
    Stop {
        child: ThreadId,
        stop: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrchState {
    pub config: Config,
    pub threads: VecMap<ThreadId, Thread>,
    pub outbox: VecSet<Message>,
    pub stop_receipts: VecSet<(ThreadId, u32)>,
    pub stops: u32,
}

impl OrchState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            threads: VecMap::new(),
            outbox: VecSet::new(),
            stop_receipts: VecSet::new(),
            stops: 0,
        }
    }

    pub fn thread(&self, t: ThreadId) -> Result<&Thread, Reject> {
        self.threads.get(&t).ok_or(Reject::UnknownThread)
    }

    pub fn thread_mut(&mut self, t: ThreadId) -> Result<&mut Thread, Reject> {
        self.threads.get_mut(&t).ok_or(Reject::UnknownThread)
    }

    pub fn task(&self, c: ThreadId) -> Result<Task, Reject> {
        self.thread(c)?.task.ok_or(Reject::NotKid)
    }

    pub(crate) fn task_mut(&mut self, c: ThreadId) -> Result<&mut Task, Reject> {
        self.thread_mut(c)?.task.as_mut().ok_or(Reject::NotKid)
    }

    pub fn kids(&self, t: ThreadId) -> Vec<ThreadId> {
        self.threads
            .values()
            .filter(|c| c.parent == Some(t))
            .map(|c| c.id)
            .collect()
    }

    pub fn unsettled_kids(&self, t: ThreadId) -> usize {
        self.threads
            .values()
            .filter(|c| c.parent == Some(t) && c.task.is_some_and(|k| k.st != TaskSt::Settled))
            .count()
    }

    pub fn desc(&self, t: ThreadId) -> Vec<ThreadId> {
        let mut out = Vec::new();
        let mut frontier = vec![t];
        while let Some(x) = frontier.pop() {
            for k in self.kids(x) {
                if k != t && !out.contains(&k) {
                    out.push(k);
                    frontier.push(k);
                }
            }
        }
        out
    }

    pub fn cohort_kids(&self, p: ThreadId, r: u32) -> Vec<ThreadId> {
        self.threads
            .values()
            .filter(|c| c.parent == Some(p) && c.task.is_some_and(|k| k.cohort == r))
            .map(|c| c.id)
            .collect()
    }

    pub fn cohort_pending(&self, p: ThreadId, r: u32) -> Vec<ThreadId> {
        self.cohort_kids(p, r)
            .into_iter()
            .filter(|c| {
                self.task(*c)
                    .is_ok_and(|k| k.st == TaskSt::Settled && k.delivery == Delivery::Pending)
            })
            .collect()
    }

    pub fn cohort_live(&self, p: ThreadId, r: u32) -> bool {
        self.thread(p).is_ok_and(|t| t.is_live() && t.rord() == r)
    }

    fn oldest_quiet(&self, t: ThreadId) -> bool {
        let Ok(th) = self.thread(t) else {
            return false;
        };
        let Some(run) = th.runs.first() else {
            return false;
        };
        let o = th.ord_at(0);
        let own_quiet = run.status != RunStatus::Live
            && run.cohort.st == CohortSt::None
            && run.cohort.del.is_empty()
            && th.ul.armed != Some(o)
            && !th.inbox.iter().any(|e| e.run == o)
            && !th
                .queue
                .iter()
                .any(|m| matches!(m, Msg::Wake { run, .. } if *run == o));
        let offer_quiet = !self
            .outbox
            .iter()
            .any(|m| matches!(m, Message::Offer { parent, run, .. } if *parent == t && *run == o));
        let kids_quiet = self.threads.values().all(|c| {
            c.parent != Some(t)
                || c.task.is_none_or(|k| {
                    k.cohort != o || (k.st == TaskSt::Settled && k.delivery.handled())
                })
        });
        own_quiet && offer_quiet && kids_quiet
    }

    pub(crate) fn retire_runs(&mut self) {
        let window = self.config.run_window.max(1);
        let long: Vec<ThreadId> = self
            .threads
            .values()
            .filter(|th| th.runs.len() > window)
            .map(|th| th.id)
            .collect();
        for t in long {
            while self.thread(t).is_ok_and(|th| th.runs.len() > window) && self.oldest_quiet(t) {
                if let Some(th) = self.threads.get_mut(&t) {
                    th.runs.remove(0);
                    th.run_base = th.run_base.saturating_add(1);
                }
            }
        }
    }

    pub(crate) fn send(&mut self, m: Message, fx: &mut Vec<Effect>) {
        self.outbox.insert(m);
        fx.push(Effect::Send(m));
    }
}

pub(crate) fn ensure(cond: bool, r: Reject) -> Result<(), Reject> {
    if cond { Ok(()) } else { Err(r) }
}
