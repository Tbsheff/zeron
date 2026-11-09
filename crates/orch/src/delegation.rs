use crate::event::{DelEvent, DelegateMode, Effect, Reject, WaitOutcome};
use crate::ids::{TaskId, ThreadId};
use crate::run::{begin_run, restart_attempt};
use crate::slots::Slots;
use crate::state::{
    CohortSt, Delivery, Disp, Hold, Message, Msg, OrchState, Reason, Start, Task, TaskSt, Thread,
    WakeRule, ensure,
};

pub(crate) fn apply(s: &mut OrchState, e: DelEvent, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    match e {
        DelEvent::OpenRoot {
            t,
            access,
            reconfigure,
        } => open_root(s, t, access, reconfigure),
        DelEvent::Stop { t } => stop(s, t, fx),
        DelEvent::NarrowAccess { t, access } => narrow_access(s, t, access, fx),
        DelEvent::InterruptChat { t } => interrupt_chat(s, t, fx),
        DelEvent::Delegate {
            t,
            child,
            mode,
            access,
            reconfigure,
        } => delegate(
            s,
            t,
            Spawn {
                child,
                mode,
                access,
                reconfigure,
            },
            fx,
        ),
        DelEvent::Followup {
            parent,
            child,
            access,
        } => crate::followup::apply(s, parent, child, access),
        DelEvent::ReadTaskResult {
            p,
            c,
            run,
            task_run,
        } => {
            ensure(s.task(c)?.run == task_run, Reject::StaleRun)?;
            read_result(s, p, c, run)
        }
        DelEvent::ReadResult { p, c, run } => {
            ensure(s.task(c)?.run == 1, Reject::StaleRun)?;
            read_result(s, p, c, run)
        }
        DelEvent::WaitReturn { t } => wait_return(s, t, fx),
        DelEvent::WaitTimeout { t } => wait_timeout(s, t, fx),
        DelEvent::WaitAbandon { t } => wait_abandon(s, t),
        DelEvent::LateWaitAck { t } => late_wait_ack(s, t),
        DelEvent::StartFromQueueWake { t } => start_from_queue_wake(s, t, fx),
    }
}

fn open_root(s: &mut OrchState, t: ThreadId, access: u8, reconfigure: bool) -> Result<(), Reject> {
    ensure(!s.threads.contains_key(&t), Reject::ThreadExists)?;
    s.threads
        .insert(t, Thread::new(t, None, 0, access, reconfigure));
    Ok(())
}

fn stop(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    s.thread(t)?;
    s.stops = s.stops.saturating_add(1);
    let id = s.stops;
    thread_stop(s, t, id, fx)
}

pub(crate) fn thread_stop(
    s: &mut OrchState,
    t: ThreadId,
    stop: u32,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let kids = s.kids(t);
    for c in &kids {
        let task = s.task_mut(*c)?;
        if !task.delivery.handled() {
            task.delivery = Delivery::Disposed;
        }
    }
    let background_live = s.natives.iter().any(|((parent, _), native)| {
        *parent == t && native.background && native.status == crate::native::NativeStatus::Running
    });
    let th = s.thread_mut(t)?;
    let live = th.is_live();
    th.mark_interrupt();
    for run in th.runs.iter_mut() {
        run.cohort.disp = Disp::Stopped;
        run.cohort.st = CohortSt::None;
        run.cohort.del.clear();
    }
    th.queue.retain(|m| !matches!(m, Msg::Wake { .. }));
    if live || (background_live && th.rord() > 0) {
        fx.push(Effect::InterruptRun { run: th.run_id() });
    }
    crate::questions::reroute(s, t, true, fx)?;
    for c in kids {
        s.send(Message::Stop { child: c, stop }, fx);
    }
    Ok(())
}

fn narrow_access(
    s: &mut OrchState,
    t: ThreadId,
    access: u8,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    ensure(access < s.thread(t)?.access, Reject::NotNarrower)?;
    let mut targets = Slots::new();
    targets.push(t);
    for d in s.desc(t) {
        targets.push(d);
    }
    for d in targets {
        let th = s.thread_mut(d)?;
        let level = th.access.min(access);
        if level != th.access {
            th.access = level;
            apply_access(th, fx);
        }
    }
    Ok(())
}

fn apply_access(th: &mut Thread, fx: &mut Vec<Effect>) {
    if !th.is_live() {
        return;
    }
    if th.reconfigure {
        fx.push(Effect::Reconfigure {
            t: th.id,
            access: th.access,
        });
    } else if !th.int_req && !th.attempt.emitted {
        restart_attempt(th, Reason::AccessRestart, None, fx);
    }
}

fn interrupt_chat(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.int_req, Reject::InterruptRequested)?;
    th.int_req = true;
    fx.push(Effect::InterruptRun { run: th.run_id() });
    Ok(())
}

struct Spawn {
    child: ThreadId,
    mode: DelegateMode,
    access: u8,
    reconfigure: bool,
}

fn ensure_may_delegate(s: &OrchState, t: ThreadId, sp: &Spawn) -> Result<(), Reject> {
    let th = s.thread(t)?;
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.int_req, Reject::InterruptRequested)?;
    ensure(th.wait_on.is_none(), Reject::Waiting)?;
    ensure(!s.threads.contains_key(&sp.child), Reject::ThreadExists)?;
    ensure(th.depth < s.config.max_depth, Reject::DepthCap)?;
    ensure(s.unsettled_kids(t) < s.config.max_fanout, Reject::FanoutCap)?;
    ensure(sp.access <= th.access, Reject::AccessWider)
}

fn delegate(s: &mut OrchState, t: ThreadId, sp: Spawn, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    ensure_may_delegate(s, t, &sp)?;
    let th = s.thread(t)?;
    let depth = th.depth.saturating_add(1);
    let wake = if sp.mode == DelegateMode::Wait {
        WakeRule::SettledOnly
    } else {
        WakeRule::Always
    };
    let mut c = Thread::new(sp.child, Some(t), depth, sp.access, sp.reconfigure);
    c.queue.push_back(Msg::Task);
    c.task = Some(Task {
        st: TaskSt::Running,
        wake,
        delivery: Delivery::Pending,
        cohort: th.rord(),
        run: 1,
        pushes: 0,
    });
    s.threads.insert(sp.child, c);
    if sp.mode == DelegateMode::Wait {
        s.thread_mut(t)?.wait_on = Some(sp.child);
    }
    fx.push(Effect::CreateChild {
        parent: t,
        child: sp.child,
        depth,
        access: sp.access,
        mode: sp.mode,
    });
    Ok(())
}

fn read_result(s: &mut OrchState, p: ThreadId, c: ThreadId, run: u32) -> Result<(), Reject> {
    let pt = s.thread(p)?;
    ensure(pt.is_live() && !pt.attempt.emitted, Reject::NotLive)?;
    ensure(!crate::questions::blocked(s, p), Reject::Held)?;
    ensure(run == pt.rord(), Reject::StaleRun)?;
    ensure(s.thread(c)?.parent == Some(p), Reject::NotKid)?;
    let task = s.task(c)?;
    ensure(task.st == TaskSt::Settled, Reject::NotSettled)?;
    ensure(task.delivery.unhandled(), Reject::AlreadyHandled)?;
    ack_by(s, c, true)
}

fn ack_by(s: &mut OrchState, c: ThreadId, seen: bool) -> Result<(), Reject> {
    let p = s.thread(c)?.parent.ok_or(Reject::Corrupt)?;
    let task = s.task_mut(c)?;
    task.delivery = Delivery::Acknowledged;
    if seen {
        task.pushes = task.pushes.saturating_add(1);
    }
    let r = task.cohort;
    let pt = s.thread_mut(p)?;
    let co = pt.cohort_mut(r).ok_or(Reject::Corrupt)?;
    let had = co.del.remove(&c);
    let clear = had && co.del.is_empty() && co.st != CohortSt::None;
    if clear {
        co.st = CohortSt::None;
        pt.queue
            .retain(|m| !matches!(m, Msg::Wake { run, .. } if *run == r));
    }
    Ok(())
}

fn waited(s: &OrchState, t: ThreadId) -> Result<(ThreadId, Task), Reject> {
    let th = s.thread(t)?;
    let c = th.wait_on.ok_or(Reject::NotWaiting)?;
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.attempt.emitted, Reject::TooLate)?;
    Ok((c, s.task(c)?))
}

fn wait_return(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    if crate::questions::wait_return(s, t, fx)? {
        return Ok(());
    }
    let (c, task) = waited(s, t)?;
    ensure(task.st == TaskSt::Settled, Reject::NotSettled)?;
    if task.delivery.unhandled() {
        ack_by(s, c, true)?;
    }
    s.thread_mut(t)?.wait_on = None;
    fx.push(Effect::CompleteWait {
        t,
        task: TaskId {
            child: c,
            run: task.run,
        },
        outcome: WaitOutcome::Result,
    });
    Ok(())
}

fn wait_timeout(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let (c, task) = waited(s, t)?;
    ensure(task.st != TaskSt::Settled, Reject::Settled)?;
    s.task_mut(c)?.wake = WakeRule::Always;
    s.thread_mut(t)?.wait_on = None;
    fx.push(Effect::CompleteWait {
        t,
        task: TaskId {
            child: c,
            run: task.run,
        },
        outcome: WaitOutcome::TimedOut,
    });
    Ok(())
}

fn wait_abandon(s: &mut OrchState, t: ThreadId) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.wait_on.is_some(), Reject::NotWaiting)?;
    th.wait_on = None;
    Ok(())
}

fn late_wait_ack(s: &mut OrchState, t: ThreadId) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    ensure(th.late_wait.is_some(), Reject::NoLateWait)?;
    th.late_wait = None;
    Ok(())
}

fn queued_wake(th: &Thread) -> Result<(usize, Msg, u32, u32), Reject> {
    let index=if th.held==Hold::Fail {th.queue.iter().position(|m|matches!(m,Msg::Wake{..})).unwrap_or(0)} else {0};
    let head = *th.queue.get(index).ok_or(Reject::QueueEmpty)?;
    let Msg::Wake { run, generation } = head else {
        return Err(Reject::HeadNotWake);
    };
    ensure(!th.is_live(), Reject::RunLive)?;
    ensure(matches!(th.held, Hold::None | Hold::Fail), Reject::Held)?;
    let co = th.cohort(run).ok_or(Reject::StaleWake)?;
    ensure(
        co.st == CohortSt::Queued && co.generation == generation,
        Reject::StaleWake,
    )?;
    Ok((index, head, run, generation))
}

fn start_from_queue_wake(
    s: &mut OrchState,
    t: ThreadId,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let th = s.thread(t)?;
    let (index, head, r, _) = queued_wake(th)?;
    let co = th.cohort(r).ok_or(Reject::Corrupt)?;
    let open = co.disp == Disp::Open;
    let claimed: Slots<ThreadId> = co
        .del
        .iter()
        .copied()
        .filter(|c| is_claimed(s, *c))
        .collect();
    let pending = s.cohort_pending(t, r);
    let mut delivered = Vec::new();
    for c in &claimed {
        let task = s.task_mut(*c)?;
        task.delivery = Delivery::Delivered;
        task.pushes = task.pushes.saturating_add(1);
        delivered.push(TaskId {
            child: *c,
            run: task.run,
        });
    }
    if open && !pending.is_empty() {
        reoffer(s, t, r, &pending, fx)?;
    } else {
        let co = s.thread_mut(t)?.cohort_mut(r).ok_or(Reject::Corrupt)?;
        co.del.clear();
        co.st = CohortSt::None;
    }
    let th = s.thread_mut(t)?;
    th.queue.remove(index);
    begin_run(th, Start::Queued(head), delivered, fx);
    Ok(())
}

fn is_claimed(s: &OrchState, c: ThreadId) -> bool {
    s.task(c).is_ok_and(|k| k.delivery == Delivery::Claimed)
}

fn reoffer(
    s: &mut OrchState,
    t: ThreadId,
    r: u32,
    pending: &Slots<ThreadId>,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    for c in pending {
        s.task_mut(*c)?.delivery = Delivery::Claimed;
    }
    let co = s.thread_mut(t)?.cohort_mut(r).ok_or(Reject::Corrupt)?;
    co.del = pending.iter().copied().collect();
    co.st = CohortSt::Offered;
    co.generation = co.generation.saturating_add(1);
    let generation = co.generation;
    s.send(
        Message::Offer {
            parent: t,
            run: r,
            generation,
        },
        fx,
    );
    Ok(())
}

pub(crate) fn plan(
    s: &mut OrchState,
    p: ThreadId,
    r: u32,
    set: &Slots<ThreadId>,
    live_now: bool,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let disp = s.thread(p)?.cohort(r).ok_or(Reject::Corrupt)?.disp;
    if disp == Disp::Stopped {
        for c in set {
            s.task_mut(*c)?.delivery = Delivery::Disposed;
        }
        return Ok(());
    }
    let mut claim = Slots::new();
    for c in set {
        if !(s.task(*c)?.wake == WakeRule::SettledOnly && live_now) {
            claim.push(*c);
        }
    }
    if claim.is_empty() {
        return Ok(());
    }
    for c in &claim {
        s.task_mut(*c)?.delivery = Delivery::Claimed;
    }
    offer_claimed(s, p, r, &claim, fx)
}

fn offer_claimed(
    s: &mut OrchState,
    p: ThreadId,
    r: u32,
    claim: &Slots<ThreadId>,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let co = s.thread_mut(p)?.cohort_mut(r).ok_or(Reject::Corrupt)?;
    let was = co.st;
    co.del.extend(claim.iter().copied());
    if was == CohortSt::None {
        co.generation = co.generation.saturating_add(1);
        co.st = CohortSt::Offered;
    }
    let generation = co.generation;
    if was != CohortSt::Queued {
        s.send(
            Message::Offer {
                parent: p,
                run: r,
                generation,
            },
            fx,
        );
    }
    Ok(())
}

pub(crate) fn end_run(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    let th = s.thread_mut(t)?;
    let r = th.rord();
    if let Some(c) = th.wait_on.take() {
        th.late_wait = Some(c);
    }
    if th.is_task_run() {
        s.send(Message::Notice { child: t, run: r }, fx);
    }
    let sweep = s.cohort_pending(t, r);
    if sweep.is_empty() {
        return Ok(());
    }
    plan(s, t, r, &sweep, false, fx)
}

#[cfg(kani)]
pub(crate) fn kani_stop(
    s: &mut OrchState,
    t: ThreadId,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    stop(s, t, fx)
}

#[cfg(kani)]
pub(crate) fn kani_narrow_access(
    s: &mut OrchState,
    t: ThreadId,
    access: u8,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    narrow_access(s, t, access, fx)
}

#[cfg(kani)]
pub(crate) fn kani_delegate(
    s: &mut OrchState,
    t: ThreadId,
    child: ThreadId,
    mode: DelegateMode,
    access: u8,
    reconfigure: bool,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let sp = Spawn {
        child,
        mode,
        access,
        reconfigure,
    };
    delegate(s, t, sp, fx)
}

#[cfg(kani)]
pub(crate) fn kani_start_from_queue_wake(
    s: &mut OrchState,
    t: ThreadId,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    start_from_queue_wake(s, t, fx)
}
