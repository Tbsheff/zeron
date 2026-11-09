use super::{DriverError, store};
use rusqlite::{Transaction, params};
use serde_json::{Value, json};

pub fn init(tx: &Transaction<'_>) -> Result<(), DriverError> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS orch_mechanism(seq INTEGER PRIMARY KEY,line TEXT NOT NULL); CREATE TABLE IF NOT EXISTS orch_mechanism_facts(id TEXT PRIMARY KEY,kind TEXT NOT NULL,data TEXT NOT NULL);")?;
    Ok(())
}

pub fn lines(tx: &Transaction<'_>) -> Result<Vec<String>, DriverError> {
    let mut stmt = tx.prepare("SELECT line FROM orch_mechanism ORDER BY seq")?;
    Ok(stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

pub fn fact(tx: &Transaction<'_>, id: &str, kind: &str, data: &Value) -> Result<(), DriverError> {
    let encoded = serde_json::to_string(data)?;
    let old: Option<(String, String)> = {
        use rusqlite::OptionalExtension;
        tx.query_row(
            "SELECT kind,data FROM orch_mechanism_facts WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    };
    if let Some(old) = old {
        if old != (kind.to_owned(), encoded) {
            return Err(DriverError::Corrupt(format!(
                "conflicting mechanism fact {id}"
            )));
        }
    } else {
        tx.execute(
            "INSERT INTO orch_mechanism_facts VALUES(?1,?2,?3)",
            params![id, kind, encoded],
        )?;
    }
    Ok(())
}

pub fn capture(
    tx: &Transaction<'_>,
    dev: &str,
    action: &str,
    args: Value,
) -> Result<(), DriverError> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq)+1,0) FROM orch_mechanism",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = tx.prepare("SELECT key,seq,result FROM orch_receipts ORDER BY seq,key")?;
    let raw = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let receipts = raw
        .into_iter()
        .map(|(key, seq, result)| {
            Ok(json!({"key":key,"seq":seq,"result":serde_json::from_str::<Value>(&result)?}))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    let mut stmt = tx.prepare("SELECT command_id FROM processed_commands ORDER BY command_id")?;
    let processed = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut stmt =
        tx.prepare("SELECT key,value FROM orch_meta WHERE key LIKE 'thread:%' ORDER BY key")?;
    let bindings = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut threads = Vec::new();
    for (key, chat) in bindings {
        let local_thread = key
            .strip_prefix("thread:")
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| DriverError::Corrupt("thread binding".into()))?;
        let host =
            store::meta(tx, &format!("remote-host:{chat}"))?;
        threads
            .push(json!({"local_thread":local_thread,"chat":chat,"shadow":host.as_deref().map(|h|h!=dev),"host":host,"host_known":host.is_some()}));
    }
    let mut stmt = tx.prepare("SELECT id,kind,data FROM orch_mechanism_facts ORDER BY id")?;
    let raw = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let facts = raw
        .into_iter()
        .map(|(id, kind, data)| {
            Ok(json!({"id":id,"kind":kind,"data":serde_json::from_str::<Value>(&data)?}))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    let row = json!({"v":2,"dev":dev,"seq":seq,"action":action,"args":args,"origin":{"kind":"sqlite","commit":format!("sqlite:{dev}:{seq}"),"parents":if seq > 0 {vec![format!("sqlite:{dev}:{}",seq-1)]} else {Vec::<String>::new()}},"snapshot":{"state":store::load_state(tx)?,"receipts":receipts,"outbox":store::all_outbox(tx)?,"processed":processed,"threads":threads,"facts":facts}});
    tx.execute(
        "INSERT INTO orch_mechanism VALUES(?1,?2)",
        params![seq, serde_json::to_string(&row)?],
    )?;
    Ok(())
}
