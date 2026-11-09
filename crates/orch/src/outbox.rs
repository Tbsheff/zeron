use crate::delegation::{plan, thread_stop};
use crate::event::{Effect, How, OutboxEvent, Reject};
use crate::ids::ThreadId;
use crate::slots::Slots;
use crate::state::{CohortSt, Delivery, Message, Msg, OrchState, TaskSt, ensure};

pub(crate) fn apply(s: &mut OrchState, e: OutboxEvent, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    match e {
        OutboxEvent::DoNotice { child, run, how } => do_notice(s, child, run, how, fx),
        OutboxEvent::DoOffer {
            parent,
            run,
            generation,
            how,
        } => do_offer(s, parent, run, generation, how),
        OutboxEvent::DoStop { child, stop, how } => do_stop(s, child, stop, how, fx),
        OutboxEvent::CrashEngine => {
            crash_engine(s);
            Ok(())
        }
    }
}

fn take(s: &mut OrchState, m: Message, how: How) -> Result<(), Reject> {
    ensure(s.outbox.contains(&m), Reject::NotInOutbox)?;
    if how == How::Ack {
        s.outbox.remove(&m);
    }
    Ok(())
}

fn do_notice(
    s: &mut OrchState,
    c: ThreadId,
    run: u32,
    how: How,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    take(s, Message::Notice { child: c, run }, how)?;
    let task = s.task(c)?;
    if task.st == TaskSt::Settled || task.run != run {
        return Ok(());
    }
    let p = s.thread(c)?.parent.ok_or(Reject::Corrupt)?;
    s.task_mut(c)?.st = TaskSt::Settled;
    let mut set = Slots::new();
    if task.delivery == Delivery::Pending {
        set.push(c);
    }
    let live_now = s.cohort_live(p, task.cohort);
    plan(s, p, task.cohort, &set, live_now, fx)
}

fn do_offer(
    s: &mut OrchState,
    p: ThreadId,
    run: u32,
    generation: u32,
    how: How,
) -> Result<(), Reject> {
    take(
        s,
        Message::Offer {
            parent: p,
            run,
            generation,
        },
        how,
    )?;
    let th = s.thread_mut(p)?;
    let co = th.cohort_mut(run).ok_or(Reject::Corrupt)?;
    if co.st == CohortSt::Offered && co.generation == generation {
        co.st = CohortSt::Queued;
        th.queue.push_back(Msg::Wake { run, generation });
    }
    Ok(())
}

fn do_stop(
    s: &mut OrchState,
    c: ThreadId,
    stop: u32,
    how: How,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    take(s, Message::Stop { child: c, stop }, how)?;
    if !s.stop_receipts.insert((c, stop)) {
        return Ok(());
    }
    thread_stop(s, c, stop, fx)
}

fn crash_engine(s: &mut OrchState) {
    for th in s.threads.values_mut() {
        th.wait_on = None;
        th.late_wait = None;
    }
}

#[cfg(kani)]
pub(crate) fn kani_do_notice(
    s: &mut OrchState,
    c: ThreadId,
    run: u32,
    how: How,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    do_notice(s, c, run, how, fx)
}
