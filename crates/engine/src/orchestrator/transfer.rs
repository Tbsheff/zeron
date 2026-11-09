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
    for old in transfers.values_mut().filter(|t| {
        t.target == target_chat
            && t.status == TransferStatus::Pending
            && !t.id.starts_with("recovery:")
    }) {
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
    consume_selected(target, delivery_ref, run_input, limit, None)
}
fn consume_selected(
    target: &SessionDoc,
    delivery_ref: &str,
    run_input: &str,
    limit: usize,
    selected: Option<&str>,
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
    if target
        .doc()
        .get_map("meta")
        .get("orchTransferCommit")
        .is_some()
    {
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
        .find(|(id, t)| {
            selected.is_none_or(|selected| id.as_str() == selected)
                && matches!(
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
    fn local_seen_point_does_not_skip_unconfirmed_external_context() {
        let doc = SessionDoc::init("b").unwrap();
        pin(
            &doc,
            "external",
            "a",
            "b",
            None,
            "m1",
            &[entry(0), entry(1)],
        )
        .unwrap();
        consume(&doc, "mock:old", "first", 16000).unwrap();
        mark_uncertain_delivery(&doc, "mock", "old", "first").unwrap();
        doc.push_message(&entry(3)).unwrap();
        let provider = zeron_doc::ProviderSessionMark {
            session_id: "live".into(),
            cwd: "/fixture".into(),
            seen: Some("m3".into()),
        };
        let packet = prepare_run(
            &doc,
            RunContext {
                chat_id: "b",
                harness: "mock",
                input_id: "next",
                prompt: "next",
                used_tokens: 0,
                window: Some(32000),
                images: 0,
                other_attachments: 0,
                provider: Some(&provider),
                needs_history: true,
            },
        )
        .unwrap()
        .unwrap();
        assert!(packet.contains("text 1"));
    }
    #[test]
    fn committed_packet_is_authoritative_with_empty_ledger() {
        let doc = SessionDoc::init("b").unwrap();
        let ledger: BTreeMap<String, Transfer> = BTreeMap::new();
        let receipts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let runs = BTreeMap::from([(
            "mock:session:input".to_owned(),
            "immutable packet".to_owned(),
        )]);
        save(&doc, "orchTransferCommit", &(&ledger, &receipts, &runs)).unwrap();
        assert_eq!(
            consume(&doc, "mock:session", "input", 0).unwrap(),
            Some("immutable packet".into())
        );
    }
    #[test]
    fn recovery_rebuilds_primary_packet_from_committed_parts() {
        let doc = SessionDoc::init("b").unwrap();
        pin(&doc, "x", "a", "b", None, "m1", &[entry(0), entry(1)]).unwrap();
        consume(&doc, "mock:old", "first", 16000).unwrap();
        mark_uncertain_delivery(&doc, "mock", "old", "first").unwrap();
        let context = || RunContext {
            chat_id: "b",
            harness: "mock",
            input_id: "next",
            prompt: "next",
            used_tokens: 0,
            window: Some(32000),
            images: 0,
            other_attachments: 0,
            provider: None,
            needs_history: false,
        };
        let first = prepare_run(&doc, context()).unwrap();
        let (ledger, receipts, mut runs): (
            BTreeMap<String, Transfer>,
            BTreeMap<String, BTreeSet<String>>,
            BTreeMap<String, String>,
        ) = read(&doc, "orchTransferCommit");
        runs.remove("mock:fresh:next:next");
        save(&doc, "orchTransferCommit", &(&ledger, &receipts, &runs)).unwrap();
        save(&doc, "orchTransferRuns", &runs).unwrap();
        pin(
            &doc,
            "later",
            "later-source",
            "b",
            None,
            "m3",
            &[entry(2), entry(3)],
        )
        .unwrap();
        assert_eq!(prepare_run(&doc, context()).unwrap(), first);
        assert_eq!(status(&doc)["later"].status, TransferStatus::Pending);
        let (mut ledger, receipts, mut runs): (
            BTreeMap<String, Transfer>,
            BTreeMap<String, BTreeSet<String>>,
            BTreeMap<String, String>,
        ) = read(&doc, "orchTransferCommit");
        runs.remove("mock:fresh:next:next");
        let parts: BTreeMap<String, Vec<String>> = read(&doc, "orchTransferParts");
        ledger.remove(&parts["mock:fresh:next:next"][0]);
        save(&doc, "orchTransferCommit", &(&ledger, &receipts, &runs)).unwrap();
        save(&doc, "orchTransfers", &ledger).unwrap();
        save(&doc, "orchTransferRuns", &runs).unwrap();
        assert!(
            prepare_run(&doc, context())
                .unwrap_err()
                .contains("recovery part missing")
        );
    }
    #[test]
    fn recovered_budget_failure_stays_terminal_on_retry() {
        let doc = SessionDoc::init("b").unwrap();
        pin(&doc, "x", "a", "b", None, "m1", &[entry(0), entry(1)]).unwrap();
        consume(&doc, "mock:old", "first", 16000).unwrap();
        mark_uncertain_delivery(&doc, "mock", "old", "first").unwrap();
        let context = |window| RunContext {
            chat_id: "b",
            harness: "mock",
            input_id: "next",
            prompt: "next",
            used_tokens: 0,
            window: Some(window),
            images: 0,
            other_attachments: 0,
            provider: None,
            needs_history: true,
        };
        let first = prepare_run(&doc, context(16000)).unwrap_err();
        assert_eq!(prepare_run(&doc, context(32000)).unwrap_err(), first);
        let (_, _, runs): (
            BTreeMap<String, Transfer>,
            BTreeMap<String, BTreeSet<String>>,
            BTreeMap<String, String>,
        ) = read(&doc, "orchTransferCommit");
        assert!(!runs.contains_key("mock:fresh:next:next"));
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
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UncertainDelivery {
    input: String,
    retry_same_input: bool,
}
pub fn uncertain_for_other_input(
    doc: &SessionDoc,
    harness: &str,
    session: &str,
    input: &str,
) -> bool {
    let uncertain: BTreeMap<String, UncertainDelivery> = read(doc, "orchTransferUncertain");
    uncertain
        .get(&format!("{harness}:{session}"))
        .is_some_and(|failed| !failed.retry_same_input || failed.input != input)
}
pub fn has_delivery_packet(doc: &SessionDoc, harness: &str, session: &str, input: &str) -> bool {
    let (_, _, runs): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    runs.get(&format!("{harness}:{session}:{input}"))
        .is_some_and(|packet| !packet.is_empty())
}
pub fn mark_uncertain_delivery(
    doc: &SessionDoc,
    harness: &str,
    session: &str,
    input: &str,
) -> Result<bool, String> {
    let (_, _, runs): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    if !runs
        .get(&format!("{harness}:{session}:{input}"))
        .is_some_and(|packet| !packet.is_empty())
    {
        return Ok(false);
    }
    let mut uncertain: BTreeMap<String, UncertainDelivery> = read(doc, "orchTransferUncertain");
    let key = format!("{harness}:{session}");
    if uncertain
        .get(&key)
        .is_some_and(|failed| !failed.retry_same_input || failed.input == input)
    {
        return Ok(false);
    }
    uncertain.insert(
        key,
        UncertainDelivery {
            input: input.to_owned(),
            retry_same_input: false,
        },
    );
    save(doc, "orchTransferUncertain", &uncertain)?;
    doc.mark_provider_switched().map_err(|e| e.to_string())?;
    Ok(true)
}
pub fn reject_delivery(
    doc: &SessionDoc,
    harness: &str,
    session: &str,
    input: &str,
) -> Result<(), String> {
    let mut uncertain: BTreeMap<String, UncertainDelivery> = read(doc, "orchTransferUncertain");
    uncertain.insert(
        format!("{harness}:{session}"),
        UncertainDelivery {
            input: input.to_owned(),
            retry_same_input: false,
        },
    );
    save(doc, "orchTransferUncertain", &uncertain)?;
    doc.mark_provider_switched().map_err(|e| e.to_string())
}
pub fn confirm_delivery(
    doc: &SessionDoc,
    harness: &str,
    session: &str,
    input: &str,
) -> Result<(), String> {
    let mut uncertain: BTreeMap<String, UncertainDelivery> = read(doc, "orchTransferUncertain");
    let (_, receipts, _): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = read(doc, "orchTransferCommit");
    let confirmed = receipts
        .get(&format!("{harness}:{session}"))
        .cloned()
        .unwrap_or_default();
    uncertain.retain(|delivery, _| {
        !receipts
            .get(delivery)
            .is_some_and(|ids| !ids.is_empty() && ids.is_subset(&confirmed))
    });
    uncertain.remove(&format!("{harness}:{session}"));
    uncertain.remove(&format!("{harness}:fresh:{input}"));
    uncertain.remove(&format!("{harness}:replacement:{input}"));
    save(doc, "orchTransferUncertain", &uncertain)?;
    Ok(())
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
    let failures: BTreeMap<String, String> = read(doc, "orchTransferFailures");
    if let Some(reason) = failures.get(&key) {
        return Err(reason.clone());
    }
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
    let uncertain: BTreeMap<String, UncertainDelivery> = read(doc, "orchTransferUncertain");
    let mut manifests: BTreeMap<String, Vec<String>> = read(doc, "orchTransferParts");
    let mut recovered = manifests.get(&key).cloned().unwrap_or_default();
    if !already_prepared && recovered.is_empty() {
        let mut ledger = transfers.clone();
        let mut seen = BTreeSet::new();
        for (delivery, mark) in &uncertain {
            if delivery == &delivery_ref && mark.input == context.input_id {
                continue;
            }
            let Some(ids) = committed.1.get(delivery) else {
                continue;
            };
            for old in committed.0.values().filter(|t| {
                t.status == TransferStatus::Consumed && t.selected.iter().any(|id| ids.contains(id))
            }) {
                let identity = serde_json::to_string(&(
                    &old.source,
                    &old.target,
                    &old.base,
                    &old.point,
                    &old.items,
                ))
                .map_err(|e| e.to_string())?;
                if !seen.insert(identity.clone()) {
                    continue;
                }
                use sha2::{Digest, Sha256};
                let stable = format!("{:x}", Sha256::digest(identity.as_bytes()));
                let id = format!(
                    "recovery:{}:{}:{}",
                    context.harness, context.input_id, stable
                );
                if !ledger.contains_key(&id) {
                    let mut replay = old.clone();
                    replay.id = id.clone();
                    if mark.input == context.input_id {
                        replay.status = TransferStatus::ResolvedPortable;
                    } else {
                        replay.status = TransferStatus::Pending;
                        replay.packet = None;
                        replay.selected.clear();
                    }
                    ledger.insert(id.clone(), replay);
                }
                recovered.push(id);
            }
        }
        if !recovered.is_empty() {
            save(doc, "orchTransfers", &ledger)?;
        }
    }
    if context.needs_history && !already_prepared && !pending && !manifests.contains_key(&key) {
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
            if base.as_deref() != Some(entries[end].id.as_str()) {
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
    }
    let limit = zeron_orch::transfer::budget(
        context.window.unwrap_or(32000),
        context.used_tokens,
        context.prompt.len(),
        context.images,
        context.other_attachments,
    );
    let packet = if already_prepared || recovered.is_empty() {
        consume(doc, &delivery_ref, context.input_id, limit)?
    } else {
        let mut packets = Vec::new();
        let mut remaining = limit;
        let ledger: BTreeMap<String, Transfer> = read(doc, "orchTransfers");
        let required = if let Some(saved) = manifests.get(&key) {
            saved.clone()
        } else {
            let mut required: BTreeSet<String> = recovered.into_iter().collect();
            required.extend(
                ledger
                    .iter()
                    .filter(|(_, t)| {
                        matches!(
                            t.status,
                            TransferStatus::Pending | TransferStatus::ResolvedPortable
                        )
                    })
                    .map(|(id, _)| id.clone()),
            );
            let required: Vec<_> = required.into_iter().collect();
            manifests.insert(key.clone(), required.clone());
            save(doc, "orchTransferParts", &manifests)?;
            required
        };
        for id in required {
            let part = ledger
                .get(&id)
                .ok_or_else(|| format!("context transfer recovery part missing: {id}"))?;
            if part
                .packet
                .as_ref()
                .is_some_and(|packet| packet.len() > remaining)
            {
                let reason = "context transfer failed: NoBudget".to_owned();
                let mut failures: BTreeMap<String, String> = read(doc, "orchTransferFailures");
                failures.insert(key.clone(), reason.clone());
                save(doc, "orchTransferFailures", &failures)?;
                return Err(reason);
            }
            let part_key = format!("{}:part:{}", context.input_id, id);
            match consume_selected(doc, &delivery_ref, &part_key, remaining, Some(&id)) {
                Ok(Some(packet)) => {
                    remaining = remaining
                        .checked_sub(packet.len())
                        .ok_or("context transfer packet exceeds budget")?;
                    packets.push(packet);
                }
                Ok(None) => return Err(format!("context transfer recovery packet missing: {id}")),
                Err(error) => {
                    let mut failures: BTreeMap<String, String> = read(doc, "orchTransferFailures");
                    failures.insert(key.clone(), error.clone());
                    save(doc, "orchTransferFailures", &failures)?;
                    return Err(error);
                }
            }
        }
        let packet = packets.join("");
        let (ledger, receipts, mut runs): (
            BTreeMap<String, Transfer>,
            BTreeMap<String, BTreeSet<String>>,
            BTreeMap<String, String>,
        ) = read(doc, "orchTransferCommit");
        runs.insert(key.clone(), packet.clone());
        save(doc, "orchTransferCommit", &(&ledger, &receipts, &runs))?;
        save(doc, "orchTransferRuns", &runs)?;
        Some(packet)
    };
    if packet.as_ref().is_some_and(|packet| !packet.is_empty()) {
        let session = delivery_ref
            .strip_prefix(&format!("{}:", context.harness))
            .unwrap_or(&delivery_ref);
        mark_uncertain_delivery(doc, context.harness, session, context.input_id)?;
    }
    Ok(packet.map(|packet| format!("{packet}\nCurrent request:\n{}", context.prompt)))
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

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LineagePoint {
    journal_revision: BTreeMap<String, i32>,
    last_item_id: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LineageKind {
    Fork,
    SubagentSpawn,
    ProviderSwitch,
    MergeBack,
    SubagentResult,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum LineageRole {
    User,
    Assistant,
    System,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageItem {
    source_chat: String,
    source_item_id: String,
    target_item_id: Option<String>,
    role: LineageRole,
    digest: String,
    validated: bool,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LineageReceipt {
    operation_id: String,
    receipt_id: String,
    kind: LineageKind,
    source_chat: String,
    target_chat: String,
    source_point: LineagePoint,
    base_point: Option<LineagePoint>,
    copied_items: Vec<LineageItem>,
    validated: bool,
}
impl LineageReceipt {
    fn valid(&self) -> bool {
        let point_valid = |point: &LineagePoint| {
            point
                .journal_revision
                .iter()
                .all(|(peer, counter)| peer.parse::<u64>().is_ok() && *counter >= 0)
        };
        point_valid(&self.source_point)
            && self.base_point.as_ref().is_none_or(point_valid)
            && self.copied_items.iter().all(|item| {
                item.digest.len() == 64
                    && item
                        .digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    }
}

pub fn record_lineage(
    target: &SessionDoc,
    source: &SessionDoc,
    operation_id: &str,
    kind: &str,
    source_chat: &str,
    target_chat: &str,
    point: &str,
    base: Option<&str>,
    entries: &[SessionMessageEntry],
) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    fn text(entry: &SessionMessageEntry) -> String {
        entry
            .parts
            .iter()
            .filter_map(|part| {
                if let MessagePart::Text { text, .. } = part {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    let mut receipts: BTreeMap<String, serde_json::Value> = read(target, "orchTransferLineage");
    if receipts.contains_key(operation_id) {
        return Ok(());
    }
    let target_entries = target.read_entries().map_err(|e| e.to_string())?;
    let copied: Vec<_> = entries.iter().map(|entry| {
        let actual = target_entries.iter().find(|actual|actual.id==entry.id);
        serde_json::json!({"source_chat":source_chat,"source_item_id":entry.id,"target_item_id":actual.map(|e|e.id.as_str()),"role":match entry.role {MessageRole::User=>"user",MessageRole::Assistant=>"assistant",MessageRole::System=>"system"},"digest":format!("{:x}",Sha256::digest(text(entry).as_bytes())),"validated":actual.is_some_and(|actual|text(actual)==text(entry)&&actual.role==entry.role)})
    }).collect();
    let version: BTreeMap<_, _> = source
        .doc()
        .state_vv()
        .iter()
        .map(|(peer, counter)| (peer.to_string(), *counter))
        .collect();
    let source_point = serde_json::json!({"journal_revision":version,"last_item_id":point});
    receipts.insert(operation_id.to_owned(),serde_json::json!({"operation_id":operation_id,"receipt_id":format!("lineage:{operation_id}"),"kind":kind,"source_chat":source_chat,"target_chat":target_chat,"source_point":source_point,"base_point":base.map(|id|serde_json::json!({"journal_revision":version,"last_item_id":id})),"copied_items":copied,"validated":kind!="fork"||copied.iter().all(|item|item["validated"]==true)}));
    save(target, "orchTransferLineage", &receipts)
}

fn packet_fact(t: &Transfer) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let Some(packet) = t.packet.as_deref() else {
        return serde_json::Value::Null;
    };
    if packet.is_empty() {
        return serde_json::json!({"header":null,"chunks":[],"remaining":0});
    }
    let Some(end) = packet.find('\n').map(|i| i + 1) else {
        return serde_json::json!({"header":null,"chunks":[],"remaining":packet.len()});
    };
    let header = &packet[..end];
    let counts = header
        .split_once(". Selected ")
        .and_then(|(_, tail)| tail.split_once(" intact items; omitted "))
        .and_then(|(selected, tail)| {
            tail.split_once(". ").and_then(|(omitted, _)| {
                Some((
                    selected.parse::<usize>().ok()?,
                    omitted.parse::<usize>().ok()?,
                ))
            })
        });
    let header = counts.map(|(selected,omitted)|serde_json::json!({"selected":selected,"omitted":omitted,"bytes":end,"digest":format!("{:x}",Sha256::digest(header.as_bytes()))}));
    let mut cursor = end;
    let mut chunks = Vec::new();
    for item in t.items.iter().filter(|item| t.selected.contains(&item.id)) {
        let len = format!("[{} {}]\n{}\n", item.role, item.id, item.text).len();
        let stop = cursor.saturating_add(len).min(packet.len());
        let bytes = &packet.as_bytes()[cursor..stop];
        chunks.push(serde_json::json!({"id":item.id,"bytes":bytes.len(),"digest":format!("{:x}",Sha256::digest(bytes))}));
        cursor = stop;
    }
    serde_json::json!({"header":header,"chunks":chunks,"remaining":packet.len()-cursor})
}

pub fn durable_fact(doc: &SessionDoc, chat: &str) -> Result<serde_json::Value, String> {
    use sha2::{Digest, Sha256};
    fn digest(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
    fn captured<T: serde::de::DeserializeOwned + Default>(
        doc: &SessionDoc,
        key: &str,
    ) -> Result<T, String> {
        match doc.doc().get_map("meta").get(key) {
            None => Ok(T::default()),
            Some(ValueOrContainer::Value(LoroValue::String(value))) => {
                serde_json::from_str(&value).map_err(|e| format!("invalid {key}: {e}"))
            }
            _ => Err(format!("invalid {key} metadata type")),
        }
    }
    let ledger: BTreeMap<String, Transfer> = captured(doc, "orchTransfers")?;
    let (committed, receipts, runs): (
        BTreeMap<String, Transfer>,
        BTreeMap<String, BTreeSet<String>>,
        BTreeMap<String, String>,
    ) = captured(doc, "orchTransferCommit")?;
    fn redacted(ledger: BTreeMap<String, Transfer>) -> serde_json::Value {
        serde_json::Value::Object(ledger.into_iter().map(|(id,t)| {
            let items: Vec<_> = t.items.iter().map(|i| serde_json::json!({
                "id":i.id,"run":i.run,"role":i.role,"text_bytes":i.text.len(),"digest":digest(&i.text),"rendered_digest":digest(&format!("[{} {}]\n{}\n",i.role,i.id,i.text))
            })).collect();
            let status = match &t.status { TransferStatus::Failed(_) => "Failed".to_owned(), other => format!("{other:?}") };
            (id, serde_json::json!({"id":t.id,"source":t.source,"target":t.target,
                "base":t.base,"point":t.point,"items":items,"status":status,
                "packet":t.packet.as_deref().map(digest),"packet_bytes":t.packet.as_ref().map_or(0,|p|p.len()),"selected":t.selected,"observed_packet":packet_fact(&t)}))
        }).collect())
    }
    let runs: BTreeMap<_, _> = runs
        .into_iter()
        .map(|(key, packet)| (key, digest(&packet)))
        .collect();
    let rejected: BTreeSet<String> = captured(doc, "orchTransferRejected")?;
    let uncertain: BTreeMap<String, UncertainDelivery> = captured(doc, "orchTransferUncertain")?;
    let failures: BTreeMap<String, String> = captured(doc, "orchTransferFailures")?;
    let failures: BTreeMap<_, _> = failures
        .into_iter()
        .map(|(key, reason)| (key, digest(&reason)))
        .collect();
    let revision: u64 = doc
        .doc()
        .state_vv()
        .iter()
        .map(|(_, counter)| *counter as u64)
        .sum();
    let lineage: BTreeMap<String, LineageReceipt> = captured(doc, "orchTransferLineage")?;
    if lineage.values().any(|receipt| !receipt.valid()) {
        return Err("invalid lineage receipt fields".into());
    }
    let providers: BTreeMap<String, zeron_doc::ProviderSessionMark> =
        captured(doc, "providerSessions")?;
    let providers: BTreeMap<_,_> = providers.into_iter().map(|(h,p)|(h,serde_json::json!({"session":p.session_id,"cwd_digest":digest(&p.cwd),"seen":p.seen}))).collect();
    let queue: Vec<_> = doc.read_queue().map_err(|e|e.to_string())?.into_iter().map(|q|serde_json::json!({"id":q.id,"hold_for_turn_end":q.hold_for_turn_end,"gated":q.delivery_gate.is_some(),"issued_by":q.issued_by})).collect();
    let version: BTreeMap<String, i32> = doc
        .doc()
        .state_vv()
        .iter()
        .map(|(peer, counter)| (peer.to_string(), *counter))
        .collect();
    let entries: Vec<_> = doc.read_entries().map_err(|e|e.to_string())?.into_iter().map(|e|serde_json::json!({"id":e.id,"role":match e.role {MessageRole::User=>"user",MessageRole::Assistant=>"assistant",MessageRole::System=>"system"},"complete":e.status==Some(zeron_doc::MessageStatus::Complete),"digest":digest(&serde_json::to_string(&e).unwrap_or_default())})).collect();
    Ok(
        serde_json::json!({"chat_id":chat,"revision":revision,"version":version,"lineage":lineage,"ledger":redacted(ledger),"committed":redacted(committed),
        "receipts":receipts,"runs":runs,"rejected":rejected,"failures":failures,"uncertain":uncertain,"providers":providers,"queue":queue,"entries":entries,"durable":true}),
    )
}

pub fn freeze_source(
    driver: &super::driver::Driver,
    host: &crate::doc_host::DocHost,
    handle: &crate::doc_host::ChatDocHandle,
    chat: &str,
) -> Result<SessionDoc, String> {
    use sha2::{Digest, Sha256};
    if !driver.mechanism_capture_enabled() {
        return Err("source capture is disabled".into());
    }
    let bytes = host
        .persist_fork_snapshot(handle)
        .map_err(|e| e.to_string())?;
    let raw = loro::LoroDoc::new();
    raw.import(&bytes).map_err(|e| e.to_string())?;
    let frozen = SessionDoc::from_doc(raw);
    if frozen.chat_id().as_deref() != Some(chat) {
        return Err("source snapshot identity mismatch".into());
    }
    let data = durable_fact(&frozen, chat)?;
    let encoded = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    driver
        .observe_mechanism(
            &format!("transfer:{chat}:{:x}", Sha256::digest(encoded)),
            "transfer_state",
            data,
        )
        .map_err(|e| e.to_string())?;
    Ok(frozen)
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
    let bytes = host
        .persist_fork_snapshot(handle)
        .map_err(|e| e.to_string())?;
    let before = fact_from_snapshot(&bytes, chat)?;
    use sha2::{Digest, Sha256};
    let encoded = serde_json::to_vec(&before).map_err(|e| e.to_string())?;
    let id = format!("transfer:{chat}:{:x}", Sha256::digest(encoded));
    driver
        .observe_mechanism(&id, "transfer_state", before.clone())
        .map_err(|e| e.to_string())?;
    Ok(Some(before))
}
pub fn fact_from_snapshot(bytes: &[u8], chat: &str) -> Result<serde_json::Value, String> {
    let raw = loro::LoroDoc::new();
    raw.import(bytes).map_err(|e| e.to_string())?;
    durable_fact(&SessionDoc::from_doc(raw), chat)
}

fn operation_journal(
    bytes: &[u8],
    chat: &str,
    before: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, String> {
    let raw = loro::LoroDoc::new();
    raw.import(bytes).map_err(|e| e.to_string())?;
    let mut version = loro::VersionVector::new();
    let peers = before["version"]
        .as_object()
        .ok_or("missing before version")?;
    for (peer, counter) in peers {
        version.set_end(loro::ID::new(
            peer.parse().map_err(|_| "invalid peer")?,
            counter
                .as_i64()
                .ok_or("invalid counter")?
                .try_into()
                .map_err(|_| "counter overflow")?,
        ));
    }
    let end = raw.oplog_vv();
    let mut changes = raw
        .export_json_updates_without_peer_compression(&version, &end)
        .map_err(|e| e.to_string())?
        .changes;
    changes.sort_by_key(|change| (change.lamport, change.id.peer, change.id.counter));
    let doc = SessionDoc::from_doc(raw);
    let mut previous = before.clone();
    let mut journal = Vec::new();
    for change in changes {
        if change
            .deps
            .iter()
            .any(|id| version.get(&id.peer).copied().unwrap_or(0) <= id.counter)
        {
            return Err("noncausal transfer journal".into());
        }
        let from_version = previous["version"].clone();
        let len: i32 = change.op_len().try_into().map_err(|_| "change overflow")?;
        version.set_end(loro::ID::new(
            change.id.peer,
            change
                .id
                .counter
                .checked_add(len)
                .ok_or("change overflow")?,
        ));
        doc.doc()
            .checkout(&doc.doc().vv_to_frontiers(&version))
            .map_err(|e| e.to_string())?;
        let next = durable_fact(&doc, chat)?;
        let changed: Vec<_> = [
            "ledger",
            "committed",
            "receipts",
            "runs",
            "rejected",
            "failures",
            "uncertain",
            "providers",
            "queue",
            "entries",
        ]
        .into_iter()
        .filter(|key| previous[*key] != next[*key])
        .collect();
        let kind = match changed.as_slice() {
            [] => "NoTransferChange",
            ["entries"] => "JournalChange",
            ["queue"] => "QueueChange",
            ["providers"] => "ProviderChange",
            _ => "MixedChange",
        };
        journal.push(serde_json::json!({"id":format!("{}@{}",change.id.counter,change.id.peer),"lamport":change.lamport,"deps":change.deps.iter().map(|id|format!("{}@{}",id.counter,id.peer)).collect::<Vec<_>>(),"from_version":from_version,"to_version":next["version"],"kind":kind,"before":previous,"after":next}));
        previous = next;
    }
    if previous["version"]
        != serde_json::to_value(
            end.iter()
                .map(|(peer, counter)| (peer.to_string(), *counter))
                .collect::<BTreeMap<_, _>>(),
        )
        .map_err(|e| e.to_string())?
    {
        return Err("incomplete transfer journal".into());
    }
    Ok(journal)
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
    let handle = host.open(chat).map_err(|e| e.to_string())?;
    let bytes = host
        .persist_fork_snapshot(&handle)
        .map_err(|e| e.to_string())?;
    let after = fact_from_snapshot(&bytes, chat)?;
    let journal = operation_journal(&bytes, chat, &before)?;
    let data = serde_json::json!({"operation":operation,"args":args,"before":before,"after":after,"succeeded":succeeded,"journal":journal});
    let encoded = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    let id = format!("transfer-operation:{chat}:{:x}", Sha256::digest(encoded));
    driver
        .observe_mechanism(&id, "transfer_operation", data)
        .map_err(|e| e.to_string())
}
pub fn source_fact(entries: &[SessionMessageEntry]) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    let rows: Vec<_> = items(entries).into_iter().zip(entries).map(|(i,e)|serde_json::json!({
        "id":i.id,"run":i.run,"role":i.role,"complete":e.status==Some(zeron_doc::MessageStatus::Complete),"text_bytes":i.text.len(),"digest":format!("{:x}",Sha256::digest(i.text.as_bytes())),"rendered_digest":format!("{:x}",Sha256::digest(format!("[{} {}]\n{}\n",i.role,i.id,i.text).as_bytes()))
    })).collect();
    serde_json::json!(rows)
}

pub fn capture_initial_catalog(
    tx: &rusqlite::Transaction<'_>,
) -> Result<String, super::DriverError> {
    use sha2::{Digest, Sha256};
    let mut stmt = tx.prepare("SELECT doc_id,bytes FROM snapshots ORDER BY doc_id")?;
    let snapshots = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut chats = Vec::new();
    let mut unknown = Vec::new();
    for (id, bytes) in snapshots {
        if id == zeron_doc::REGISTRY_DOC_ID {
            continue;
        }
        let raw = loro::LoroDoc::new();
        if raw.import(&bytes).is_err() {
            unknown.push(serde_json::json!({"id":id,"reason":"decode","digest":format!("{:x}",Sha256::digest(&bytes))}));
            continue;
        }
        let doc = SessionDoc::from_doc(raw);
        if let Some(chat) = doc.chat_id() {
            if id == format!("{chat}.pre-chat2") {
                continue;
            }
            if chat != id {
                unknown.push(serde_json::json!({"id":id,"reason":"chat_identity"}));
                continue;
            }
            match durable_fact(&doc, &chat) {
                Ok(data) => chats.push(data),
                Err(_) => unknown.push(serde_json::json!({"id":id,"reason":"transfer_metadata"})),
            }
        } else {
            unknown.push(serde_json::json!({"id":id,"reason":"missing_chat_identity"}));
        }
    }
    let data = serde_json::json!({"scope":"durable_snapshots","chats":chats,"unknown":unknown});
    let encoded = serde_json::to_vec(&data)?;
    let id = format!("transfer-catalog:{:x}", Sha256::digest(encoded));
    super::mechanism::fact(tx, &id, "transfer_catalog", &data)?;
    Ok(id)
}

#[cfg(test)]
pub(crate) fn export_capture_for_test(driver: &super::driver::Driver, name: &str, scope: &str) {
    let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") else {
        return;
    };
    let dir =
        std::path::PathBuf::from(root).join(format!("transfer-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = format!("{}.jsonl", driver.device());
    let rows = driver.mechanism_lines().unwrap();
    std::fs::write(dir.join(&file), rows.join("\n") + "\n").unwrap();
    std::fs::write(dir.join("capture.json"),serde_json::to_vec(&serde_json::json!({"test_name":name,"scope":scope,"synthetic":true,"devices":[driver.device()],"filenames":[file],"capture":"present"})).unwrap()).unwrap();
}

#[cfg(test)]
mod capture_schema_tests {
    use super::*;
    #[test]
    fn lineage_capture_rejects_unknown_payload_fields() {
        let doc = SessionDoc::init("a").unwrap();
        let receipt = serde_json::json!({"operation_id":"o","receipt_id":"r","kind":"fork","source_chat":"a","target_chat":"b","source_point":{"journal_revision":{"1":1},"last_item_id":"m"},"base_point":null,"copied_items":[],"validated":true,"unknown_payload":"fixture must not be exported"});
        save(
            &doc,
            "orchTransferLineage",
            &BTreeMap::from([("o", receipt)]),
        )
        .unwrap();
        assert!(durable_fact(&doc, "a").is_err());
    }
    #[test]
    fn lineage_capture_rejects_non_hash_digest() {
        let doc = SessionDoc::init("a").unwrap();
        let receipt = serde_json::json!({"operation_id":"o","receipt_id":"r","kind":"fork","source_chat":"a","target_chat":"b","source_point":{"journal_revision":{"1":1},"last_item_id":"m"},"base_point":null,"copied_items":[{"source_chat":"a","source_item_id":"m","target_item_id":"m","role":"assistant","digest":"fixture must not be exported","validated":true}],"validated":true});
        save(
            &doc,
            "orchTransferLineage",
            &BTreeMap::from([("o", receipt)]),
        )
        .unwrap();
        assert!(durable_fact(&doc, "a").is_err());
    }
}
