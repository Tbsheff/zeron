use super::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Serialize)]
pub(super) struct NativeSource {
    native_key: String,
    native_path: Vec<String>,
    activation_id: Option<String>,
    provider_session_id: Option<String>,
    status: String,
    background: Option<bool>,
    progress_seen: bool,
}
#[derive(Clone, Serialize)]
pub(super) struct StreamSource {
    chat: String,
    provider_run_id: String,
    stream_seq: u64,
    provider_alive: Option<bool>,
    root_turn_active: Option<bool>,
    recovering: bool,
    provider_session_id: Option<String>,
    activation_id: Option<String>,
    #[serde(serialize_with = "serialize_natives")]
    natives: BTreeMap<String, NativeSource>,
}
pub(super) struct Observer {
    inner: Arc<Inner>,
    chat: String,
    run: String,
    closed: bool,
}
impl Observer {
    pub(super) fn open(inner: Arc<Inner>, chat: &str, run: &str, recovering: bool) -> Self {
        let observer = Self {
            inner,
            chat: chat.into(),
            run: run.into(),
            closed: false,
        };
        if observer.enabled() {
            lock(&observer.inner.provider_sources).insert(
                run.into(),
                StreamSource {
                    chat: chat.into(),
                    provider_run_id: run.into(),
                    stream_seq: 0,
                    provider_alive: None,
                    root_turn_active: None,
                    recovering,
                    provider_session_id: None,
                    activation_id: None,
                    natives: BTreeMap::new(),
                },
            );
            observer.emit("stream_opened", None, None, None, None);
        }
        observer
    }
    fn enabled(&self) -> bool {
        self.inner
            .orch
            .get()
            .is_some_and(|wire| wire.driver().mechanism_capture_enabled())
    }
    pub(super) fn event(&self, event: &AgentEvent) {
        if !self.enabled() {
            return;
        }
        let mut leaf = event;
        let mut path = Vec::new();
        while let AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } = leaf
        {
            path.push(parent_tool_use_id.clone());
            leaf = event;
        }
        let payload = serde_json::to_vec(event).unwrap_or_default();
        let hash = format!("{:x}", Sha256::digest(&payload));
        let encoded = serde_json::to_value(leaf).unwrap_or_default();
        let raw_kind = encoded["type"].as_str().unwrap_or("unknown").to_owned();
        let native_key = path.last().cloned();
        let mut kind = "stream_event";
        let mut status = None;
        {
            let mut streams = lock(&self.inner.provider_sources);
            let Some(stream) = streams.get_mut(&self.run) else {
                return;
            };
            stream.provider_alive = Some(true);
            if path.is_empty()
                && matches!(
                    leaf,
                    AgentEvent::TextDelta { .. } | AgentEvent::ToolCall { .. }
                )
            {
                stream.root_turn_active = Some(true);
            }
            if let Some(key) = &native_key {
                let native = stream
                    .natives
                    .entry(path_id(&path))
                    .or_insert_with(|| NativeSource {
                        native_key: key.clone(),
                        native_path: path.clone(),
                        activation_id: None,
                        provider_session_id: None,
                        status: "running".into(),
                        background: None,
                        progress_seen: false,
                    });
                if native.status == "spawned" {
                    native.status = "running".into();
                }
                kind = "native_progress";
                match leaf {
                    AgentEvent::SessionStarted {
                        session_id,
                        assistant_message_id,
                        ..
                    } => {
                        native.activation_id = Some(assistant_message_id.clone());
                        native.provider_session_id = Some(session_id.clone());
                        kind = "native_started";
                    }
                    AgentEvent::Done { status: done, .. } => {
                        let word = if *done == DoneStatus::Completed {
                            "completed"
                        } else if *done == DoneStatus::Interrupted {
                            "interrupted"
                        } else {
                            "failed"
                        };
                        native.status = word.into();
                        status = Some(word.to_owned());
                        kind = "native_terminal";
                    }
                    _ => native.progress_seen = true,
                }
            } else {
                match leaf {
                    AgentEvent::Done { status: done, .. } => {
                        stream.root_turn_active = Some(false);
                        kind = "root_terminal";
                        status = Some(
                            if *done == DoneStatus::Completed {
                                "completed"
                            } else if *done == DoneStatus::Interrupted {
                                "interrupted"
                            } else {
                                "failed"
                            }
                            .to_owned(),
                        );
                    }
                    AgentEvent::Steered { .. } => {
                        stream.root_turn_active = Some(true);
                        kind = "root_started";
                    }
                    AgentEvent::SessionStarted {
                        session_id,
                        assistant_message_id,
                        ..
                    } => {
                        stream.provider_alive = Some(true);
                        stream.root_turn_active = Some(true);
                        stream.provider_session_id = Some(session_id.clone());
                        stream.activation_id = Some(assistant_message_id.clone());
                        stream.recovering = false;
                        kind = "session_started";
                    }
                    AgentEvent::TextDelta { .. } => {
                        stream.root_turn_active = Some(true);
                        kind = "root_output";
                    }
                    _ => {}
                }
            }
        }
        let tool = match leaf {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => {
                let background = match call {
                    zeron_proto::ToolCall::Unknown {
                        input: Some(input), ..
                    }
                    | zeron_proto::ToolCall::Mcp {
                        input: Some(input), ..
                    } => input
                        .get("run_in_background")
                        .or_else(|| input.get("background"))
                        .and_then(|v| v.as_bool()),
                    _ => None,
                };
                let mut child_path = path.clone();
                child_path.push(id.clone());
                if let Some(stream) = lock(&self.inner.provider_sources).get_mut(&self.run) {
                    stream
                        .natives
                        .entry(path_id(&child_path))
                        .or_insert_with(|| NativeSource {
                            native_key: id.clone(),
                            native_path: child_path.clone(),
                            activation_id: None,
                            provider_session_id: None,
                            status: "spawned".into(),
                            background,
                            progress_seen: false,
                        });
                }
                kind = "native_spawn";
                Some(child_path)
            }
            _ => (!path.is_empty()).then_some(path),
        };
        self.emit(kind, Some(raw_kind), tool, status, Some(hash));
    }
    pub(super) fn close(&mut self, crashed: bool) {
        if self.closed {
            return;
        }
        self.closed = true;
        if !self.enabled() {
            return;
        }
        if let Some(stream) = lock(&self.inner.provider_sources).get_mut(&self.run) {
            stream.provider_alive = None;
            stream.root_turn_active = None;
        }
        self.emit(
            if crashed {
                "stream_crashed"
            } else {
                "stream_closed"
            },
            None,
            None,
            None,
            None,
        );
    }
    fn emit(
        &self,
        kind: &str,
        raw_kind: Option<String>,
        native_path: Option<Vec<String>>,
        status: Option<String>,
        payload_hash: Option<String>,
    ) {
        let Some(wire) = self.inner.orch.get() else {
            return;
        };
        let active: Vec<_> = lock(&self.inner.runs)
            .values()
            .map(|handle| handle.run_id.clone())
            .collect();
        let (state, streams, complete) = {
            let mut sources = lock(&self.inner.provider_sources);
            let complete = active.iter().all(|id| sources.contains_key(id));
            let Some(state) = sources.get_mut(&self.run) else {
                return;
            };
            state.stream_seq += 1;
            (
                state.clone(),
                sources.values().cloned().collect::<Vec<_>>(),
                complete,
            )
        };
        let event_id = format!("{}:{}", self.run, state.stream_seq);
        let native_key = native_path.as_ref().and_then(|path| path.last());
        let native = native_path
            .as_ref()
            .and_then(|path| state.natives.get(&path_id(path)));
        let data = serde_json::json!({"chat":self.chat,"provider_run_id":self.run,"stream_seq":state.stream_seq,"event_id":event_id,"event_kind":kind,"raw_event_kind":raw_kind,"parent_tool_id":native_key,"native_key":native_key,"native_path":native_path,"background":native.and_then(|n|n.background),"activation_id":native.and_then(|n|n.activation_id.as_ref()),"status":status,"payload_hash":payload_hash,"replay_of":null,"observer":state});
        let saved = wire
            .driver()
            .put_meta(
                &format!("provider-observer:{}", self.run),
                &serde_json::to_string(&data).unwrap_or_default(),
            )
            .and_then(|()| {
                wire.driver().observe_mechanism(
                    &format!("provider-event:{event_id}"),
                    "provider_event",
                    data,
                )
            })
            .and_then(|()| {
                wire.driver().observe_mechanism(
                    &format!("provider-catalog:{event_id}"),
                    "provider_stream_catalog",
                    serde_json::json!({"chat":self.chat,"streams":streams,"complete":complete}),
                )
            });
        if let Err(error) = saved {
            tracing::warn!(chat=%self.chat,%error,"provider observation save failed");
        }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        if self.closed || !self.enabled() {
            return;
        }
        if let Some(stream) = lock(&self.inner.provider_sources).get_mut(&self.run) {
            stream.provider_alive = None;
            stream.root_turn_active = None;
        }
        self.emit(
            "session_closed",
            Some("consumer_drop".into()),
            None,
            None,
            None,
        );
    }
}

pub(super) fn initial_catalog(inner: &Inner, wire: &OrchWire) {
    if !wire.driver().mechanism_capture_enabled() {
        return;
    }
    let active: Vec<_> = lock(&inner.runs)
        .values()
        .map(|handle| handle.run_id.clone())
        .collect();
    let streams = lock(&inner.provider_sources)
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let complete = active
        .iter()
        .all(|id| streams.iter().any(|stream| &stream.provider_run_id == id));
    if let Err(error)=wire.driver().observe_mechanism(&format!("provider-catalog:enable:{}",new_id()),"provider_stream_catalog",serde_json::json!({"chat":null,"scope":"device","streams":streams,"complete":complete,"unobserved_streams":active.into_iter().filter(|id|!lock(&inner.provider_sources).contains_key(id)).collect::<Vec<_>>()})) {
        tracing::warn!(%error,"initial provider catalog save failed");
    }
}

fn serialize_natives<S: serde::Serializer>(
    natives: &BTreeMap<String, NativeSource>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    natives.values().collect::<Vec<_>>().serialize(serializer)
}

fn path_id(path: &[String]) -> String {
    serde_json::to_string(path).expect("tool path serializes")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_paths_keep_reused_local_ids_distinct() {
        let first = vec!["parent-a".to_owned(), "local".to_owned()];
        let second = vec!["parent-b".to_owned(), "local".to_owned()];
        assert_ne!(path_id(&first), path_id(&second));
        assert_ne!(path_id(&["parent-a/local".into()]), path_id(&first));
        let mut natives = BTreeMap::new();
        for path in [first, second] {
            natives.insert(
                path_id(&path),
                NativeSource {
                    native_key: "local".into(),
                    native_path: path,
                    activation_id: None,
                    provider_session_id: None,
                    status: "running".into(),
                    background: None,
                    progress_seen: true,
                },
            );
        }
        let encoded = serde_json::to_value(natives.values().collect::<Vec<_>>()).unwrap();
        assert_eq!(encoded.as_array().unwrap().len(), 2);
        assert_ne!(encoded[0]["native_path"], encoded[1]["native_path"]);
        assert_eq!(encoded[0]["native_key"], encoded[1]["native_key"]);
    }
}

pub(super) fn native_doc_commit(
    inner: &Arc<Inner>,
    chat: &str,
    key: u64,
    binding: &crate::orchestrator::native::NativeBinding,
    owner_bytes: &[u8],
    native_bytes: &[u8],
) -> Result<(), EngineError> {
    let Some(wire) = inner
        .orch
        .get()
        .filter(|w| w.driver().mechanism_capture_enabled())
    else {
        return Ok(());
    };
    let raw = loro::LoroDoc::new();
    raw.import(owner_bytes)
        .map_err(|e| EngineError::Other(e.to_string()))?;
    let doc = SessionDoc::from_doc(raw);
    let chip = doc
        .read_entries()?
        .into_iter()
        .flat_map(|entry| entry.parts)
        .find_map(|part| match part {
            MessagePart::Tool {
                id,
                subagent_status,
                subagent_ref,
                ..
            } if id == binding.tool_id => Some((subagent_status, subagent_ref)),
            _ => None,
        })
        .ok_or_else(|| EngineError::Other("saved native chip missing".into()))?;
    let owner_hash = format!("{:x}", Sha256::digest(owner_bytes));
    let native_hash = format!("{:x}", Sha256::digest(native_bytes));
    wire.driver().observe_mechanism(&format!("native-doc:{key}:{owner_hash}:{native_hash}"),"native_doc_commit",serde_json::json!({"chat":chat,"task_key":key,"native_path":binding.path,"tool_id":binding.tool_id,"doc_id":binding.doc_id,"chip_doc_id":binding.chip_doc_id,"chip_status":chip.0,"chip_ref":chip.1,"doc_hash":native_hash,"chip_doc_hash":owner_hash})).map_err(|e|EngineError::Other(e.to_string()))?;
    Ok(())
}
