use crate::state::ensure;
use crate::{AttemptId, Delivery, Effect, OrchState, Reject, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NativeStatus {
    Running,
    Completed,
    Failed,
    Orphaned,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeTask {
    pub attempt: AttemptId,
    pub background: bool,
    #[serde(default)]
    pub ancestor: Option<u64>,
    pub status: NativeStatus,
    pub delivery: Delivery,
    pub generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "act", content = "args")]
pub enum NativeEvent {
    Start {
        parent: ThreadId,
        key: u64,
        background: bool,
    },
    StartNested {
        parent: ThreadId,
        key: u64,
        ancestor: u64,
        background: bool,
    },
    End {
        parent: ThreadId,
        key: u64,
        failed: bool,
    },
    Claim {
        parent: ThreadId,
        key: u64,
    },
    Delivered {
        parent: ThreadId,
        key: u64,
        generation: u32,
    },
    Recover {
        parent: ThreadId,
        key: u64,
    },
}
impl NativeEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Start { .. } => "NativeStart",
            Self::StartNested { .. } => "NativeStartNested",
            Self::End { .. } => "NativeEnd",
            Self::Claim { .. } => "NativeClaim",
            Self::Delivered { .. } => "NativeDelivered",
            Self::Recover { .. } => "NativeRecover",
        }
    }
}
pub(crate) fn apply(s: &mut OrchState, e: NativeEvent, fx: &mut Vec<Effect>) -> Result<(), Reject> {
    match e {
        NativeEvent::Start {
            parent,
            key,
            background,
        } => {
            if s.natives.contains_key(&(parent, key)) {
                return Ok(());
            }
            let th = s.thread(parent)?;
            ensure(th.is_live(), Reject::NotLive)?;
            let attempt = th.attempt_id();
            s.natives.insert(
                (parent, key),
                NativeTask {
                    attempt,
                    background,
                    ancestor: None,
                    status: NativeStatus::Running,
                    delivery: Delivery::Pending,
                    generation: 0,
                },
            );
            fx.push(Effect::NativeChanged { parent, key });
        }
        NativeEvent::StartNested {
            parent,
            key,
            ancestor,
            background,
        } => {
            if s.natives.contains_key(&(parent, key)) {
                return Ok(());
            }
            ensure(key != ancestor, Reject::NotKid)?;
            let owner = *s.natives.get(&(parent, ancestor)).ok_or(Reject::NotKid)?;
            ensure(owner.status == NativeStatus::Running, Reject::NotLive)?;
            let root = s.thread(parent)?;
            ensure(
                owner.background
                    || (root.is_live()
                        && !root.attempt.emitted
                        && root.attempt_id() == owner.attempt),
                Reject::NotLive,
            )?;
            s.natives.insert(
                (parent, key),
                NativeTask {
                    attempt: owner.attempt,
                    background: background || owner.background,
                    ancestor: Some(ancestor),
                    status: NativeStatus::Running,
                    delivery: Delivery::Pending,
                    generation: 0,
                },
            );
            fx.push(Effect::NativeChanged { parent, key });
        }
        NativeEvent::End {
            parent,
            key,
            failed,
        } => {
            let n = s.natives.get_mut(&(parent, key)).ok_or(Reject::NotKid)?;
            if n.status != NativeStatus::Running {
                return Ok(());
            }
            n.status = if failed {
                NativeStatus::Failed
            } else {
                NativeStatus::Completed
            };
            let inline = s
                .threads
                .get(&parent)
                .is_some_and(|th| th.is_live() && th.attempt_id() == n.attempt);
            if inline {
                n.delivery = Delivery::Delivered;
            } else {
                n.delivery = Delivery::Claimed;
                fx.push(Effect::DeliverNative {
                    parent,
                    key,
                    generation: n.generation,
                });
            }
            fx.push(Effect::NativeChanged { parent, key });
        }
        NativeEvent::Claim { parent, key } => {
            let n = s.natives.get_mut(&(parent, key)).ok_or(Reject::NotKid)?;
            ensure(n.status != NativeStatus::Running, Reject::NotSettled)?;
            if n.delivery == Delivery::Pending {
                n.delivery = Delivery::Claimed;
                fx.push(Effect::DeliverNative {
                    parent,
                    key,
                    generation: n.generation,
                });
            }
        }
        NativeEvent::Delivered {
            parent,
            key,
            generation,
        } => {
            let n = s.natives.get_mut(&(parent, key)).ok_or(Reject::NotKid)?;
            if n.generation == generation && n.delivery == Delivery::Claimed {
                n.delivery = Delivery::Delivered;
            }
        }
        NativeEvent::Recover { parent, key } => {
            let n = s.natives.get_mut(&(parent, key)).ok_or(Reject::NotKid)?;
            if n.delivery == Delivery::Claimed {
                n.generation = n.generation.saturating_add(1);
                fx.push(Effect::DeliverNative {
                    parent,
                    key,
                    generation: n.generation,
                });
            }
        }
    }
    Ok(())
}
pub(crate) fn reconcile(s: &mut OrchState, fx: &mut Vec<Effect>) {
    let mut changed = Vec::new();
    for ((parent, key), n) in s.natives.iter() {
        if n.status == NativeStatus::Running
            && !n.background
            && (s
                .threads
                .get(parent)
                .is_none_or(|th| !th.is_live() || th.attempt_id() != n.attempt)
                || {
                    let mut ancestor = n.ancestor;
                    let mut invalid = false;
                    let mut depth = 0;
                    while let Some(id) = ancestor {
                        depth += 1;
                        match s.natives.get(&(*parent, id)) {
                            Some(owner)
                                if owner.status == NativeStatus::Running
                                    && depth <= s.natives.len() =>
                            {
                                ancestor = owner.ancestor
                            }
                            _ => {
                                invalid = true;
                                break;
                            }
                        }
                    }
                    invalid
                })
        {
            changed.push((*parent, *key));
        }
    }
    for (parent, key) in changed {
        let n = s.natives.get_mut(&(parent, key)).unwrap();
        n.status = NativeStatus::Orphaned;
        n.delivery = Delivery::Disposed;
        fx.push(Effect::NativeChanged { parent, key });
    }
}
