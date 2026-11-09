use crate::ids::ThreadId;
use crate::state::{
    Delivery, Message, Msg, OrchState, RunStatus, Task, TaskSt, Thread, WakeRule,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    S1AtMostOneActiveRun(ThreadId),
    S3ExactlyOnce(ThreadId),
    S4NoWakeIntoLiveRun(ThreadId),
    S5aDepth(ThreadId),
    S5aFanout(ThreadId),
    S6AccessNarrow(ThreadId),
    S7AckFromLiveRun(ThreadId),
}

pub fn check_invariants(s: &OrchState) -> Result<(), Violation> {
    for th in s.threads.values() {
        s1(th)?;
        s5a(s, th)?;
        if let Some(task) = th.task {
            s3(th.id, task)?;
            s4(s, th, task)?;
            s6(s, th)?;
            s7(th.id, task)?;
        }
    }
    Ok(())
}

pub fn all_violations(s: &OrchState) -> Vec<Violation> {
    let mut out = Vec::new();
    for th in s.threads.values() {
        out.extend(s1(th).err());
        out.extend(s5a(s, th).err());
        if let Some(task) = th.task {
            out.extend(s3(th.id, task).err());
            out.extend(s4(s, th, task).err());
            out.extend(s6(s, th).err());
            out.extend(s7(th.id, task).err());
        }
    }
    out
}

fn fail(ok: bool, v: Violation) -> Result<(), Violation> {
    if ok { Ok(()) } else { Err(v) }
}

fn s1(th: &Thread) -> Result<(), Violation> {
    fail(th.live_count() <= 1, Violation::S1AtMostOneActiveRun(th.id))
}

fn s3(c: ThreadId, task: Task) -> Result<(), Violation> {
    let reached = matches!(task.delivery, Delivery::Delivered | Delivery::Acknowledged);
    fail(
        task.pushes <= 1 && (task.pushes != 1 || reached),
        Violation::S3ExactlyOnce(c),
    )
}

fn s4(s: &OrchState, th: &Thread, task: Task) -> Result<(), Violation> {
    let claimed_only = task.wake == WakeRule::SettledOnly && task.delivery == Delivery::Claimed;
    let live = th.parent.is_some_and(|p| s.cohort_live(p, task.cohort));
    fail(
        !(claimed_only && live),
        Violation::S4NoWakeIntoLiveRun(th.id),
    )
}

fn s5a(s: &OrchState, th: &Thread) -> Result<(), Violation> {
    fail(th.depth <= s.config.max_depth, Violation::S5aDepth(th.id))?;
    fail(
        s.unsettled_kids(th.id) <= s.config.max_fanout,
        Violation::S5aFanout(th.id),
    )
}

fn s6(s: &OrchState, th: &Thread) -> Result<(), Violation> {
    let parent = th.parent.and_then(|p| s.threads.get(&p));
    fail(
        parent.is_none_or(|p| th.access <= p.access),
        Violation::S6AccessNarrow(th.id),
    )
}

fn s7(c: ThreadId, task: Task) -> Result<(), Violation> {
    let reached = matches!(task.delivery, Delivery::Delivered | Delivery::Acknowledged);
    fail(!reached || task.pushes == 1, Violation::S7AckFromLiveRun(c))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    Key(ThreadId),
    Lineage(ThreadId),
    Depth(ThreadId),
    LiveNotLatest(ThreadId),
    Attempt(ThreadId),
    Inbox(ThreadId),
    Timer(ThreadId),
    Wait(ThreadId),
    TaskCohort(ThreadId),
    Claim(ThreadId),
    CohortMember(ThreadId),
    QueueWake(ThreadId),
    Outbox,
    Receipt,
}

pub fn well_formed(s: &OrchState) -> Result<(), Malformed> {
    for (k, th) in &s.threads {
        wf_ok(*k == th.id, Malformed::Key(th.id))?;
        wf_lineage(s, th)?;
        wf_runs(th)?;
        wf_waits(s, th)?;
        wf_task(s, th)?;
        wf_cohorts(s, th)?;
    }
    for m in &s.outbox {
        wf_ok(message_ok(s, *m), Malformed::Outbox)?;
    }
    for (c, _) in &s.stop_receipts {
        wf_ok(s.threads.contains_key(c), Malformed::Receipt)?;
    }
    Ok(())
}

fn wf_ok(ok: bool, m: Malformed) -> Result<(), Malformed> {
    if ok { Ok(()) } else { Err(m) }
}

fn wf_lineage(s: &OrchState, th: &Thread) -> Result<(), Malformed> {
    wf_ok(
        th.parent.is_some() == th.task.is_some(),
        Malformed::Lineage(th.id),
    )?;
    match th.parent {
        None => wf_ok(th.depth == 0, Malformed::Depth(th.id)),
        Some(p) => {
            let pt = s.threads.get(&p).ok_or(Malformed::Lineage(th.id))?;
            wf_ok(
                u16::from(pt.depth) + 1 == u16::from(th.depth),
                Malformed::Depth(th.id),
            )
        }
    }
}

fn wf_runs(th: &Thread) -> Result<(), Malformed> {
    let rord = th.rord();
    let live_ok = th
        .runs
        .iter()
        .enumerate()
        .all(|(i, r)| r.status != RunStatus::Live || i + 1 == th.runs.len());
    wf_ok(live_ok, Malformed::LiveNotLatest(th.id))?;
    wf_ok(
        (rord == 0) == (th.attempt.n == 0),
        Malformed::Attempt(th.id),
    )?;
    wf_ok(
        th.inbox
            .iter()
            .all(|e| e.run >= 1 && e.run <= rord && (e.run < rord || e.attempt <= th.attempt.n)),
        Malformed::Inbox(th.id),
    )?;
    wf_ok(
        th.ul.armed.is_none_or(|r| r >= 1 && r <= rord),
        Malformed::Timer(th.id),
    )
}

fn wf_waits(s: &OrchState, th: &Thread) -> Result<(), Malformed> {
    let kid = |c: ThreadId| s.threads.get(&c).is_some_and(|k| k.parent == Some(th.id));
    wf_ok(
        th.wait_on.is_none_or(kid) && th.late_wait.is_none_or(kid),
        Malformed::Wait(th.id),
    )?;
    wf_ok(th.wait_on.is_none() || th.is_live(), Malformed::Wait(th.id))
}

fn wf_task(s: &OrchState, th: &Thread) -> Result<(), Malformed> {
    let (Some(task), Some(p)) = (th.task, th.parent) else {
        return Ok(());
    };
    let pt = s.threads.get(&p).ok_or(Malformed::Lineage(th.id))?;
    if pt.retired(task.cohort) {
        let quiet = task.st == TaskSt::Settled && task.delivery.handled();
        return wf_ok(quiet, Malformed::TaskCohort(th.id));
    }
    let co = pt.cohort(task.cohort).ok_or(Malformed::TaskCohort(th.id))?;
    let claimed = task.delivery == Delivery::Claimed;
    wf_ok(claimed == co.del.contains(&th.id), Malformed::Claim(th.id))
}

fn wf_cohorts(s: &OrchState, th: &Thread) -> Result<(), Malformed> {
    for (i, run) in th.runs.iter().enumerate() {
        let r = th.ord_at(i);
        let members_ok = run.cohort.del.iter().all(|c| {
            s.threads
                .get(c)
                .is_some_and(|k| k.parent == Some(th.id) && k.task.is_some_and(|t| t.cohort == r))
        });
        wf_ok(members_ok, Malformed::CohortMember(th.id))?;
    }
    let wakes_ok = th.queue.iter().all(|m| match m {
        Msg::Wake { run, .. } => th.cohort(*run).is_some(),
        _ => true,
    });
    wf_ok(wakes_ok, Malformed::QueueWake(th.id))
}

fn message_ok(s: &OrchState, m: Message) -> bool {
    match m {
        Message::Notice { child, run } => s
            .threads
            .get(&child)
            .is_some_and(|c| c.task.is_some() && run >= 1 && run <= c.rord()),
        Message::Offer { parent, run, .. } => s
            .threads
            .get(&parent)
            .is_some_and(|p| p.cohort(run).is_some()),
        Message::Stop { child, .. } => s.threads.get(&child).is_some_and(|c| c.parent.is_some()),
    }
}
