use crate::delegation;
use crate::event::{Effect, Mode, Reject, RunEvent};
use crate::ids::{TaskId, ThreadId};
use crate::state::{
    Hold, Msg, Node, OrchState, Outcome, ProviderEvent, Reason, Run, RunStatus, Start, Thread,
    UsageLimit, ensure,
};

pub(crate) fn apply(s: &mut OrchState, e: RunEvent, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    match e {
        RunEvent::PersonSend { t, mode, msg } => person_send(s, t, mode, msg),
        RunEvent::Steer { t, msg } => steer(s, t, msg, fx),
        RunEvent::RestartActive { t, mode, msg } => restart_active(s, t, mode, msg, fx),
        RunEvent::RequestInterrupt { t } => request_interrupt(s, t, fx),
        RunEvent::ServerEnqueue { t, msg } => server_enqueue(s, t, msg),
        RunEvent::ProviderTerminal {
            t,
            run,
            attempt,
            outcome,
        } => provider_terminal(s, t, run, attempt, outcome),
        RunEvent::ChildStart { t } => child_start(s, t),
        RunEvent::ChildEnd { t } => child_end(s, t),
        RunEvent::ResetPasses { t, run } => reset_passes(s, t, run),
        RunEvent::StartFromQueue { t } => start_from_queue(s, t, fx),
        RunEvent::UsageLimitRecovery { t } => usage_limit_recovery(s, t, fx),
        RunEvent::IngestEnd { t, ev } => ingest_end(s, t, ev, fx),
        RunEvent::RetryAttempt { t, ev } => retry_attempt(s, t, ev, fx),
        RunEvent::IngestChild { t, ev } => ingest_child(s, t, ev),
        RunEvent::IngestStale { t, ev } => ingest_stale(s, t, ev),
    }
}

fn person_send(s: &mut OrchState, t: ThreadId, mode: Mode, msg: u64) -> Result<(), Reject> {
    let max = s.config.max_queue;
    let th = s.thread_mut(t)?;
    ensure(th.queue.len() < max, Reject::QueueFull)?;
    ensure(person_may_queue(th, mode), Reject::SteerApplies)?;
    th.queue.push_back(Msg::Person(msg));
    th.held = Hold::None;
    Ok(())
}

fn person_may_queue(th: &Thread, mode: Mode) -> bool {
    !th.is_live() || matches!(mode, Mode::Queue | Mode::Now) || th.int_req || th.attempt.emitted
}

fn ensure_steerable(th: &Thread) -> Result<(), Reject> {
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.int_req, Reject::InterruptRequested)?;
    ensure(!th.attempt.emitted, Reject::TooLate)
}

fn steer(s: &mut OrchState, t: ThreadId, msg: u64, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure_steerable(th)?;
    th.attempt.steers = th.attempt.steers.saturating_add(1);
    fx.push(Effect::SteerRun {
        attempt: th.attempt_id(),
        msg,
    });
    Ok(())
}

fn restart_active(
    s: &mut OrchState,
    t: ThreadId,
    mode: Mode,
    msg: u64,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    ensure(
        matches!(mode, Mode::Steer | Mode::Restart),
        Reject::NotRestartMode,
    )?;
    let th = s.thread_mut(t)?;
    ensure_steerable(th)?;
    restart_attempt(th, Reason::SteeringRestart, Some(msg), fx);
    th.attempt.steers = 0;
    Ok(())
}

pub(crate) fn restart_attempt(
    th: &mut Thread,
    reason: Reason,
    msg: Option<u64>,
    fx: &mut Vec<Effect>,
) {
    let old = th.attempt_id();
    th.supersede_attempt(reason);
    fx.push(Effect::InterruptAttempt { attempt: old });
    fx.push(Effect::StartAttempt {
        attempt: th.attempt_id(),
        reason,
        access: th.access,
        msg,
    });
}

fn request_interrupt(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    let live = th.is_live();
    th.mark_interrupt();
    if live {
        fx.push(Effect::InterruptRun { run: th.run_id() });
    }
    Ok(())
}

fn server_enqueue(s: &mut OrchState, t: ThreadId, msg: u64) -> Result<(), Reject> {
    let max = s.config.max_queue;
    let th = s.thread_mut(t)?;
    ensure(th.queue.len() < max, Reject::QueueFull)?;
    th.queue.push_back(Msg::Server(msg));
    Ok(())
}

fn provider_terminal(
    s: &mut OrchState,
    t: ThreadId,
    run: u32,
    attempt: u32,
    outcome: Outcome,
) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(
        run == th.rord() && attempt == th.attempt.n,
        Reject::NotCurrent,
    )?;
    ensure(!th.attempt.emitted, Reject::AlreadyEmitted)?;
    ensure(
        outcome != Outcome::Interrupted || th.int_req,
        Reject::InterruptNotRequested,
    )?;
    th.attempt.emitted = true;
    th.inbox.insert(ProviderEvent {
        node: Node::Root,
        run,
        attempt,
        outcome,
    });
    Ok(())
}

fn child_start(s: &mut OrchState, t: ThreadId) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.attempt.emitted, Reject::AlreadyEmitted)?;
    ensure(!th.attempt.kid_open, Reject::ChildOpen)?;
    th.attempt.kid_open = true;
    Ok(())
}

fn child_end(s: &mut OrchState, t: ThreadId) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.attempt.kid_open, Reject::NoChildOpen)?;
    th.attempt.kid_open = false;
    let ev = ProviderEvent {
        node: Node::Child,
        run: th.rord(),
        attempt: th.attempt.n,
        outcome: Outcome::Completed,
    };
    th.inbox.insert(ev);
    Ok(())
}

fn reset_passes(s: &mut OrchState, t: ThreadId, run: u32) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.ul.armed == Some(run), Reject::TimerNotArmed)?;
    ensure(!th.ul.due, Reject::TimerAlreadyFired)?;
    th.ul.due = true;
    Ok(())
}

fn start_from_queue(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    let head = *th.queue.front().ok_or(Reject::QueueEmpty)?;
    ensure(!matches!(head, Msg::Wake { .. }), Reject::HeadIsWake)?;
    ensure(!th.is_live(), Reject::RunLive)?;
    ensure(th.held == Hold::None, Reject::Held)?;
    th.queue.pop_front();
    begin_run(th, Start::Queued(head), Vec::new(), fx);
    Ok(())
}

pub(crate) fn begin_run(
    th: &mut Thread,
    start: Start,
    delivered: Vec<TaskId>,
    fx: &mut Vec<Effect>,
) {
    th.runs.push(Run {
        start,
        status: RunStatus::Live,
        cohort: Default::default(),
    });
    th.int_req = false;
    th.new_attempt(1, Reason::Initial);
    th.attempt.retries = 0;
    th.attempt.steers = 0;
    th.ul = UsageLimit::default();
    th.wait_on = None;
    fx.push(Effect::StartRun {
        run: th.run_id(),
        start,
        access: th.access,
        delivered,
    });
}

fn usage_limit_recovery(
    s: &mut OrchState,
    t: ThreadId,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(
        th.ul.armed.is_some() && th.ul.armed == Some(th.rord()),
        Reject::UsageLimitNotArmed,
    )?;
    ensure(
        th.last_out() == Some(RunStatus::UsageLimit),
        Reject::LastNotUsageLimit,
    )?;
    ensure(th.ul.due, Reject::ResetNotDue)?;
    ensure(!th.is_live(), Reject::RunLive)?;
    ensure(matches!(th.held, Hold::None | Hold::Limit), Reject::Held)?;
    th.held = Hold::None;
    begin_run(th, Start::Resume, Vec::new(), fx);
    Ok(())
}

fn ensure_current_root(th: &Thread, ev: &ProviderEvent) -> Result<(), Reject> {
    ensure(th.inbox.contains(ev), Reject::NotInInbox)?;
    ensure(ev.node == Node::Root, Reject::NotRoot)?;
    ensure(th.is_current(ev), Reject::NotCurrent)
}

fn ingest_end(
    s: &mut OrchState,
    t: ThreadId,
    ev: ProviderEvent,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let max_retry = s.config.max_retry;
    let th = s.thread_mut(t)?;
    ensure_current_root(th, &ev)?;
    ensure(
        !(ev.outcome.retryable() && th.may_retry(max_retry)),
        Reject::UseRetry,
    )?;
    ensure(
        ev.outcome != Outcome::Completed || th.wait_on.is_none(),
        Reject::Waiting,
    )?;
    th.inbox.remove(&ev);
    let out = ev.outcome.end_status();
    finish_run(th, out, fx);
    crate::questions::end_run(s, t, fx)?;
    delegation::end_run(s, t, fx)
}

fn finish_run(th: &mut Thread, out: RunStatus, fx: &mut Vec<Effect>) {
    let was_stop = th.held == Hold::Stop;
    let ord = th.rord();
    if let Some(run) = th.run_mut(ord) {
        run.status = out;
    }
    th.int_req = false;
    th.held = next_hold(th.held, out);
    th.attempt.kid_open = false;
    if out == RunStatus::UsageLimit {
        th.ul.due = false;
        if !was_stop {
            th.ul.armed = Some(ord);
            fx.push(Effect::ArmTimer { run: th.run_id() });
        }
    }
}

fn next_hold(held: Hold, out: RunStatus) -> Hold {
    match (out, held) {
        (RunStatus::Failed | RunStatus::UsageLimit, Hold::Stop) => Hold::Stop,
        (RunStatus::Failed, _) => Hold::Fail,
        (RunStatus::UsageLimit, _) => Hold::Limit,
        _ => held,
    }
}

fn retry_attempt(
    s: &mut OrchState,
    t: ThreadId,
    ev: ProviderEvent,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let max_retry = s.config.max_retry;
    let th = s.thread_mut(t)?;
    ensure_current_root(th, &ev)?;
    ensure(ev.outcome.retryable(), Reject::NotRetryable)?;
    ensure(th.attempt.retries < max_retry, Reject::RetryBudget)?;
    ensure(!th.int_req, Reject::InterruptRequested)?;
    th.inbox.remove(&ev);
    let reason = if ev.outcome == Outcome::Transient {
        Reason::Retry
    } else {
        Reason::ProviderRecovery
    };
    th.new_attempt(th.attempt.n.saturating_add(1), reason);
    th.attempt.retries = th.attempt.retries.saturating_add(1);
    fx.push(Effect::StartAttempt {
        attempt: th.attempt_id(),
        reason,
        access: th.access,
        msg: None,
    });
    Ok(())
}

fn ingest_child(s: &mut OrchState, t: ThreadId, ev: ProviderEvent) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.inbox.contains(&ev), Reject::NotInInbox)?;
    ensure(ev.node == Node::Child, Reject::NotChildNode)?;
    ensure(th.is_current(&ev), Reject::NotCurrent)?;
    th.inbox.remove(&ev);
    Ok(())
}

fn ingest_stale(s: &mut OrchState, t: ThreadId, ev: ProviderEvent) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.inbox.contains(&ev), Reject::NotInInbox)?;
    ensure(!th.is_current(&ev), Reject::Current)?;
    th.inbox.remove(&ev);
    Ok(())
}

#[cfg(kani)]
pub(crate) fn kani_ingest_end(
    s: &mut OrchState,
    t: ThreadId,
    ev: ProviderEvent,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    ingest_end(s, t, ev, fx)
}
