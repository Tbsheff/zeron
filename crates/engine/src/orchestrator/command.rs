use super::DriverError;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    WireSend,
    WireInterrupt,
    DocHostExecute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandDisposition {
    Effects,
    Refused,
    Bypass,
    Error,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRefusal {
    AlreadyRan,
    InFlight,
    ReducerRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub source: CommandSource,
    pub disposition: CommandDisposition,
    pub message_id: Option<String>,
    pub effect_ids: Vec<String>,
    pub refusal: Option<CommandRefusal>,
}

impl CommandReceipt {
    pub fn phase(&self) -> &'static str {
        if self.source == CommandSource::DocHostExecute {
            "outcome"
        } else {
            "decision"
        }
    }
}

pub fn init(tx: &Transaction<'_>) -> Result<(), DriverError> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS orch_command_decisions(command_id TEXT NOT NULL,phase TEXT NOT NULL,receipt TEXT NOT NULL,PRIMARY KEY(command_id,phase));")?;
    Ok(())
}

pub fn get(
    tx: &Transaction<'_>,
    id: &str,
    phase: &str,
) -> Result<Option<CommandReceipt>, DriverError> {
    let value: Option<String> = tx
        .query_row(
            "SELECT receipt FROM orch_command_decisions WHERE command_id=?1 AND phase=?2",
            params![id, phase],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|s| serde_json::from_str(&s).map_err(DriverError::from))
        .transpose()
}

pub fn put(tx: &Transaction<'_>, id: &str, receipt: &CommandReceipt) -> Result<(), DriverError> {
    tx.execute(
        "INSERT INTO orch_command_decisions VALUES(?1,?2,?3)",
        params![id, receipt.phase(), serde_json::to_string(receipt)?],
    )?;
    Ok(())
}

pub fn all(tx: &Transaction<'_>) -> Result<Vec<serde_json::Value>, DriverError> {
    let mut stmt = tx.prepare(
        "SELECT command_id,phase,receipt FROM orch_command_decisions ORDER BY command_id,phase",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|(id,phase,receipt)|Ok(serde_json::json!({"command_id":id,"phase":phase,"receipt":serde_json::from_str::<CommandReceipt>(&receipt)?}))).collect()
}
