use super::{Driver, DriverError};
use zeron_orch::native::{NativeEvent, NativeStatus};
use zeron_orch::{Event, ThreadId};
use zeron_proto::{AgentEvent, DoneStatus, ToolCall};

pub fn doc_id(chat: &str, tool: &str) -> String {
    format!("{chat}--sub--{tool}")
}
fn key(driver: &Driver, parent: ThreadId, tool: &str) -> Result<u64, DriverError> {
    let (key, _) = driver.intern(&format!("native-key:{parent:?}:{tool}"), "native-counter")?;
    driver.put_meta(&format!("native-tool:{key}"), tool)?;
    Ok(key)
}
pub fn tool_id(driver: &Driver, key: u64) -> Result<String, DriverError> {
    driver
        .meta(&format!("native-tool:{key}"))?
        .ok_or_else(|| DriverError::Corrupt("missing native tool id".into()))
}
pub fn observe(driver: &Driver, parent: ThreadId, event: &AgentEvent) -> Result<(), DriverError> {
    match event {
        AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => {
            let k = key(driver, parent, id)?;
            let background = match call {
                ToolCall::Unknown {
                    input: Some(input), ..
                }
                | ToolCall::Mcp {
                    input: Some(input), ..
                } => input
                    .get("run_in_background")
                    .or_else(|| input.get("background"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                _ => false,
            };
            driver.put_meta(
                &format!("native-spawn:{parent:?}:{k}"),
                if background {
                    "background"
                } else {
                    "foreground"
                },
            )?;
        }
        AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } => {
            let k = key(driver, parent, parent_tool_use_id)?;
            let Some(mode) = driver.meta(&format!("native-spawn:{parent:?}:{k}"))? else {
                return Ok(());
            };
            if !driver.state()?.natives.contains_key(&(parent, k)) {
                driver.apply(
                    &format!("native-start:{parent:?}:{k}"),
                    Event::Native(NativeEvent::Start {
                        parent,
                        key: k,
                        background: mode == "background",
                    }),
                )?;
            }
            if let AgentEvent::Done { status, .. } = event.as_ref() {
                end(driver, parent, k, *status != DoneStatus::Completed)?;
            }
        }
        AgentEvent::ToolResult { id, is_error, .. } => {
            let Some(k) = driver.meta(&format!("native-key:{parent:?}:{id}"))? else {
                return Ok(());
            };
            let k = k
                .parse()
                .map_err(|_| DriverError::Corrupt("invalid native id".into()))?;
            if driver.state()?.natives.contains_key(&(parent, k)) {
                end(driver, parent, k, *is_error)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn end(driver: &Driver, parent: ThreadId, key: u64, failed: bool) -> Result<(), DriverError> {
    driver.apply(
        &format!("native-end:{parent:?}:{key}"),
        Event::Native(NativeEvent::End {
            parent,
            key,
            failed,
        }),
    )?;
    Ok(())
}
pub fn destination(parent_chat: &str, key: u64, generation: u32) -> String {
    format!("{parent_chat}--native-result--{key}--{generation}")
}
pub fn recover(driver: &Driver) -> Result<(), DriverError> {
    let tasks: Vec<_> = driver
        .state()?
        .natives
        .iter()
        .filter(|(_, n)| {
            n.status != NativeStatus::Running && n.delivery == zeron_orch::Delivery::Claimed
        })
        .map(|((p, k), _)| (*p, *k))
        .collect();
    for (parent, key) in tasks {
        let generation = driver.state()?.natives[&(parent, key)].generation;
        driver.apply(
            &format!("native-recover:{parent:?}:{key}:{generation}"),
            Event::Native(NativeEvent::Recover { parent, key }),
        )?;
    }
    Ok(())
}
pub fn delivered(
    driver: &Driver,
    parent: ThreadId,
    key: u64,
    generation: u32,
) -> Result<(), DriverError> {
    driver.apply(
        &format!("native-delivered:{parent:?}:{key}:{generation}"),
        Event::Native(NativeEvent::Delivered {
            parent,
            key,
            generation,
        }),
    )?;
    Ok(())
}
