use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Transaction;
use zeron_orch::{AttemptId, Config, DelEvent, Effect, Event, OrchState, step};
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
    capture: bool,
    faults: Faults,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Driver {
    pub fn open(store: Arc<DocsStore>, device: &str, config: Config) -> Result<Self, DriverError> {
        Self::open_with_capture(
            store,
            device,
            config,
            std::env::var("ORCH_CAPTURE_MECHANISM").is_ok_and(|v| v == "1"),
        )
    }

    pub fn open_with_capture(
        store: Arc<DocsStore>,
        device: &str,
        config: Config,
        capture: bool,
    ) -> Result<Self, DriverError> {
        let capture = store.transact(|tx| {
            let existing = store::load_state(tx)?.is_some();
            init(tx, device, config)?;
            super::mechanism::init(tx)?;
            super::command::init(tx)?;
            let capture = capture || store::meta(tx, "mechanism_capture")?.as_deref() == Some("1");
            if capture {
                store::put_meta(tx, "mechanism_capture", "1")?;
                let dev = store::meta(tx, "device")?.ok_or_else(|| DriverError::Corrupt("missing device".into()))?;
                super::mechanism::begin_capture_group(tx, &dev)?;
                if existing {
                    super::mechanism::capture(tx, &dev, "RecoveryOpen", serde_json::json!({}))?;
                    let before: i64 = tx.query_row("SELECT COUNT(*) FROM orch_mechanism_facts", [], |r| r.get(0))?;
                    let fact_id = super::transfer::capture_initial_catalog(tx)?;
                    let after: i64 = tx.query_row("SELECT COUNT(*) FROM orch_mechanism_facts", [], |r| r.get(0))?;
                    if after != before {
                        if after != before + 1 { return Err(DriverError::Corrupt("catalog changed more than one fact".into())); }
                        super::mechanism::capture(tx, &dev, "ObserveCatalog", serde_json::json!({"fact_id":fact_id,"kind":"transfer_catalog","source":"durable_snapshots"}))?;
                    }
                } else {
                    super::transfer::capture_initial_catalog(tx)?;
                    super::mechanism::capture(tx, &dev, "Init", serde_json::json!({}))?;
                }
                super::mechanism::end_capture_group(tx)?;
            }
            Ok::<bool, DriverError>(capture)
        })?;
        let device = store
            .transact(|tx| store::meta(tx, "device"))?
            .ok_or_else(|| DriverError::Corrupt("orch_meta has no device".into()))?;
        Ok(Self {
            store,
            device,
            capture,
            faults: Faults::default(),
            changed: tokio::sync::watch::Sender::new(0),
        })
    }

    pub fn mechanism_capture_enabled(&self) -> bool { self.capture }

    pub fn mechanism_lines(&self) -> Result<Vec<String>, DriverError> {
        self.store.transact(super::mechanism::lines)
    }

    pub fn observe_mechanism(
        &self,
        id: &str,
        kind: &str,
        data: serde_json::Value,
    ) -> Result<(), DriverError> {
        if !self.capture {
            return Ok(());
        }
        self.store.transact(|tx| {
            super::mechanism::fact(tx, id, kind, &data)?;
            self.capture_tx(tx, "Observe", serde_json::json!({"fact_id":id,"kind":kind,"effect_id":data.get("effect_id").and_then(serde_json::Value::as_str)}))
        })
    }

    fn capture_tx(
        &self,
        tx: &Transaction<'_>,
        action: &str,
        args: serde_json::Value,
    ) -> Result<(), DriverError> {
        if self.capture {
            super::mechanism::capture(tx, &self.device, action, args)?;
        }
        Ok(())
    }

    pub fn record_command_decision(&self, id: &str, receipt: super::CommandReceipt) -> Result<super::CommandReceipt, DriverError> {
        self.store.transact(|tx| {
            let previous = super::command::get(tx, id, receipt.phase())?;
            let duplicate = previous.is_some();
            let stored = previous.unwrap_or_else(|| receipt.clone());
            if !duplicate { super::command::put(tx, id, &stored)?; }
            store::put_meta(tx, &format!("cmd:{id}"), "1")?;
            self.capture_tx(tx, "CommandDecision", serde_json::json!({"command_id":id,"phase":receipt.phase(),"duplicate":duplicate,"observed":receipt,"receipt":stored}))?;
            Ok(stored)
        })
    }

    pub fn command_decision(&self, id: &str, phase: &str) -> Result<Option<super::CommandReceipt>, DriverError> {
        self.store.transact(|tx| super::command::get(tx, id, phase))
    }

    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
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

    pub(crate) fn record_engine_interrupt(&self, key: &str, at: AttemptId) -> Result<(), DriverError> {
        let duplicate = self.store.transact(|tx| {
            if store::receipt(tx, key)?.is_none() && !loaded(tx)?.thread(at.run.thread).is_ok_and(|thread|
                thread.is_live() && !thread.int_req && thread.attempt_id() == at)
            {
                return Ok(true);
            }
            if self.capture { super::mechanism::begin_capture_group(tx, &self.device)?; }
            let out = self.apply_tx(tx, key, None, &Event::Del(DelEvent::InterruptChat { t: at.run.thread }))?;
            if let Applied::Accepted { effects, .. } = &out.applied && !out.duplicate {
                for id in effects {
                    let payload: String = tx.query_row("SELECT payload FROM orch_outbox WHERE effect_id = ?1", [id], |row| row.get(0))?;
                    if !matches!(serde_json::from_str::<Effect>(&payload)?, Effect::InterruptRun { run } if run == at.run) {
                        continue;
                    }
                    self.faults.hit(FaultPoint::Fp2EffectRunNotMarked)?;
                    store::mark_done(tx, id)?;
                    self.capture_tx(tx, "CompleteEffect", serde_json::json!({"effect_id":id}))?;
                }
            }
            if self.capture { super::mechanism::end_capture_group(tx)?; }
            Ok::<bool, DriverError>(out.duplicate)
        })?;
        self.changed();
        if !duplicate {
            self.faults.hit(FaultPoint::Fp1StepCommittedEffectNotRun)?;
        }
        Ok(())
    }

    pub fn apply_if(
        &self,
        key: &str,
        event: Event,
        enabled: &dyn Fn(&OrchState) -> bool,
    ) -> Result<Option<ApplyResult>, DriverError> {
        let out = self.store.transact(|tx| {
            if store::receipt(tx, key)?.is_none() && !enabled(&loaded(tx)?) {
                return Ok(None);
            }
            self.apply_tx(tx, key, None, &event).map(Some)
        })?;
        self.changed();
        if out.as_ref().is_some_and(|o| !o.duplicate) {
            self.faults.hit(FaultPoint::Fp1StepCommittedEffectNotRun)?;
        }
        Ok(out)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, DriverError> {
        self.store.transact(|tx| store::meta(tx, key))
    }

    pub fn put_meta(&self, key: &str, value: &str) -> Result<(), DriverError> {
        self.store.transact(|tx| store::put_meta(tx, key, value))?;
        self.changed();
        Ok(())
    }

    pub fn delete_meta(&self, key: &str) -> Result<(), DriverError> {
        self.store.transact(|tx| store::delete_meta(tx, key))
    }

    pub fn intern(&self, key: &str, counter: &str) -> Result<(u64, bool), DriverError> {
        self.store.transact(|tx| {
            if let Some(v) = store::meta(tx, key)? {
                return parse_meta(key, &v).map(|n| (n, false));
            }
            let last = match store::meta(tx, counter)? {
                Some(v) => parse_meta(counter, &v)?,
                None => 0,
            };
            let n = last + 1;
            store::put_meta(tx, counter, &n.to_string())?;
            store::put_meta(tx, key, &n.to_string())?;
            Ok((n, true))
        })
    }

    pub fn bump(&self, counter: &str) -> Result<u64, DriverError> {
        self.store.transact(|tx| {
            let n = match store::meta(tx, counter)? {
                Some(v) => parse_meta(counter, &v)? + 1,
                None => 1,
            };
            store::put_meta(tx, counter, &n.to_string())?;
            Ok(n)
        })
    }

    pub fn complete(&self, effect_id: &str) -> Result<(), DriverError> {
        self.faults.hit(FaultPoint::Fp2EffectRunNotMarked)?;
        self.store.transact(|tx| {
            store::mark_done(tx, effect_id)?;
            self.capture_tx(
                tx,
                "CompleteEffect",
                serde_json::json!({"effect_id":effect_id}),
            )
        })?;
        self.changed();
        Ok(())
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
        self.changed();
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
            self.capture_tx(tx, "Apply", serde_json::json!({"key":key,"command_id":cmd,"event":event,"duplicate":true,"accepted":matches!(prev, Applied::Accepted {..})}))?;
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
        self.capture_tx(tx, "Apply", serde_json::json!({"key":key,"command_id":cmd,"event":event,"duplicate":false,"accepted":matches!(applied, Applied::Accepted {..})}))?;
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
            self.mark_command(key)?;
        }
        Ok(out)
    }

    pub fn apply_command(
        &self,
        key: &str,
        cmd: Option<&str>,
        event: Event,
    ) -> Result<ApplyResult, DriverError> {
        self.apply_cmd(key, cmd, event)
    }

    pub fn receipt(&self, key: &str) -> Result<Option<Applied>, DriverError> {
        self.store.transact(|tx| store::receipt(tx, key))
    }

    pub fn has_receipt(&self, key: &str) -> Result<bool, DriverError> {
        Ok(self.store.transact(|tx| store::receipt(tx, key))?.is_some())
    }

    pub fn mark_command(&self, cmd: &str) -> Result<(), DriverError> {
        self.faults.hit(FaultPoint::Fp4ReceiptNotMarked)?;
        self.store.transact(|tx| {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            tx.execute(
                "INSERT OR IGNORE INTO processed_commands(command_id,processed_at) VALUES(?1,?2)",
                rusqlite::params![cmd, ms],
            )?;
            self.capture_tx(tx, "MarkCommand", serde_json::json!({"command_id":cmd}))
        })?;
        self.changed();
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<OutboxRow>, DriverError> {
        self.store.transact(store::pending_outbox)
    }

    pub fn effect_pending(&self, effect_id: &str) -> Result<bool, DriverError> {
        self.store.transact(|tx| store::effect_pending(tx, effect_id))
    }

    pub fn outbox(&self) -> Result<Vec<OutboxRow>, DriverError> {
        self.store.transact(store::all_outbox)
    }

    pub fn run_outbox(&self, sink: &mut dyn EffectSink) -> Result<usize, DriverError> {
        let rows = self.pending()?;
        for row in &rows {
            sink.execute(&row.effect_id, &row.effect)
                .map_err(|e| DriverError::Effect(row.effect_id.clone(), e.0))?;
            self.complete(&row.effect_id)?;
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

fn parse_meta(key: &str, v: &str) -> Result<u64, DriverError> {
    v.parse()
        .map_err(|_| DriverError::Corrupt(format!("orch_meta {key} = {v:?}")))
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
