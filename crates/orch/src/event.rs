use crate::ids::{AttemptId, RunId, TaskId, ThreadId};
use crate::state::{Message, Outcome, ProviderEvent, Reason, Start};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Mode {
    Steer,
    Restart,
    Queue,
    Now,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DelegateMode {
    Async,
    Wait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum How {
    Ack,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Event {
    Run(RunEvent),
    Del(DelEvent),
    Outbox(OutboxEvent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum RunEvent {
    PersonSend {
        t: ThreadId,
        mode: Mode,
        msg: u64,
    },
    Steer {
        t: ThreadId,
        msg: u64,
    },
    RestartActive {
        t: ThreadId,
        mode: Mode,
        msg: u64,
    },
    RequestInterrupt {
        t: ThreadId,
    },
    ServerEnqueue {
        t: ThreadId,
        msg: u64,
    },
    ProviderTerminal {
        t: ThreadId,
        run: u32,
        attempt: u32,
        outcome: Outcome,
    },
    ChildStart {
        t: ThreadId,
    },
    ChildEnd {
        t: ThreadId,
    },
    ResetPasses {
        t: ThreadId,
        run: u32,
    },
    StartFromQueue {
        t: ThreadId,
    },
    UsageLimitRecovery {
        t: ThreadId,
    },
    IngestEnd {
        t: ThreadId,
        ev: ProviderEvent,
    },
    RetryAttempt {
        t: ThreadId,
        ev: ProviderEvent,
    },
    IngestChild {
        t: ThreadId,
        ev: ProviderEvent,
    },
    IngestStale {
        t: ThreadId,
        ev: ProviderEvent,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum DelEvent {
    OpenRoot {
        t: ThreadId,
        access: u8,
        reconfigure: bool,
    },
    Stop {
        t: ThreadId,
    },
    NarrowAccess {
        t: ThreadId,
        access: u8,
    },
    InterruptChat {
        t: ThreadId,
    },
    Delegate {
        t: ThreadId,
        child: ThreadId,
        mode: DelegateMode,
        access: u8,
        reconfigure: bool,
    },
    ReadResult {
        p: ThreadId,
        c: ThreadId,
        run: u32,
    },
    WaitReturn {
        t: ThreadId,
    },
    WaitTimeout {
        t: ThreadId,
    },
    WaitAbandon {
        t: ThreadId,
    },
    LateWaitAck {
        t: ThreadId,
    },
    StartFromQueueWake {
        t: ThreadId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum OutboxEvent {
    DoNotice {
        child: ThreadId,
        run: u32,
        how: How,
    },
    DoOffer {
        parent: ThreadId,
        run: u32,
        generation: u32,
        how: How,
    },
    DoStop {
        child: ThreadId,
        stop: u32,
        how: How,
    },
    CrashEngine,
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::Run(e) => e.name(),
            Event::Del(e) => e.name(),
            Event::Outbox(e) => e.name(),
        }
    }
}

impl RunEvent {
    pub fn name(&self) -> &'static str {
        match self {
            RunEvent::PersonSend { .. } => "PersonSend",
            RunEvent::Steer { .. } => "Steer",
            RunEvent::RestartActive { .. } => "RestartActive",
            RunEvent::RequestInterrupt { .. } => "RequestInterrupt",
            RunEvent::ServerEnqueue { .. } => "ServerEnqueue",
            RunEvent::ProviderTerminal { .. } => "ProviderTerminal",
            RunEvent::ChildStart { .. } => "ChildStart",
            RunEvent::ChildEnd { .. } => "ChildEnd",
            RunEvent::ResetPasses { .. } => "ResetPasses",
            RunEvent::StartFromQueue { .. } => "StartFromQueue",
            RunEvent::UsageLimitRecovery { .. } => "UsageLimitRecovery",
            RunEvent::IngestEnd { .. } => "IngestEnd",
            RunEvent::RetryAttempt { .. } => "RetryAttempt",
            RunEvent::IngestChild { .. } => "IngestChild",
            RunEvent::IngestStale { .. } => "IngestStale",
        }
    }
}

impl DelEvent {
    pub fn name(&self) -> &'static str {
        match self {
            DelEvent::OpenRoot { .. } => "OpenRoot",
            DelEvent::Stop { .. } => "Stop",
            DelEvent::NarrowAccess { .. } => "NarrowAccess",
            DelEvent::InterruptChat { .. } => "InterruptChat",
            DelEvent::Delegate { .. } => "Delegate",
            DelEvent::ReadResult { .. } => "ReadResult",
            DelEvent::WaitReturn { .. } => "WaitReturn",
            DelEvent::WaitTimeout { .. } => "WaitTimeout",
            DelEvent::WaitAbandon { .. } => "WaitAbandon",
            DelEvent::LateWaitAck { .. } => "LateWaitAck",
            DelEvent::StartFromQueueWake { .. } => "StartFromQueueWake",
        }
    }
}

impl OutboxEvent {
    pub fn name(&self) -> &'static str {
        match self {
            OutboxEvent::DoNotice { .. } => "DoNotice",
            OutboxEvent::DoOffer { .. } => "DoOffer",
            OutboxEvent::DoStop { .. } => "DoStop",
            OutboxEvent::CrashEngine => "CrashEngine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WaitOutcome {
    Result,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    StartRun {
        run: RunId,
        start: Start,
        access: u8,
        delivered: Vec<TaskId>,
    },
    StartAttempt {
        attempt: AttemptId,
        reason: Reason,
        access: u8,
        msg: Option<u64>,
    },
    SteerRun {
        attempt: AttemptId,
        msg: u64,
    },
    InterruptAttempt {
        attempt: AttemptId,
    },
    InterruptRun {
        run: RunId,
    },
    ArmTimer {
        run: RunId,
    },
    CreateChild {
        parent: ThreadId,
        child: ThreadId,
        depth: u8,
        access: u8,
        mode: DelegateMode,
    },
    CompleteWait {
        t: ThreadId,
        task: TaskId,
        outcome: WaitOutcome,
    },
    Reconfigure {
        t: ThreadId,
        access: u8,
    },
    Send(Message),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Reject {
    UnknownThread,
    ThreadExists,
    Corrupt,
    QueueFull,
    QueueEmpty,
    SteerApplies,
    NotRestartMode,
    NotLive,
    RunLive,
    Held,
    HeadIsWake,
    HeadNotWake,
    StaleWake,
    InterruptRequested,
    InterruptNotRequested,
    TooLate,
    AlreadyEmitted,
    NotCurrent,
    Current,
    ChildOpen,
    NoChildOpen,
    NotInInbox,
    NotRoot,
    NotChildNode,
    UseRetry,
    NotRetryable,
    RetryBudget,
    Waiting,
    NotWaiting,
    TimerNotArmed,
    TimerAlreadyFired,
    UsageLimitNotArmed,
    LastNotUsageLimit,
    ResetNotDue,
    DepthCap,
    FanoutCap,
    AccessWider,
    NotNarrower,
    StaleRun,
    NotKid,
    NotSettled,
    Settled,
    AlreadyHandled,
    NoLateWait,
    NotInOutbox,
}

impl Reject {
    pub const ALL: [Reject; 45] = [
        Reject::UnknownThread,
        Reject::ThreadExists,
        Reject::Corrupt,
        Reject::QueueFull,
        Reject::QueueEmpty,
        Reject::SteerApplies,
        Reject::NotRestartMode,
        Reject::NotLive,
        Reject::RunLive,
        Reject::Held,
        Reject::HeadIsWake,
        Reject::HeadNotWake,
        Reject::StaleWake,
        Reject::InterruptRequested,
        Reject::InterruptNotRequested,
        Reject::TooLate,
        Reject::AlreadyEmitted,
        Reject::NotCurrent,
        Reject::Current,
        Reject::ChildOpen,
        Reject::NoChildOpen,
        Reject::NotInInbox,
        Reject::NotRoot,
        Reject::NotChildNode,
        Reject::UseRetry,
        Reject::NotRetryable,
        Reject::RetryBudget,
        Reject::Waiting,
        Reject::NotWaiting,
        Reject::TimerNotArmed,
        Reject::TimerAlreadyFired,
        Reject::UsageLimitNotArmed,
        Reject::LastNotUsageLimit,
        Reject::ResetNotDue,
        Reject::DepthCap,
        Reject::FanoutCap,
        Reject::AccessWider,
        Reject::NotNarrower,
        Reject::StaleRun,
        Reject::NotKid,
        Reject::NotSettled,
        Reject::Settled,
        Reject::AlreadyHandled,
        Reject::NoLateWait,
        Reject::NotInOutbox,
    ];
}
