use super::{Driver, DriverError};
use zeron_orch::native::{NativeEvent, NativeStatus};
use zeron_orch::{Event, ThreadId};
use zeron_proto::{AgentEvent, DoneStatus, ToolCall};

pub fn doc_id(chat: &str, tool: &str) -> String {
    crate::sessions::subagent_doc_id(chat, tool)
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct NativeBinding {
    pub path: Vec<String>,
    pub tool_id: String,
    pub doc_id: String,
    pub chip_doc_id: String,
    pub own_background: bool,
    pub effective_background: bool,
}
pub fn doc_id_path(chat: &str, path: &[String]) -> String {
    if path.len() == 1 {
        return doc_id(chat, &path[0]);
    }
    use sha2::{Digest, Sha256};
    format!(
        "{chat}--native-path--{:x}",
        Sha256::digest(serde_json::to_vec(path).expect("native path serializes"))
    )
}
fn lookup(parent: ThreadId, path: &[String]) -> String {
    if path.len() == 1 {
        format!("native-key:{parent:?}:{}", path[0])
    } else {
        format!(
            "native-path-key:{parent:?}:{}",
            serde_json::to_string(path).expect("native path serializes")
        )
    }
}
pub fn path_key(
    driver: &Driver,
    parent: ThreadId,
    path: &[String],
) -> Result<Option<u64>, DriverError> {
    driver
        .meta(&lookup(parent, path))?
        .map(|value| {
            value
                .parse()
                .map_err(|_| DriverError::Corrupt("invalid native key".into()))
        })
        .transpose()
}
fn key(driver: &Driver, parent: ThreadId, path: &[String]) -> Result<u64, DriverError> {
    let (key, _) = driver.intern(&lookup(parent, path), "native-counter")?;
    driver.put_meta(
        &format!("native-tool:{key}"),
        path.last()
            .ok_or_else(|| DriverError::Corrupt("empty native path".into()))?,
    )?;
    Ok(key)
}
pub fn tool_id(driver: &Driver, key: u64) -> Result<String, DriverError> {
    driver
        .meta(&format!("native-tool:{key}"))?
        .ok_or_else(|| DriverError::Corrupt("missing native tool id".into()))
}
pub fn binding(driver: &Driver, key: u64) -> Result<Option<NativeBinding>, DriverError> {
    driver
        .meta(&format!("native-binding:{key}"))?
        .map(|raw| serde_json::from_str(&raw).map_err(DriverError::from))
        .transpose()
}
fn spawn(
    driver: &Driver,
    parent: ThreadId,
    path: &[String],
    call: &ToolCall,
) -> Result<(), DriverError> {
    let key = key(driver, parent, path)?;
    let own = match call {
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
    let mut background = own;
    for end in 1..path.len() {
        if let Some(ancestor) = path_key(driver, parent, &path[..end])? {
            background |= driver
                .meta(&format!("native-spawn:{parent:?}:{ancestor}"))?
                .as_deref()
                == Some("background");
        }
    }
    driver.put_meta(
        &format!("native-spawn:{parent:?}:{key}"),
        if background {
            "background"
        } else {
            "foreground"
        },
    )?;
    if let Some(chat) = driver.meta(&format!("thread:{}", parent.0))? {
        let binding = NativeBinding {
            path: path.to_vec(),
            tool_id: path.last().unwrap().clone(),
            doc_id: doc_id_path(&chat, path),
            chip_doc_id: if path.len() == 1 {
                chat.clone()
            } else {
                doc_id_path(&chat, &path[..path.len() - 1])
            },
            own_background: own,
            effective_background: background,
        };
        driver.put_meta(
            &format!("native-binding:{key}"),
            &serde_json::to_string(&binding)?,
        )?;
        if driver.mechanism_capture_enabled() {
            driver.observe_mechanism(&format!("native-binding:{parent:?}:{key}"),"native_binding",serde_json::json!({"chat":chat,"task_key":key,"native_path":binding.path,"tool_id":binding.tool_id,"doc_id":binding.doc_id,"chip_doc_id":binding.chip_doc_id,"own_background":own,"effective_background":background}))?;
        }
    }
    if path.len() > 1 {
        activate(driver, parent, path)?;
    }
    Ok(())
}
pub fn observe(driver: &Driver, parent: ThreadId, event: &AgentEvent) -> Result<(), DriverError> {
    observe_at(driver, parent, event, &mut Vec::new())
}
fn observe_at(
    driver: &Driver,
    parent: ThreadId,
    event: &AgentEvent,
    path: &mut Vec<String>,
) -> Result<(), DriverError> {
    match event {
        AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => {
            path.push(id.clone());
            spawn(driver, parent, path, call)?;
            path.pop();
        }
        AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } => {
            path.push(parent_tool_use_id.clone());
            if let Some(k) = path_key(driver, parent, path)? {
                if driver
                    .meta(&format!("native-spawn:{parent:?}:{k}"))?
                    .is_some()
                {
                    activate(driver, parent, path)?;
                    if let AgentEvent::Done { status, .. } = event.as_ref() {
                        end(driver, parent, k, *status != DoneStatus::Completed)?;
                    }
                }
            }
            observe_at(driver, parent, event, path)?;
            path.pop();
        }
        AgentEvent::ToolResult { id, is_error, .. } => {
            path.push(id.clone());
            if let Some(k) = path_key(driver, parent, path)? {
                if driver
                    .state()?
                    .natives
                    .get(&(parent, k))
                    .is_some_and(|n| n.status == NativeStatus::Running)
                {
                    end(driver, parent, k, *is_error)?;
                }
            }
            path.pop();
        }
        _ => {}
    }
    Ok(())
}
pub fn activate(driver: &Driver, parent: ThreadId, path: &[String]) -> Result<(), DriverError> {
    let Some(k) = path_key(driver, parent, path)? else {
        return Ok(());
    };
    let Some(mode) = driver.meta(&format!("native-spawn:{parent:?}:{k}"))? else {
        return Ok(());
    };
    if !driver.state()?.natives.contains_key(&(parent, k)) {
        let event = if path.len() > 1 {
            let Some(ancestor) = path_key(driver, parent, &path[..path.len() - 1])? else {
                return Ok(());
            };
            Event::Native(NativeEvent::StartNested {
                parent,
                key: k,
                ancestor,
                background: mode == "background",
            })
        } else {
            Event::Native(NativeEvent::Start {
                parent,
                key: k,
                background: mode == "background",
            })
        };
        driver.apply_if(&format!("native-start:{parent:?}:{k}"), event, &|state| {
            zeron_orch::step(state, event).is_ok()
        })?;
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
