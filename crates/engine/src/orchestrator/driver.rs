use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Transaction;
use zeron_orch::{Config, Effect, Event, OrchState, step};
use zeron_sync::DocsStore;

use super::DriverError;
use super::store::{self, Applied, OutboxRow};
use super::trace::{self, Step, TraceEffect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultPoint {
    Fp1StepCommittedEffectNotRun,
    Fp2EffectRunNotMarked,
    Fp3ReceivedNoReceipt,
    Fp4ReceiptNotMarked,
}

impl FaultPoint {
    pub const ALL: [FaultPoint; 4] = [
        FaultPoint::Fp1StepCommittedEffectNotRun,
        FaultPoint::Fp2EffectRunNotMarked,
        FaultPoint::Fp3ReceivedNoReceipt,
        FaultPoint::Fp4ReceiptNotMarked,
    ];
}

#[derive(Debug, Default)]
struct Faults {
    armed: Mutex<Option<(FaultPoint, usize)>>,
}

impl Faults {
    fn arm(&self, at: FaultPoint, skip: usize) {
        *self.armed.lock().unwrap_or_else(PoisonError::into_inner) = Some((at, skip));
    }

    fn hit(&self, here: FaultPoint) -> Result<(), DriverError> {
        let mut armed = self.armed.lock().unwrap_or_else(PoisonError::into_inner);
        match armed.as_mut() {
            Some((at, 0)) if *at == here => {
                *armed = None;
                Err(DriverError::Crashed(here))
            }
            Some((at, n)) if *at == here => {
                *n -= 1;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SinkError(pub String);

pub trait EffectSink {
    fn execute(&mut self, effect_id: &str, effect: &Effect) -> Result<(), SinkError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    pub applied: Applied,
    pub duplicate: bool,
}

pub fn effect_id(key: &str, idx: usize) -> String {
    format!("{key}#{idx}")
}

pub struct Driver {
    store: Arc<DocsStore>,
    device: String,
    faults: Faults,
}

impl Driver {
    pub fn open(store: Arc<DocsStore>, device: &str, config: Config) -> Result<Self, DriverError> {
        store.transact(|tx| init(tx, device, config))?;
        let device = store
            .transact(|tx| store::meta(tx, "device"))?
            .ok_or_else(|| DriverError::Corrupt("orch_meta has no device".into()))?;
        Ok(Self {
            store,
            device,
            faults: Faults::default(),
        })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn crash_at(&self, at: FaultPoint, skip: usize) {
        self.faults.arm(at, skip);
    }

    pub fn state(&self) -> Result<OrchState, DriverError> {
        self.store.transact(loaded)
    }

    pub fn apply(&self, key: &str, event: Event) -> Result<ApplyResult, DriverError> {
        self.apply_cmd(key, None, event)
    }

    fn apply_cmd(
        &self,
        key: &str,
        cmd: Option<&str>,
        event: Event,
    ) -> Result<ApplyResult, DriverError> {
        let out = self
            .store
            .transact(|tx| self.apply_tx(tx, key, cmd, &event))?;
        if !out.duplicate {
            self.faults.hit(FaultPoint::Fp1StepCommittedEffectNotRun)?;
        }
        Ok(out)
    }

    fn apply_tx(
        &self,
        tx: &Transaction<'_>,
        key: &str,
        cmd: Option<&str>,
        event: &Event,
    ) -> Result<ApplyResult, DriverError> {
        let seq = store::next_seq(tx)?;
        let state = loaded(tx)?;
        let base = Step {
            dev: &self.device,
            seq,
            key,
            cmd,
            event,
            dup: false,
            reject: None,
            fx: Vec::new(),
            post: &state,
        };
        if let Some(prev) = store::receipt(tx, key)? {
            write_trace(tx, Step { dup: true, ..base })?;
            return Ok(ApplyResult {
                applied: prev,
                duplicate: true,
            });
        }
        let (applied, fx, next) = match step(&state, *event) {
            Ok((next, effects)) => {
                store::save_state(tx, seq, &next)?;
                let fx = enqueue(tx, key, seq, effects)?;
                let ids = fx.iter().map(|f| f.id.clone()).collect();
                (Applied::Accepted { seq, effects: ids }, fx, Some(next))
            }
            Err(reason) => (Applied::Rejected { seq, reason }, Vec::new(), None),
        };
        self.faults.hit(FaultPoint::Fp3ReceivedNoReceipt)?;
        store::put_receipt(tx, key, &applied)?;
        let reject = match &applied {
            Applied::Rejected { reason, .. } => Some(*reason),
            Applied::Accepted { .. } => None,
        };
        let post = next.as_ref().unwrap_or(&state);
        write_trace(
            tx,
            Step {
                reject,
                fx,
                post,
                ..base
            },
        )?;
        Ok(ApplyResult {
            applied,
            duplicate: false,
        })
    }

    pub fn drain(&self, rows: &[(String, Event)]) -> Result<Vec<ApplyResult>, DriverError> {
        let mut out = Vec::new();
        for (key, event) in rows {
            if self.store.is_processed(key)? {
                continue;
            }
            out.push(self.apply_cmd(key, Some(key), *event)?);
            self.faults.hit(FaultPoint::Fp4ReceiptNotMarked)?;
            self.store.mark_processed(key)?;
        }
        Ok(out)
    }

    pub fn pending(&self) -> Result<Vec<OutboxRow>, DriverError> {
        self.store.transact(store::pending_outbox)
    }

    pub fn outbox(&self) -> Result<Vec<OutboxRow>, DriverError> {
        self.store.transact(store::all_outbox)
    }

    pub fn run_outbox(&self, sink: &mut dyn EffectSink) -> Result<usize, DriverError> {
        let rows = self.pending()?;
        for row in &rows {
            sink.execute(&row.effect_id, &row.effect)
                .map_err(|e| DriverError::Effect(row.effect_id.clone(), e.0))?;
            self.faults.hit(FaultPoint::Fp2EffectRunNotMarked)?;
            self.store
                .transact(|tx| store::mark_done(tx, &row.effect_id))?;
        }
        Ok(rows.len())
    }

    pub fn trace_lines(&self) -> Result<Vec<String>, DriverError> {
        self.store.transact(store::trace_lines)
    }
}

fn init(tx: &Transaction<'_>, device: &str, config: Config) -> Result<(), DriverError> {
    if store::load_state(tx)?.is_some() {
        return Ok(());
    }
    let s = OrchState::new(config);
    let seq = store::next_seq(tx)?;
    store::put_meta(tx, "device", device)?;
    store::save_state(tx, seq, &s)?;
    let line = serde_json::to_string(&trace::init_line(device, &s)?)?;
    store::put_trace(tx, seq, device, "Init", true, &line)
}

fn loaded(tx: &Transaction<'_>) -> Result<OrchState, DriverError> {
    store::load_state(tx)?.ok_or_else(|| DriverError::Corrupt("orch_state row missing".into()))
}

fn enqueue(
    tx: &Transaction<'_>,
    key: &str,
    seq: u64,
    effects: Vec<Effect>,
) -> Result<Vec<TraceEffect>, DriverError> {
    let mut fx = Vec::with_capacity(effects.len());
    for (i, effect) in effects.into_iter().enumerate() {
        let id = effect_id(key, i);
        let idx = u32::try_from(i).map_err(|_| DriverError::Corrupt("effect index".into()))?;
        store::put_outbox(tx, &id, seq, idx, &effect)?;
        fx.push(TraceEffect { id, effect });
    }
    Ok(fx)
}

fn write_trace(tx: &Transaction<'_>, st: Step<'_>) -> Result<(), DriverError> {
    let (seq, dev) = (st.seq, st.dev.to_owned());
    let l = trace::line(st)?;
    let json = serde_json::to_string(&l)?;
    store::put_trace(tx, seq, &dev, &l.act, l.ok, &json)
}
