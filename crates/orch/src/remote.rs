use crate::state::ensure;
use crate::{
    Cohort, Delivery, Effect, How, Message, Msg, OrchState, OutboxEvent, Reject, Run, RunStatus,
    Start, Task, TaskSt, Thread, ThreadId, WakeRule,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum RemoteEvent {
    RemoteFollowup {
        child: ThreadId,
        run: u32,
        cohort: u32,
        access: u8,
    },
    RemoteImport {
        parent: ThreadId,
        child: ThreadId,
        ancestor: Option<ThreadId>,
        depth: u8,
        access: u8,
        cohort: u32,
    },
    RemoteNotice {
        child: ThreadId,
        run: u32,
        status: RunStatus,
    },
    RemoteSent {
        message: Message,
    },
    RemoteStop {
        child: ThreadId,
        stop: u32,
    },
    RemoteFence {
        t: ThreadId,
    },
    RemoteClamp {
        child: ThreadId,
        access: u8,
    },
    RemoteCancel {
        child: ThreadId,
    },
}
impl RemoteEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RemoteFollowup { .. } => "RemoteFollowup",
            Self::RemoteImport { .. } => "RemoteImport",
            Self::RemoteNotice { .. } => "RemoteNotice",
            Self::RemoteSent { .. } => "RemoteSent",
            Self::RemoteStop { .. } => "RemoteStop",
            Self::RemoteFence { .. } => "RemoteFence",
            Self::RemoteClamp { .. } => "RemoteClamp",
            Self::RemoteCancel { .. } => "RemoteCancel",
        }
    }
}
fn stub(id: ThreadId, parent: Option<ThreadId>, depth: u8, cohort: u32) -> Thread {
    let mut th = Thread::new(id, parent, depth, 2, false);
    th.run_base = cohort.saturating_sub(1);
    th.runs.push(Run {
        start: Start::Queued(Msg::Task),
        status: RunStatus::Completed,
        cohort: Cohort::default(),
    });
    th.attempt.n = 1;
    if parent.is_some() {
        th.task = Some(Task {
            st: TaskSt::Settled,
            wake: WakeRule::Always,
            delivery: Delivery::Disposed,
            cohort: 1,
            run: cohort,
            pushes: 0,
        });
    }
    th
}
pub(crate) fn apply(s: &mut OrchState, e: RemoteEvent, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    match e {
        RemoteEvent::RemoteFollowup {
            child,
            run,
            cohort,
            access,
        } => {
            let th = s.thread(child)?;
            ensure(!th.is_live() && th.queue.is_empty(), Reject::RunLive)?;
            ensure(th.held == crate::Hold::None && !th.int_req, Reject::Held)?;
            ensure(!crate::questions::blocked(s, child), Reject::Held)?;
            ensure(th.rord().checked_add(1) == Some(run), Reject::StaleRun)?;
            ensure(access <= th.access, Reject::AccessWider)?;
            let task = s.task(child)?;
            ensure(
                task.st == TaskSt::Settled && task.delivery.handled(),
                Reject::NotSettled,
            )?;
            let parent = th.parent.ok_or(Reject::Corrupt)?;
            let pt = s.thread_mut(parent)?;
            ensure(cohort > 0 && !pt.retired(cohort), Reject::StaleRun)?;
            while pt.rord() < cohort {
                pt.runs.push(Run {
                    start: Start::Queued(Msg::Task),
                    status: RunStatus::Completed,
                    cohort: Cohort::default(),
                });
            }
            let th = s.thread_mut(child)?;
            th.access = access;
            th.queue.push_back(Msg::Task);
            th.task = Some(Task {
                st: TaskSt::Running,
                wake: WakeRule::Always,
                delivery: Delivery::Pending,
                cohort,
                run,
                pushes: 0,
            });
            Ok(())
        }
        RemoteEvent::RemoteImport {
            parent,
            child,
            ancestor,
            depth,
            access,
            cohort,
        } => {
            ensure(!s.threads.contains_key(&child), Reject::ThreadExists)?;
            ensure(
                (1..=s.config.max_depth).contains(&depth) && depth <= 2 && cohort > 0,
                Reject::DepthCap,
            )?;
            ensure(access <= 2, Reject::AccessWider)?;
            if depth == 2 {
                let a = ancestor.ok_or(Reject::Corrupt)?;
                ensure(
                    a != parent && a != child && parent != child,
                    Reject::Corrupt,
                )?;
                if !s.threads.contains_key(&a) {
                    s.threads.insert(a, stub(a, None, 0, 1));
                }
            } else {
                ensure(ancestor.is_none() && parent != child, Reject::Corrupt)?;
            }
            if !s.threads.contains_key(&parent) {
                s.threads
                    .insert(parent, stub(parent, ancestor, depth - 1, cohort));
            }
            ensure(s.thread(parent)?.depth + 1 == depth, Reject::Corrupt)?;
            let pt = s.thread_mut(parent)?;
            ensure(!pt.retired(cohort), Reject::StaleRun)?;
            while pt.rord() < cohort {
                pt.runs.push(Run {
                    start: Start::Queued(Msg::Task),
                    status: RunStatus::Completed,
                    cohort: Cohort::default(),
                });
            }
            let mut th = Thread::new(child, Some(parent), depth, access, false);
            th.queue.push_back(Msg::Task);
            th.task = Some(Task {
                st: TaskSt::Running,
                wake: WakeRule::Always,
                delivery: Delivery::Pending,
                cohort,
                run: 1,
                pushes: 0,
            });
            s.threads.insert(child, th);
            Ok(())
        }
        RemoteEvent::RemoteNotice { child, run, status } => {
            ensure(status != RunStatus::Live, Reject::NotSettled)?;
            ensure(s.task(child)?.run == run, Reject::StaleRun)?;
            if s.task(child)?.st == TaskSt::Settled {
                return Ok(());
            }
            let th = s.thread_mut(child)?;
            ensure(!th.is_live(), Reject::RunLive)?;
            th.queue.clear();
            th.run_base = run - 1;
            th.runs.clear();
            th.runs.push(Run {
                start: Start::Queued(Msg::Task),
                status,
                cohort: Cohort::default(),
            });
            th.attempt.n = 1;
            s.outbox.insert(Message::Notice { child, run });
            crate::outbox::apply(
                s,
                OutboxEvent::DoNotice {
                    child,
                    run,
                    how: How::Ack,
                },
                fx,
            )
        }
        RemoteEvent::RemoteClamp { child, access } => {
            ensure(s.thread(child)?.access == access, Reject::NotNarrower)?;
            fx.push(Effect::Reconfigure { t: child, access });
            Ok(())
        }
        RemoteEvent::RemoteCancel { child } => {
            s.stops = s.stops.saturating_add(1);
            let stop = s.stops;
            crate::delegation::thread_stop(s, child, stop, fx)?;
            s.send(Message::Stop { child, stop }, fx);
            Ok(())
        }
        RemoteEvent::RemoteStop { child, stop } => {
            s.thread(child)?;
            if !s.stop_receipts.insert((child, stop)) {
                return Ok(());
            }
            s.stops = s.stops.max(stop).saturating_add(1);
            crate::delegation::thread_stop(s, child, s.stops, fx)
        }
        RemoteEvent::RemoteFence { t } => {
            s.thread(t)?;
            Ok(())
        }
        RemoteEvent::RemoteSent { message } => {
            s.outbox.remove(&message);
            if let Message::Notice { child, run } = message {
                let task = s.task_mut(child)?;
                if task.run == run {
                    task.st = TaskSt::Settled;
                    task.delivery = Delivery::Disposed;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Event, check_invariants, step, well_formed};
    #[test]
    fn imports_distinct_parent_cohorts_and_grandchild_depth() {
        let mut s = OrchState::new(Config::default());
        for (child, cohort) in [(ThreadId(3), 1), (ThreadId(4), 3)] {
            s = step(
                &s,
                Event::Remote(RemoteEvent::RemoteImport {
                    parent: ThreadId(2),
                    child,
                    ancestor: Some(ThreadId(1)),
                    depth: 2,
                    access: 1,
                    cohort,
                }),
            )
            .unwrap()
            .0;
            well_formed(&s).unwrap();
            check_invariants(&s).unwrap();
        }
        assert_eq!(s.thread(ThreadId(2)).unwrap().rord(), 3);
        assert_eq!(s.thread(ThreadId(3)).unwrap().task.unwrap().cohort, 1);
        assert!(s.thread(ThreadId(2)).unwrap().queue.is_empty());
    }
    #[test]
    fn received_stop_allocates_a_fresh_local_cascade_epoch() {
        let mut s = OrchState::new(Config::default());
        s = step(
            &s,
            Event::Remote(RemoteEvent::RemoteImport {
                parent: ThreadId(1),
                child: ThreadId(2),
                ancestor: None,
                depth: 1,
                access: 2,
                cohort: 1,
            }),
        )
        .unwrap()
        .0;
        s = step(&s, Event::Del(crate::DelEvent::Stop { t: ThreadId(2) }))
            .unwrap()
            .0;
        assert_eq!(s.stops, 1);
        s = step(
            &s,
            Event::Remote(RemoteEvent::RemoteStop {
                child: ThreadId(2),
                stop: 1,
            }),
        )
        .unwrap()
        .0;
        assert_eq!(s.stops, 2);
        let again = step(
            &s,
            Event::Remote(RemoteEvent::RemoteStop {
                child: ThreadId(2),
                stop: 1,
            }),
        )
        .unwrap()
        .0;
        assert_eq!(again.stops, 2);
    }
}
