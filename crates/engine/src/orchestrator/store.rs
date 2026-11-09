use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use zeron_orch::{Effect, OrchState, Reject};

use super::DriverError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ok")]
pub enum Applied {
    #[serde(rename = "accepted")]
    Accepted { seq: u64, effects: Vec<String> },
    #[serde(rename = "rejected")]
    Rejected { seq: u64, reason: Reject },
}

impl Applied {
    pub fn seq(&self) -> u64 {
        match self {
            Applied::Accepted { seq, .. } | Applied::Rejected { seq, .. } => *seq,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxRow {
    pub effect_id: String,
    pub seq: u64,
    pub idx: u32,
    pub effect: Effect,
    pub done: bool,
}

pub fn meta(tx: &Transaction<'_>, key: &str) -> Result<Option<String>, DriverError> {
    Ok(tx
        .query_row(
            "SELECT value FROM orch_meta WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn put_meta(tx: &Transaction<'_>, key: &str, value: &str) -> Result<(), DriverError> {
    tx.execute(
        "INSERT INTO orch_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn delete_meta(tx: &Transaction<'_>, key: &str) -> Result<(), DriverError> {
    tx.execute("DELETE FROM orch_meta WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn last_seq(tx: &Transaction<'_>) -> Result<Option<u64>, DriverError> {
    let Some(v) = meta(tx, "seq")? else {
        return Ok(None);
    };
    v.parse()
        .map(Some)
        .map_err(|_| DriverError::Corrupt(format!("orch_meta seq {v:?}")))
}

pub fn next_seq(tx: &Transaction<'_>) -> Result<u64, DriverError> {
    let seq = last_seq(tx)?.map_or(0, |s| s + 1);
    put_meta(tx, "seq", &seq.to_string())?;
    Ok(seq)
}

pub fn load_state(tx: &Transaction<'_>) -> Result<Option<OrchState>, DriverError> {
    let json: Option<String> = tx
        .query_row("SELECT state FROM orch_state WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?;
    json.map(|j| serde_json::from_str(&j).map_err(DriverError::from))
        .transpose()
}

pub fn save_state(tx: &Transaction<'_>, seq: u64, s: &OrchState) -> Result<(), DriverError> {
    let json = serde_json::to_string(s)?;
    tx.execute(
        "INSERT INTO orch_state (id, seq, state) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET seq = excluded.seq, state = excluded.state",
        params![sql_int(seq)?, json],
    )?;
    Ok(())
}

pub fn receipt(tx: &Transaction<'_>, key: &str) -> Result<Option<Applied>, DriverError> {
    let json: Option<String> = tx
        .query_row(
            "SELECT result FROM orch_receipts WHERE key = ?1",
            params![key],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|j| serde_json::from_str(&j).map_err(DriverError::from))
        .transpose()
}

pub fn put_receipt(tx: &Transaction<'_>, key: &str, applied: &Applied) -> Result<(), DriverError> {
    let json = serde_json::to_string(applied)?;
    tx.execute(
        "INSERT INTO orch_receipts (key, seq, result) VALUES (?1, ?2, ?3)",
        params![key, sql_int(applied.seq())?, json],
    )?;
    Ok(())
}

pub fn put_outbox(
    tx: &Transaction<'_>,
    effect_id: &str,
    seq: u64,
    idx: u32,
    effect: &Effect,
) -> Result<(), DriverError> {
    let json = serde_json::to_string(effect)?;
    tx.execute(
        "INSERT INTO orch_outbox (effect_id, seq, idx, payload, status)
         VALUES (?1, ?2, ?3, ?4, 'pending') ON CONFLICT(effect_id) DO NOTHING",
        params![effect_id, sql_int(seq)?, idx, json],
    )?;
    Ok(())
}

fn outbox_rows(tx: &Transaction<'_>, only_pending: bool) -> Result<Vec<OutboxRow>, DriverError> {
    let sql = if only_pending {
        "SELECT effect_id, seq, idx, payload, status FROM orch_outbox
         WHERE status = 'pending' ORDER BY seq, idx"
    } else {
        "SELECT effect_id, seq, idx, payload, status FROM orch_outbox ORDER BY seq, idx"
    };
    let mut stmt = tx.prepare(sql)?;
    let raw = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(|(effect_id, seq, idx, payload, status)| {
            Ok(OutboxRow {
                effect_id,
                seq: u64::try_from(seq).map_err(|_| DriverError::Corrupt("outbox seq".into()))?,
                idx,
                effect: serde_json::from_str(&payload)?,
                done: status == "done",
            })
        })
        .collect()
}

pub fn pending_outbox(tx: &Transaction<'_>) -> Result<Vec<OutboxRow>, DriverError> {
    outbox_rows(tx, true)
}

pub fn all_outbox(tx: &Transaction<'_>) -> Result<Vec<OutboxRow>, DriverError> {
    outbox_rows(tx, false)
}

pub fn mark_done(tx: &Transaction<'_>, effect_id: &str) -> Result<bool, DriverError> {
    let n = tx.execute(
        "UPDATE orch_outbox SET status = 'done' WHERE effect_id = ?1 AND status = 'pending'",
        params![effect_id],
    )?;
    Ok(n == 1)
}

pub fn put_trace(
    tx: &Transaction<'_>,
    seq: u64,
    dev: &str,
    act: &str,
    accepted: bool,
    line: &str,
) -> Result<(), DriverError> {
    tx.execute(
        "INSERT INTO orch_trace (seq, dev, act, accepted, line) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sql_int(seq)?, dev, act, accepted, line],
    )?;
    Ok(())
}

pub fn trace_lines(tx: &Transaction<'_>) -> Result<Vec<String>, DriverError> {
    let mut stmt = tx.prepare("SELECT line FROM orch_trace ORDER BY seq")?;
    let lines = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(lines)
}

fn sql_int(v: u64) -> Result<i64, DriverError> {
    i64::try_from(v).map_err(|_| DriverError::Corrupt(format!("seq {v} overflows i64")))
}
