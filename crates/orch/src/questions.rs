use crate::state::{Hold, ensure};
use crate::{Effect, How, Message, OrchState, Reject, Start, ThreadId, VecSet, WakeRule};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Question {
    pub child: ThreadId,
    pub qid: u64,
    pub epoch: u32,
    pub pending: bool,
    pub holder: Option<ThreadId>,
    pub at_person: bool,
    pub views: VecSet<ThreadId>,
    pub waiting: VecSet<ThreadId>,
    pub seen: VecSet<ThreadId>,
    #[serde(default)]
    pub future_answers: crate::VecMap<u32, u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum QuestionEvent {
    Raise {
        child: ThreadId,
        qid: u64,
    },
    ImportUp {
        child: ThreadId,
        qid: u64,
        epoch: u32,
        to: ThreadId,
    },
    ImportAnswer {
        child: ThreadId,
        qid: u64,
        epoch: u32,
        via: ThreadId,
        answer: u64,
    },
    ImportClosed {
        child: ThreadId,
        qid: u64,
        epoch: u32,
        to: ThreadId,
    },
    ImportRelease {
        child: ThreadId,
        qid: u64,
        epoch: u32,
        via: ThreadId,
    },
    Deliver {
        message: Message,
        how: How,
    },
    Start {
        t: ThreadId,
    },
    AgentAnswer {
        agent: ThreadId,
        child: ThreadId,
        qid: u64,
        epoch: u32,
        answer: u64,
    },
    PersonAnswer {
        child: ThreadId,
        qid: u64,
        epoch: u32,
        answer: u64,
    },
}
impl QuestionEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ImportUp { .. } => "ImportQuestion",
            Self::ImportAnswer { .. } => "ImportAnswer",
            Self::ImportClosed { .. } => "ImportClosed",
            Self::ImportRelease { .. } => "ImportRelease",
            Self::Raise { .. } => "RaiseQuestion",
            Self::Deliver { .. } => "DeliverQuestion",
            Self::Start { .. } => "StartQuestion",
            Self::AgentAnswer { .. } => "AgentAnswer",
            Self::PersonAnswer { .. } => "PersonAnswer",
        }
    }
}

pub fn blocked(s: &OrchState, t: ThreadId) -> bool {
    s.questions.values().any(|q| q.child == t && q.pending)
}

pub(crate) fn apply(
    s: &mut OrchState,
    e: QuestionEvent,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    match e {
        QuestionEvent::ImportUp {
            child,
            qid,
            epoch,
            to,
        } => {
            s.thread(child)?;
            s.thread(to)?;
            if let Some(q) = s.questions.get_mut(&(child, qid)) {
                if !q.pending || epoch < q.epoch {
                    return Ok(());
                }
                q.epoch = epoch;
            } else {
                s.questions.insert(
                    (child, qid),
                    Question {
                        child,
                        qid,
                        epoch,
                        pending: true,
                        holder: None,
                        at_person: false,
                        views: VecSet::new(),
                        waiting: VecSet::new(),
                        seen: VecSet::new(),
                        future_answers: crate::VecMap::new(),
                    },
                );
            }
            let message = Message::QuestionUp {
                child,
                qid,
                epoch,
                to: Some(to),
            };
            s.outbox.insert(message);
            deliver(s, message, How::Ack, fx)
        }
        QuestionEvent::ImportAnswer {
            child,
            qid,
            epoch,
            via,
            answer,
        } => {
            let message = Message::AnswerDown {
                child,
                qid,
                epoch,
                via,
                answer,
            };
            s.outbox.insert(message);
            deliver(s, message, How::Ack, fx)
        }
        QuestionEvent::ImportClosed {
            child,
            qid,
            epoch,
            to,
        } => {
            let epoch = if let Some(q) = s.questions.get_mut(&(child, qid)) {
                q.pending = false;
                q.holder = None;
                q.at_person = false;
                q.epoch = q.epoch.max(epoch);
                q.epoch
            } else {
                epoch
            };
            let message = Message::QuestionClosed {
                child,
                qid,
                epoch,
                to,
            };
            s.outbox.insert(message);
            deliver(s, message, How::Ack, fx)?;
            if let Some(parent) = s.thread(to)?.parent {
                s.send(
                    Message::QuestionClosed {
                        child,
                        qid,
                        epoch,
                        to: parent,
                    },
                    fx,
                );
            }
            Ok(())
        }
        QuestionEvent::ImportRelease {
            child,
            qid,
            epoch,
            via,
        } => {
            let message = Message::QuestionRelease {
                child,
                qid,
                epoch,
                via,
            };
            s.outbox.insert(message);
            deliver(s, message, How::Ack, fx)
        }
        QuestionEvent::Raise { child, qid } => {
            let th = s.thread(child)?;
            ensure(th.is_live(), Reject::NotLive)?;

            ensure(th.wait_on.is_none(), Reject::Waiting)?;
            ensure(
                th.held != Hold::Stop && !th.int_req,
                Reject::InterruptRequested,
            )?;
            if s.questions.contains_key(&(child, qid)) {
                return Ok(());
            }
            ensure(!blocked(s, child), Reject::Waiting)?;
            let to = th.parent;
            s.questions.insert(
                (child, qid),
                Question {
                    child,
                    qid,
                    epoch: 1,
                    pending: true,
                    holder: None,
                    at_person: false,
                    views: VecSet::new(),
                    waiting: VecSet::new(),
                    seen: VecSet::new(),
                    future_answers: crate::VecMap::new(),
                },
            );
            s.send(
                Message::QuestionUp {
                    child,
                    qid,
                    epoch: 1,
                    to,
                },
                fx,
            );
            reroute(s, child, true, fx)?;
            Ok(())
        }
        QuestionEvent::Deliver { message, how } => deliver(s, message, how, fx),
        QuestionEvent::Start { t } => {
            let th = s.thread(t)?;
            ensure(!th.is_live(), Reject::RunLive)?;
            ensure(th.held == Hold::None, Reject::Held)?;
            ensure(!blocked(s, t), Reject::Waiting)?;
            let keys: Vec<_> = s
                .questions
                .iter()
                .filter(|(_, q)| q.pending && q.holder == Some(t) && q.views.contains(&t))
                .map(|(k, _)| *k)
                .collect();
            ensure(!keys.is_empty(), Reject::QueueEmpty)?;
            for k in keys {
                let q = s.questions.get_mut(&k).unwrap();
                q.seen.insert(t);
                view(q, t, false, fx);
            }
            crate::run::begin_run(s.thread_mut(t)?, Start::Questions, Vec::new(), fx);
            Ok(())
        }
        QuestionEvent::AgentAnswer {
            agent,
            child,
            qid,
            epoch,
            answer,
        } => {
            let th = s.thread(agent)?;
            ensure(th.is_live(), Reject::NotLive)?;
            ensure(!blocked(s, agent), Reject::Waiting)?;
            let Some(q) = s.questions.get(&(child, qid)) else {
                return Ok(());
            };
            if !q.pending || q.epoch != epoch {
                return Ok(());
            }
            ensure(
                q.holder == Some(agent) && q.seen.contains(&agent),
                Reject::NotCurrent,
            )?;
            s.send(
                Message::AnswerDown {
                    child,
                    qid,
                    epoch,
                    via: agent,
                    answer,
                },
                fx,
            );
            Ok(())
        }
        QuestionEvent::PersonAnswer {
            child,
            qid,
            epoch,
            answer,
        } => close(s, child, qid, epoch, answer, fx),
    }
}
fn view(q: &Question, viewer: ThreadId, retract: bool, fx: &mut Vec<Effect>) {
    fx.push(Effect::QuestionView {
        viewer,
        child: q.child,
        qid: q.qid,
        epoch: q.epoch,
        retract,
    });
}
fn deliver(s: &mut OrchState, m: Message, how: How, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    ensure(s.outbox.contains(&m), Reject::NotInOutbox)?;
    if how == How::Ack {
        s.outbox.remove(&m);
    }
    if !s.question_receipts.insert(m) {
        return Ok(());
    }
    match m {
        Message::QuestionUp {
            child,
            qid,
            epoch,
            to,
        } => {
            let Some(q) = s.questions.get(&(child, qid)) else {
                return Ok(());
            };
            if !q.pending || q.epoch != epoch {
                return Ok(());
            }
            if let Some(p) = to {
                let th = s.thread(p)?;
                let wait = th.is_live() && th.wait_on == Some(child);
                let parent = th.parent;
                if blocked(s, p) || th.held != Hold::None {
                    s.send(
                        Message::QuestionUp {
                            child,
                            qid,
                            epoch,
                            to: parent,
                        },
                        fx,
                    );
                } else {
                    let q = s.questions.get_mut(&(child, qid)).unwrap();
                    q.holder = Some(p);
                    q.views.insert(p);
                    if wait {
                        q.waiting.insert(p);
                    } else {
                        view(q, p, false, fx);
                    }
                }
            } else {
                let q = s.questions.get_mut(&(child, qid)).unwrap();
                q.at_person = true;
                q.holder = None;
                fx.push(Effect::NotifyQuestion { child, qid, epoch });
            }
        }
        Message::AnswerDown {
            child,
            qid,
            epoch,
            via,
            answer,
        } => {
            let Some(q) = s.questions.get(&(child, qid)) else {
                return Ok(());
            };
            if !q.pending || q.epoch > epoch {
                return Ok(());
            }
            if q.epoch < epoch {
                let q = s.questions.get_mut(&(child, qid)).unwrap();
                if !q.future_answers.contains_key(&epoch) {
                    q.future_answers.insert(epoch, answer);
                }
                return Ok(());
            }
            let mut hop = child;
            while let Some(p) = s.thread(hop)?.parent {
                if p == via {
                    break;
                }
                hop = p;
            }
            if hop == child {
                close(s, child, qid, epoch, answer, fx)?;
            } else {
                s.send(
                    Message::AnswerDown {
                        child,
                        qid,
                        epoch,
                        via: hop,
                        answer,
                    },
                    fx,
                );
            }
        }
        Message::QuestionRelease {
            child,
            qid,
            epoch,
            via,
        } => {
            let Some(q) = s.questions.get_mut(&(child, qid)) else {
                return Ok(());
            };
            if !q.pending || epoch < q.epoch {
                return Ok(());
            }
            q.epoch = epoch;
            let mut hop = child;
            while let Some(p) = s.thread(hop)?.parent {
                if p == via {
                    break;
                }
                hop = p;
            }
            let buffered = s
                .questions
                .get_mut(&(child, qid))
                .unwrap()
                .future_answers
                .remove(&epoch);
            if hop != child {
                s.send(
                    Message::QuestionRelease {
                        child,
                        qid,
                        epoch,
                        via: hop,
                    },
                    fx,
                );
                if let Some(answer) = buffered {
                    s.send(
                        Message::AnswerDown {
                            child,
                            qid,
                            epoch,
                            via: hop,
                            answer,
                        },
                        fx,
                    );
                }
            } else if let Some(answer) = buffered {
                close(s, child, qid, epoch, answer, fx)?;
            }
        }
        Message::QuestionClosed {
            child,
            qid,
            epoch,
            to,
        } => {
            if let Some(q) = s.questions.get_mut(&(child, qid)) {
                if q.epoch == epoch {
                    q.views.remove(&to);
                    q.waiting.remove(&to);
                    q.seen.remove(&to);
                    view(q, to, true, fx);
                }
            }
        }
        _ => return Err(Reject::Corrupt),
    }
    Ok(())
}
fn close(
    s: &mut OrchState,
    child: ThreadId,
    qid: u64,
    epoch: u32,
    answer: u64,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let Some(q) = s.questions.get_mut(&(child, qid)) else {
        return Ok(());
    };
    if !q.pending || q.epoch != epoch {
        return Ok(());
    }
    q.pending = false;
    q.holder = None;
    q.at_person = false;
    let mut viewers = q.views.clone();
    let mut parent = s.thread(child)?.parent;
    while let Some(p) = parent {
        viewers.insert(p);
        parent = s.thread(p)?.parent;
    }
    fx.push(Effect::RespondInput {
        child,
        qid,
        epoch,
        answer,
    });
    for to in &viewers {
        s.send(
            Message::QuestionClosed {
                child,
                qid,
                epoch,
                to: *to,
            },
            fx,
        );
    }
    Ok(())
}

pub(crate) fn wait_return(
    s: &mut OrchState,
    t: ThreadId,
    fx: &mut Vec<Effect>,
) -> Result<bool, Reject> {
    let th = s.thread(t)?;
    let Some(c) = th.wait_on else {
        return Ok(false);
    };
    ensure(th.is_live(), Reject::NotLive)?;
    ensure(!th.attempt.emitted, Reject::TooLate)?;
    let keys: Vec<_> = s
        .questions
        .iter()
        .filter(|(_, q)| q.child == c && q.pending && q.holder == Some(t))
        .map(|(k, _)| *k)
        .collect();
    if keys.is_empty() {
        return Ok(false);
    }
    let task = s.task(c)?;
    s.task_mut(c)?.wake = WakeRule::Always;
    s.thread_mut(t)?.wait_on = None;
    for k in keys {
        let q = s.questions.get_mut(&k).unwrap();
        q.waiting.remove(&t);
        q.seen.insert(t);
        view(q, t, false, fx);
    }
    fx.push(Effect::CompleteWait {
        t,
        task: crate::TaskId {
            child: c,
            run: task.run,
        },
        outcome: crate::WaitOutcome::Question,
    });
    Ok(true)
}

pub(crate) fn end_run(s: &mut OrchState, t: ThreadId, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    for q in s.questions.values_mut() {
        if q.pending && q.waiting.remove(&t) {
            view(q, t, false, fx);
        }
    }
    let held = s.thread(t)?.held != Hold::None;
    reroute(s, t, held, fx)
}
pub(crate) fn reroute(
    s: &mut OrchState,
    t: ThreadId,
    all: bool,
    fx: &mut Vec<Effect>,
) -> Result<(), Reject> {
    let to = s.thread(t)?.parent;
    let keys: Vec<_> = s
        .questions
        .iter()
        .filter(|(_, q)| q.pending && q.holder == Some(t) && (!q.waiting.contains(&t) || all))
        .map(|(k, _)| *k)
        .collect();
    for k in keys {
        let q = s.questions.get_mut(&k).unwrap();
        if !all && !blocked_holder(q, t) {
            continue;
        }
        view(q, t, true, fx);
        q.views.remove(&t);
        q.seen.remove(&t);
        q.waiting.remove(&t);
        q.holder = None;
        q.epoch = q.epoch.saturating_add(1);
        let m = Message::QuestionUp {
            child: q.child,
            qid: q.qid,
            epoch: q.epoch,
            to,
        };
        let release = Message::QuestionRelease {
            child: q.child,
            qid: q.qid,
            epoch: q.epoch,
            via: t,
        };
        s.send(release, fx);
        s.send(m, fx);
    }
    Ok(())
}
fn blocked_holder(q: &Question, t: ThreadId) -> bool {
    q.seen.contains(&t)
}
