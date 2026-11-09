use crate::state::ensure;
use crate::{Delivery, Hold, Msg, OrchState, Reject, Task, TaskSt, ThreadId, WakeRule};

pub(crate) fn apply(
    s: &mut OrchState,
    parent: ThreadId,
    child: ThreadId,
    access: u8,
) -> Result<(), Reject> {
    let p = s.thread(parent)?;
    ensure(p.is_live() && !p.attempt.emitted, Reject::NotLive)?;
    ensure(
        !p.int_req && p.held == Hold::None,
        Reject::InterruptRequested,
    )?;
    ensure(p.wait_on.is_none(), Reject::Waiting)?;
    ensure(!crate::questions::blocked(s, parent), Reject::Held)?;
    ensure(
        s.unsettled_kids(parent) < s.config.max_fanout,
        Reject::FanoutCap,
    )?;
    ensure(access <= p.access, Reject::AccessWider)?;
    let cohort = p.rord();
    let c = s.thread(child)?;
    ensure(c.parent == Some(parent), Reject::NotKid)?;
    ensure(c.depth <= s.config.max_depth, Reject::DepthCap)?;
    ensure(!c.is_live(), Reject::RunLive)?;
    ensure(c.held == Hold::None && !c.int_req, Reject::Held)?;
    ensure(c.queue.is_empty(), Reject::QueueFull)?;
    ensure(!crate::questions::blocked(s, child), Reject::Held)?;
    let old = s.task(child)?;
    ensure(old.st == TaskSt::Settled, Reject::NotSettled)?;
    ensure(old.delivery.handled(), Reject::AlreadyHandled)?;
    ensure(access <= c.access, Reject::AccessWider)?;
    let next = c.rord().checked_add(1).ok_or(Reject::Corrupt)?;
    let c = s.thread_mut(child)?;
    c.access = access;
    c.queue.push_back(Msg::Task);
    c.task = Some(Task {
        st: TaskSt::Running,
        wake: WakeRule::Always,
        delivery: Delivery::Pending,
        cohort,
        run: next,
        pushes: 0,
    });
    Ok(())
}
