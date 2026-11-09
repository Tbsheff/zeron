use loro::{LoroValue, ValueOrContainer};
use std::collections::{BTreeMap, BTreeSet};
use zeron_doc::{MessagePart, MessageRole, SessionDoc, SessionMessageEntry};
use zeron_orch::transfer::{Item, Transfer, TransferStatus};

fn read<T: serde::de::DeserializeOwned + Default>(doc: &SessionDoc, key: &str) -> T {
    match doc.doc().get_map("meta").get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(value))) => {
            serde_json::from_str(&value).unwrap_or_default()
        }
        _ => T::default(),
    }
}
fn save<T: serde::Serialize>(doc: &SessionDoc, key: &str, value: &T) -> Result<(), String> {
    doc.doc()
        .get_map("meta")
        .insert(
            key,
            serde_json::to_string(value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    doc.doc().commit();
    Ok(())
}
pub fn items(entries: &[SessionMessageEntry]) -> Vec<Item> {
    entries
        .iter()
        .map(|e| Item {
            id: e.id.clone(),
            run: e.continuation_of.clone().unwrap_or_else(|| e.id.clone()),
            role: match e.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => "system",
            }
            .into(),
            text: e
                .parts
                .iter()
                .filter_map(|p| match p {
                    MessagePart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .collect()
}
pub fn pin(
    target: &SessionDoc,
    id: &str,
    source_chat: &str,
    target_chat: &str,
    base: Option<String>,
    point: &str,
    entries: &[SessionMessageEntry],
) -> Result<Transfer, String> {
    let mut transfers: BTreeMap<String, Transfer> = read(target, "orchTransfers");
    if let Some(old) = transfers.get(id) {
        return Ok(old.clone());
    }
    let transfer = Transfer::pin(
        id.into(),
        source_chat.into(),
        target_chat.into(),
        base,
        point.into(),
        &items(entries),
    )
    .map_err(|e| format!("transfer: {e:?}"))?;
    for old in transfers
        .values_mut()
        .filter(|t| t.target == target_chat && t.status == TransferStatus::Pending)
    {
        old.status = TransferStatus::Superseded;
    }
    transfers.insert(id.into(), transfer.clone());
    save(target, "orchTransfers", &transfers)?;
    Ok(transfer)
}
pub fn consume(
    target: &SessionDoc,
    delivery_ref: &str,
    run_input: &str,
    limit: usize,
) -> Result<Option<String>, String> {
    let committed: (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(target, "orchTransferCommit");
    let mut transfers: BTreeMap<String, Transfer> = read(target, "orchTransfers");
    let mut receipts: BTreeMap<String, BTreeSet<String>> = read(target, "orchTransferReceipts");
    let key = format!("{delivery_ref}:{run_input}");
    let mut failures: BTreeMap<String, String> = read(target, "orchTransferFailures");
    if let Some(reason) = failures.get(&key) {
        return Err(reason.clone());
    }
    let mut runs: BTreeMap<String, String> = read(target, "orchTransferRuns");
    if !committed.0.is_empty() {
        for (id, transfer) in committed.0 {
            if transfer.status == TransferStatus::Consumed {
                transfers.insert(id, transfer);
            }
        }
        receipts = committed.1;
        runs = committed.2;
    }
    if let Some(packet) = runs.get(&key) {
        return Ok(Some(packet.clone()));
    }
    let Some(id) = transfers
        .iter()
        .find(|(_, t)| {
            matches!(
                t.status,
                TransferStatus::Pending | TransferStatus::ResolvedPortable
            )
        })
        .map(|(id, _)| id.clone())
    else {
        return Ok(None);
    };
    let transfer = transfers.get_mut(&id).unwrap();
    if transfer.status == TransferStatus::Pending {
        if let Err(error) =
            transfer.resolve(limit, receipts.entry(delivery_ref.into()).or_default())
        {
            save(target, "orchTransfers", &transfers)?;
            let reason = format!("context transfer failed: {error:?}");
            failures.insert(key, reason.clone());
            save(target, "orchTransferFailures", &failures)?;
            marker(
                target,
                &format!("handoff-error:{id}"),
                MessagePart::Error {
                    id: format!("handoff-error:{id}"),
                    message: format!("Context transfer failed: {error:?}"),
                },
            )?;
            return Err(reason);
        }
    }
    let packet = transfer
        .consume()
        .map_err(|e| format!("transfer: {e:?}"))?
        .to_owned();
    let coverage = zeron_doc::ContextCoverage {
        transfer_id: transfer.id.clone(),
        source_chat_id: transfer.source.clone(),
        base_message_id: transfer.base.clone(),
        point_message_id: transfer.point.clone(),
        selected_message_ids: transfer.selected.clone(),
        omitted_count: transfer.items.len().saturating_sub(transfer.selected.len()),
    };
    marker(
        target,
        &format!("handoff:{id}"),
        MessagePart::Handoff {
            id: format!("handoff:{id}"),
            strategy: "portable_replay".into(),
            coverage,
        },
    )?;
    receipts
        .entry(delivery_ref.into())
        .or_default()
        .extend(transfer.selected.iter().cloned());
    runs.insert(key, packet.clone());
    save(
        target,
        "orchTransferCommit",
        &(&transfers, &receipts, &runs),
    )?;
    save(target, "orchTransfers", &transfers)?;
    save(target, "orchTransferReceipts", &receipts)?;
    save(target, "orchTransferRuns", &runs)?;
    Ok(Some(packet))
}
pub fn native_safe(
    source_host: &str,
    target_host: &str,
    source_harness: &str,
    target_harness: &str,
    strong: bool,
    supported: bool,
) -> bool {
    !source_host.is_empty()
        && source_host == target_host
        && source_harness == target_harness
        && strong
        && supported
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(n: usize) -> SessionMessageEntry {
        SessionMessageEntry {
            id: format!("m{n}"),
            role: if n % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            parts: vec![MessagePart::Text {
                id: format!("t{n}"),
                text: format!("text {n}"),
            }],
            created_at: n as i64,
            device_id: "host".into(),
            status: Some(zeron_doc::MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }
    #[test]
    fn persisted_snapshot_survives_source_change_and_retry() {
        let target = SessionDoc::init("b").unwrap();
        let mut source = vec![entry(0), entry(1)];
        pin(&target, "x", "a", "b", None, "m1", &source).unwrap();
        source[0].parts.clear();
        source.push(entry(2));
        let packet = consume(&target, "h", "input", 16000).unwrap().unwrap();
        assert!(packet.contains("text 0"));
        assert!(!packet.contains("text 2"));
        assert_eq!(consume(&target, "h", "input", 1).unwrap(), Some(packet));

        pin(&target, "y", "a", "b", None, "m2", &source).unwrap();
        let delta = consume(&target, "h", "input2", 16000).unwrap().unwrap();
        assert!(delta.contains("text 2"));
        assert!(!delta.contains("text 1"));
    }
    #[test]
    fn binds_fresh_delivery_and_excludes_current_input() {
        let doc = SessionDoc::init("a").unwrap();
        for entry in [entry(0), entry(1), entry(2)] {
            doc.push_message(&entry).unwrap();
        }
        let first = prepare_run(
            &doc,
            RunContext {
                chat_id: "a",
                harness: "h",
                input_id: "m2",
                prompt: "new request",
                used_tokens: 0,
                window: Some(32000),
                images: 0,
                other_attachments: 0,
                provider: None,
                needs_history: true,
            },
        )
        .unwrap()
        .unwrap();
        assert!(!first.contains("text 2"));
        assert!(first.ends_with("new request"));
        bind_provider_session(&doc, "h", "m2", "session").unwrap();
        pin(&doc, "again", "a", "a", None, "m1", &[entry(0), entry(1)]).unwrap();
        let packet = consume(&doc, "h:session", "next", 16000).unwrap().unwrap();
        assert!(!packet.contains("text 0"));
        assert!(!packet.contains("text 1"));
    }
    #[test]
    fn resume_rejection_replaces_delta_with_full_pinned_prefix() {
        let doc = SessionDoc::init("a").unwrap();
        for n in 0..6 {
            doc.push_message(&entry(n)).unwrap();
        }
        let old = zeron_doc::ProviderSessionMark {
            session_id: "old-a".into(),
            cwd: "/tmp".into(),
            seen: Some("m1".into()),
        };
        let initial = prepare_run(
            &doc,
            RunContext {
                chat_id: "a",
                harness: "a",
                input_id: "m4",
                prompt: "current",
                used_tokens: 0,
                window: Some(32000),
                images: 0,
                other_attachments: 0,
                provider: Some(&old),
                needs_history: true,
            },
        )
        .unwrap()
        .unwrap();
        assert!(!initial.contains("text 0"));
        assert!(initial.contains("text 3"));
        replace_provider_session(&doc, "a", "a", "m4").unwrap();
        let fresh = || RunContext {
            chat_id: "a",
            harness: "a",
            input_id: "m4",
            prompt: "current",
            used_tokens: 0,
            window: Some(32000),
            images: 0,
            other_attachments: 0,
            provider: None,
            needs_history: true,
        };
        let full = prepare_run(&doc, fresh()).unwrap().unwrap();
        assert!(full.contains("text 0"));
        assert!(full.contains("text 1"));
        assert!(!full.contains("text 4"));
        assert!(!full.contains("text 5"));
        assert_eq!(prepare_run(&doc, fresh()).unwrap().unwrap(), full);
        assert_eq!(status(&doc)["switch:a:m4"].status, TransferStatus::Consumed);
        assert_eq!(
            status(&doc)["replacement:a:m4"].status,
            TransferStatus::Consumed
        );
    }
    #[test]
    fn failed_resolution_leaves_an_error_marker() {
        let target = SessionDoc::init("b").unwrap();
        pin(&target, "x", "a", "b", None, "m1", &[entry(0), entry(1)]).unwrap();
        let reason = consume(&target, "h", "input", 0).unwrap_err();
        assert_eq!(consume(&target, "h", "input", 16000).unwrap_err(), reason);
        assert!(target.read_entries().unwrap().iter().any(|e| {
            e.parts
                .iter()
                .any(|p| matches!(p, MessagePart::Error { .. }))
        }));
        let transfers: BTreeMap<String, Transfer> = read(&target, "orchTransfers");
        assert!(matches!(transfers["x"].status, TransferStatus::Failed(_)));
    }
    #[test]
    fn native_requires_host_harness_strength_and_support() {
        assert!(native_safe("a", "a", "h", "h", true, true));
        assert!(!native_safe("a", "b", "h", "h", true, true));
        assert!(!native_safe("a", "a", "h", "h", false, true));
        assert!(!native_safe("a", "a", "h", "h", true, false));
    }
}

fn marker(doc: &SessionDoc, id: &str, part: MessagePart) -> Result<(), String> {
    if doc
        .read_entries()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|e| e.id == id)
    {
        return Ok(());
    }
    doc.push_message(&SessionMessageEntry {
        id: id.into(),
        role: MessageRole::System,
        parts: vec![part],
        created_at: 0,
        device_id: String::new(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })
    .map_err(|e| e.to_string())
}

pub struct RunContext<'a> {
    pub chat_id: &'a str,
    pub harness: &'a str,
    pub input_id: &'a str,
    pub prompt: &'a str,
    pub used_tokens: usize,
    pub window: Option<usize>,
    pub images: usize,
    pub other_attachments: usize,
    pub provider: Option<&'a zeron_doc::ProviderSessionMark>,
    pub needs_history: bool,
}
pub fn prepare_run(doc: &SessionDoc, context: RunContext<'_>) -> Result<Option<String>, String> {
    let rejected: BTreeSet<String> = read(doc, "orchTransferRejected");
    let replacement = rejected.contains(&format!("{}:{}", context.harness, context.input_id));
    let delivery_ref = context
        .provider
        .filter(|p| !p.session_id.is_empty())
        .map(|p| format!("{}:{}", context.harness, p.session_id))
        .unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                context.harness,
                if replacement { "replacement" } else { "fresh" },
                context.input_id
            )
        });
    let key = format!("{delivery_ref}:{}", context.input_id);
    let committed: (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    let already_prepared = committed.2.contains_key(&key);
    let transfers: BTreeMap<String, Transfer> = read(doc, "orchTransfers");
    let pending = transfers.values().any(|t| {
        matches!(
            t.status,
            TransferStatus::Pending | TransferStatus::ResolvedPortable
        )
    });
    if context.needs_history && !already_prepared && !pending {
        let entries = doc.read_entries().map_err(|e| e.to_string())?;
        let end = entries
            .iter()
            .position(|e| e.id == context.input_id)
            .unwrap_or(entries.len());
        let stable = entries[..end].iter().rposition(|e| {
            e.role == MessageRole::Assistant && e.status == Some(zeron_doc::MessageStatus::Complete)
        });
        if let Some(end) = stable {
            let base = context
                .provider
                .and_then(|p| p.seen.clone())
                .filter(|base| entries[..=end].iter().any(|e| &e.id == base));
            if base.as_deref() == Some(entries[end].id.as_str()) {
                return Ok(None);
            }
            pin(
                doc,
                &format!("switch:{}:{}", context.harness, context.input_id),
                context.chat_id,
                context.chat_id,
                base,
                &entries[end].id,
                &entries[..=end],
            )?;
        }
    }
    let limit = zeron_orch::transfer::budget(
        context.window.unwrap_or(32000),
        context.used_tokens,
        context.prompt.len(),
        context.images,
        context.other_attachments,
    );
    consume(doc, &delivery_ref, context.input_id, limit).map(|packet| {
        packet.map(|packet| format!("{packet}\nCurrent request:\n{}", context.prompt))
    })
}

pub fn status(doc: &SessionDoc) -> BTreeMap<String, Transfer> {
    read(doc, "orchTransfers")
}
pub fn fork_base(doc: &SessionDoc) -> Option<String> {
    let transfers = status(doc);
    transfers
        .values()
        .find(|t| t.id.starts_with("fork:") && t.source != t.target)
        .map(|t| t.point.clone())
}
pub fn merge_marker(doc: &SessionDoc, transfer: &Transfer) -> Result<(), String> {
    let id = format!("merge-marker:{}", transfer.id);
    marker(
        doc,
        &id,
        MessagePart::MergeBack {
            id: id.clone(),
            coverage: zeron_doc::ContextCoverage {
                transfer_id: transfer.id.clone(),
                source_chat_id: transfer.source.clone(),
                base_message_id: transfer.base.clone(),
                point_message_id: transfer.point.clone(),
                selected_message_ids: vec![],
                omitted_count: 0,
            },
        },
    )
}

pub fn bind_provider_session(
    doc: &SessionDoc,
    harness: &str,
    input_id: &str,
    session_id: &str,
) -> Result<(), String> {
    if session_id.is_empty() {
        return Ok(());
    }
    let (transfers, mut receipts, mut runs): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    let fresh = format!("{harness}:fresh:{input_id}");
    let actual = format!("{harness}:{session_id}");
    let replacement = format!("{harness}:replacement:{input_id}");
    if let Some(ids) = receipts.get(&replacement).cloned() {
        receipts.entry(actual.clone()).or_default().extend(ids);
    }
    if let Some(packet) = runs.get(&format!("{replacement}:{input_id}")).cloned() {
        runs.entry(format!("{actual}:{input_id}")).or_insert(packet);
    }
    if let Some(ids) = receipts.get(&fresh).cloned() {
        receipts.entry(actual.clone()).or_default().extend(ids);
    }
    if let Some(packet) = runs.get(&format!("{fresh}:{input_id}")).cloned() {
        runs.entry(format!("{actual}:{input_id}")).or_insert(packet);
    }
    save(doc, "orchTransferCommit", &(&transfers, &receipts, &runs))?;
    save(doc, "orchTransferReceipts", &receipts)?;
    save(doc, "orchTransferRuns", &runs)
}

pub fn attachment_counts(paths: &[String]) -> (usize, usize) {
    let images = paths
        .iter()
        .filter(|path| {
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "png"
                            | "jpg"
                            | "jpeg"
                            | "gif"
                            | "webp"
                            | "heic"
                            | "avif"
                            | "bmp"
                            | "tif"
                            | "tiff"
                    )
                })
        })
        .count();
    (images, paths.len() - images)
}

pub fn replace_provider_session(
    doc: &SessionDoc,
    chat: &str,
    harness: &str,
    input_id: &str,
) -> Result<(), String> {
    let entries = doc.read_entries().map_err(|e| e.to_string())?;
    let input = entries
        .iter()
        .position(|e| e.id == input_id)
        .unwrap_or(entries.len());
    let point = entries[..input].iter().rposition(|e| {
        e.role == MessageRole::Assistant && e.status == Some(zeron_doc::MessageStatus::Complete)
    });
    if let Some(point) = point {
        pin(
            doc,
            &format!("replacement:{harness}:{input_id}"),
            chat,
            chat,
            None,
            &entries[point].id,
            &entries[..=point],
        )?;
    }
    let mut rejected: BTreeSet<String> = read(doc, "orchTransferRejected");
    rejected.insert(format!("{harness}:{input_id}"));
    save(doc, "orchTransferRejected", &rejected)
}

pub fn durable_fact(doc: &SessionDoc, chat: &str) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    fn digest(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
    let ledger: BTreeMap<String, Transfer> = read(doc, "orchTransfers");
    let (committed, receipts, runs): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    fn redacted(ledger: BTreeMap<String, Transfer>) -> serde_json::Value {
        serde_json::Value::Object(ledger.into_iter().map(|(id,t)| {
            let items: Vec<_> = t.items.iter().map(|i| serde_json::json!({
                "id":i.id,"run":i.run,"role":i.role,"text_bytes":i.text.len(),"digest":digest(&i.text)
            })).collect();
            let status = match &t.status { TransferStatus::Failed(_) => "Failed".to_owned(), other => format!("{other:?}") };
            (id, serde_json::json!({"id":t.id,"source":t.source,"target":t.target,
                "base":t.base,"point":t.point,"items":items,"status":status,
                "packet":t.packet.as_deref().map(digest),"packet_bytes":t.packet.as_ref().map_or(0,|p|p.len()),"selected":t.selected}))
        }).collect())
    }
    let runs: BTreeMap<_, _> = runs
        .into_iter()
        .map(|(key, packet)| (key, digest(&packet)))
        .collect();
    let rejected: BTreeSet<String> = read(doc, "orchTransferRejected");
    let failures: BTreeMap<String, String> = read(doc, "orchTransferFailures");
    let failures: BTreeMap<_, _> = failures
        .into_iter()
        .map(|(key, reason)| (key, digest(&reason)))
        .collect();
    let revision: u64 = doc
        .doc()
        .oplog_vv()
        .iter()
        .map(|(_, counter)| *counter as u64)
        .sum();
    serde_json::json!({"chat_id":chat,"revision":revision,"ledger":redacted(ledger),"committed":redacted(committed),
        "receipts":receipts,"runs":runs,"rejected":rejected,"failures":failures,"durable":true})
}

pub fn begin_operation(
    driver: &super::driver::Driver,
    host: &crate::doc_host::DocHost,
    handle: &crate::doc_host::ChatDocHandle,
    chat: &str,
) -> Result<Option<serde_json::Value>, String> {
    if !driver.mechanism_capture_enabled() {
        return Ok(None);
    }
    host.persist_fork(handle).map_err(|e| e.to_string())?;
    saved_fact(host, chat).map(Some)
}
pub fn saved_fact(
    host: &crate::doc_host::DocHost,
    chat: &str,
) -> Result<serde_json::Value, String> {
    let bytes = host
        .docs_store()
        .load_snapshot(chat)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "saved transfer snapshot is missing".to_owned())?;
    let raw = loro::LoroDoc::new();
    raw.import(&bytes).map_err(|e| e.to_string())?;
    Ok(durable_fact(&SessionDoc::from_doc(raw), chat))
}
pub fn end_operation(
    driver: &super::driver::Driver,
    host: &crate::doc_host::DocHost,
    chat: &str,
    before: Option<serde_json::Value>,
    operation: &str,
    args: serde_json::Value,
    succeeded: bool,
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let Some(before) = before else {
        return Ok(());
    };
    let after = saved_fact(host, chat)?;
    let data = serde_json::json!({"operation":operation,"args":args,"before":before,"after":after,"succeeded":succeeded});
    let encoded = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    let id = format!("transfer-operation:{chat}:{:x}", Sha256::digest(encoded));
    driver
        .observe_mechanism(&id, "transfer_operation", data)
        .map_err(|e| e.to_string())
}
pub fn source_fact(entries: &[SessionMessageEntry]) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let rows: Vec<_> = items(entries).into_iter().map(|i|serde_json::json!({
        "id":i.id,"run":i.run,"role":i.role,"text_bytes":i.text.len(),"digest":format!("{:x}",Sha256::digest(i.text.as_bytes()))
    })).collect();
    serde_json::json!(rows)
}
