use super::*;

pub(super) fn fold(
    inner: &Arc<Inner>,
    chat: &str,
    path: &[String],
    event: &AgentEvent,
    sinks: &mut HashMap<Vec<String>, SubagentSink>,
    device: &str,
) -> Result<bool, EngineError> {
    if path.len() < 2 {
        return Ok(true);
    }
    let Some(wire) = inner.orch.get() else {
        return Ok(false);
    };
    if !wire
        .native_path_known(chat, path)
        .map_err(|e| EngineError::Other(e.to_string()))?
    {
        return Ok(false);
    }
    wire.native_activate_path(chat, path)
        .map_err(|e| EngineError::Other(e.to_string()))?;
    if !wire
        .native_path_running(chat, path)
        .map_err(|e| EngineError::Other(e.to_string()))?
    {
        return Ok(false);
    }
    let host = inner
        .doc_host()
        .ok_or_else(|| EngineError::Other("native document host unavailable".into()))?;
    let doc_id = crate::orchestrator::native::doc_id_path(chat, path);
    if !sinks.contains_key(path) {
        let handle = host.open(&doc_id)?;
        sinks.insert(
            path.to_vec(),
            SubagentSink {
                doc_id,
                doc: handle.writer(),
                entry_id: new_id(),
                started_at: now_ms(),
                entry_index: None,
                written: Vec::new(),
                folded: Vec::new(),
                dirty: false,
            },
        );
    }
    let sink = sinks.get_mut(path).expect("native sink exists");
    if let AgentEvent::UserMessage { text } = event {
        sink.push_user(device, text);
    } else {
        fold_event_into_parts(&mut sink.folded, event);
        sink.dirty = true;
    }
    if let AgentEvent::Done { status, .. } = event {
        let sink = sinks.remove(path).expect("native sink exists");
        finish(
            inner,
            chat,
            path,
            sink,
            device,
            if *status == DoneStatus::Interrupted {
                MessageStatus::Aborted
            } else {
                MessageStatus::Complete
            },
            *status != DoneStatus::Completed,
        )?;
    }
    Ok(true)
}
pub(super) fn finish(
    inner: &Arc<Inner>,
    chat: &str,
    path: &[String],
    sink: SubagentSink,
    device: &str,
    status: MessageStatus,
    failed: bool,
) -> Result<(), EngineError> {
    let doc_id = sink.doc_id.clone();
    let json = sink
        .finish(device, status)
        .ok_or_else(|| EngineError::Other("native final write failed".into()))?;
    let host = inner
        .doc_host()
        .ok_or_else(|| EngineError::Other("native document host unavailable".into()))?;
    let handle = host.open(&doc_id)?;
    if let Some(wire) = inner.orch.get() {
        wire.stage_native_terminal_path(
            chat,
            path,
            &doc_id,
            handle.doc().export_snapshot()?,
            failed,
        )
        .map_err(|e| EngineError::Other(e.to_string()))?;
    }
    host.persist_fork(&handle)?;
    host.upload_tool_sidecar(
        chat,
        zeron_doc::SidecarPayload {
            part_id: doc_id,
            output: Some(json),
            diff: None,
        },
    );
    Ok(())
}
