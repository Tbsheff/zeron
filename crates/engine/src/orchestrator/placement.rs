use zeron_doc::{
    DocError, OrchMessage, SessionCommandEntry, SessionCommandPayload, SessionCommandStatus,
    SessionDoc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRoute {
    pub chat: String,
    pub host: String,
    pub parent: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("unknown chat or registry unavailable")]
    Unknown,
    #[error("route or host mismatch")]
    Spoofed,
    #[error("device does not support orch-v2")]
    Unsupported,
    #[error(transparent)]
    Doc(#[from] DocError),
}

pub trait Registry {
    fn chat(&self, chat: &str) -> Option<ChatRoute>;
    fn supports_orch(&self, device: &str) -> bool;
}

pub fn require_capability(registry: &dyn Registry, device: &str) -> Result<(), RouteError> {
    if registry.supports_orch(device) {
        Ok(())
    } else {
        Err(RouteError::Unsupported)
    }
}

pub fn validate(
    registry: &dyn Registry,
    local_device: &str,
    room: &str,
    message: &OrchMessage,
) -> Result<(), RouteError> {
    let sender = registry
        .chat(&message.sender_chat)
        .ok_or(RouteError::Unknown)?;
    let target = registry
        .chat(&message.target_chat)
        .ok_or(RouteError::Unknown)?;
    if sender.host != message.sender_device
        || target.host != local_device
        || message.id != message.expected_id()
    {
        return Err(RouteError::Spoofed);
    }
    let valid = if message.upward() {
        room == sender.chat && sender.parent.as_deref() == Some(target.chat.as_str())
    } else {
        room == target.chat && target.parent.as_deref() == Some(sender.chat.as_str())
    };
    if !valid {
        return Err(RouteError::Spoofed);
    }
    Ok(())
}

pub fn send(
    registry: &dyn Registry,
    local_device: &str,
    doc: &SessionDoc,
    message: &OrchMessage,
) -> Result<bool, RouteError> {
    let sender = registry
        .chat(&message.sender_chat)
        .ok_or(RouteError::Unknown)?;
    let target = registry
        .chat(&message.target_chat)
        .ok_or(RouteError::Unknown)?;
    if sender.host != local_device {
        return Err(RouteError::Spoofed);
    }
    require_capability(registry, &target.host)?;
    let room = doc.chat_id().ok_or(RouteError::Unknown)?;
    validate(registry, &target.host, &room, message)?;
    if message.upward() {
        return Ok(doc.append_orch_out(message)?);
    }
    if doc.read_commands()?.iter().any(|row| row.id == message.id) {
        return Ok(false);
    }
    doc.queue_command(&SessionCommandEntry {
        id: message.id.clone(),
        payload: SessionCommandPayload::Orch {
            message: message.clone(),
        },
        issued_by: local_device.into(),
        issued_at: 0,
        based_on: None,
        expires_at: Some(i64::MAX),
        status: SessionCommandStatus::Pending,
        resolution: None,
    })?;
    Ok(true)
}

pub fn scan(
    registry: &dyn Registry,
    local_device: &str,
    doc: &SessionDoc,
) -> Result<Vec<OrchMessage>, RouteError> {
    let room = doc.chat_id().ok_or(RouteError::Unknown)?;
    let mut rows = doc.read_orch_out()?;
    rows.extend(
        doc.read_commands()?
            .into_iter()
            .filter_map(|row| match row.payload {
                SessionCommandPayload::Orch { message }
                    if row.id == message.id && row.issued_by == message.sender_device =>
                {
                    Some(message)
                }
                _ => None,
            }),
    );
    Ok(rows
        .into_iter()
        .filter(|row| validate(registry, local_device, &room, row).is_ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use zeron_doc::OrchBody;

    struct Routes(BTreeMap<String, ChatRoute>);
    impl Registry for Routes {
        fn chat(&self, id: &str) -> Option<ChatRoute> {
            self.0.get(id).cloned()
        }
        fn supports_orch(&self, device: &str) -> bool {
            matches!(device, "laptop" | "cloud")
        }
    }
    fn routes() -> Routes {
        Routes(
            [
                ChatRoute {
                    chat: "p".into(),
                    host: "laptop".into(),
                    parent: None,
                },
                ChatRoute {
                    chat: "c".into(),
                    host: "cloud".into(),
                    parent: Some("p".into()),
                },
            ]
            .into_iter()
            .map(|r| (r.chat.clone(), r))
            .collect(),
        )
    }
    fn notice(run: u32) -> OrchMessage {
        OrchMessage {
            id: format!("notice:c:{run}"),
            sender_chat: "c".into(),
            target_chat: "p".into(),
            sender_device: "cloud".into(),
            body: OrchBody::Notice {
                run,
                outcome: "completed".into(),
                summary: "done".into(),
            },
        }
    }
    #[test]
    fn two_hosts_backfill_duplicate_reorder_rollback_and_spoof() {
        let routes = routes();
        let child = SessionDoc::init("c").unwrap();
        assert!(send(&routes, "cloud", &child, &notice(1)).unwrap());
        assert!(send(&routes, "cloud", &child, &notice(2)).unwrap());
        assert!(!send(&routes, "cloud", &child, &notice(1)).unwrap());
        let rebuilt = zeron_doc::rebuild_thin_doc(&child).unwrap().doc;
        let mut receipts = BTreeSet::new();
        let mut applied = Vec::new();
        for message in scan(&routes, "laptop", &rebuilt).unwrap().into_iter().rev() {
            if receipts.insert(message.id.clone()) {
                applied.push(message.id);
            }
        }
        for message in scan(&routes, "laptop", &rebuilt).unwrap() {
            if receipts.insert(message.id.clone()) {
                applied.push(message.id);
            }
        }
        assert_eq!(applied.len(), 2);
        receipts.clear();
        assert_eq!(scan(&routes, "laptop", &rebuilt).unwrap().len(), 2);
        let mut forged = notice(3);
        forged.sender_device = "laptop".into();
        assert!(matches!(
            validate(&routes, "laptop", "c", &forged),
            Err(RouteError::Spoofed)
        ));
        assert!(validate(&routes, "laptop", "p", &notice(1)).is_err());
        assert!(validate(&routes, "cloud", "c", &notice(1)).is_err());
        assert!(validate(&Routes(BTreeMap::new()), "laptop", "c", &notice(1)).is_err());
    }
    #[test]
    fn mismatched_ledger_id_never_reaches_receiver() {
        let registry = routes();
        let doc = SessionDoc::init("c").unwrap();
        let message = OrchMessage {
            id: "stop:c:7".into(),
            sender_chat: "p".into(),
            target_chat: "c".into(),
            sender_device: "laptop".into(),
            body: OrchBody::Stop { epoch: 7 },
        };
        doc.queue_command(&SessionCommandEntry {
            id: "forged-ledger-id".into(),
            payload: SessionCommandPayload::Orch { message },
            issued_by: "laptop".into(),
            issued_at: 0,
            based_on: None,
            expires_at: Some(i64::MAX),
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .unwrap();
        assert!(scan(&registry, "cloud", &doc).unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_command_does_not_block_next_control() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble(
            dir.path(),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
        .unwrap();
        let device = core.workspace.device_id().to_string();
        core.workspace
            .create_chat("p", None, Some(&device), None, None)
            .unwrap();
        core.workspace
            .create_chat_with_parent("c", None, Some(&device), None, None, Some("p".into()))
            .unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(BTreeSet::new()));
        let received = seen.clone();
        core.doc_host
            .set_orch_receiver(std::sync::Arc::new(move |message| {
                received.lock().unwrap().insert(message.id);
                Ok(())
            }));
        let doc = core.doc_host.open("c").unwrap();
        let message = OrchMessage {
            id: "stop:c:7".into(),
            sender_chat: "p".into(),
            target_chat: "c".into(),
            sender_device: device.clone(),
            body: OrchBody::Stop { epoch: 7 },
        };
        let row = |id: &str| SessionCommandEntry {
            id: id.into(),
            payload: SessionCommandPayload::Orch {
                message: message.clone(),
            },
            issued_by: device.clone(),
            issued_at: 0,
            based_on: None,
            expires_at: Some(i64::MAX),
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        assert!(
            core.doc_host
                .ingest_relayed_command("c", row(&message.id))
                .await
                .is_err()
        );
        assert!(seen.lock().unwrap().is_empty());
        assert!(
            !core
                .doc_host
                .docs_store()
                .is_processed(&message.id)
                .unwrap()
        );
        doc.doc().queue_command(&row("wrong-id")).unwrap();
        doc.doc().queue_command(&row(&message.id)).unwrap();
        core.doc_host.drain_commands(&doc).await;
        let rows = doc.doc().read_commands().unwrap();
        assert_eq!(rows[0].status, SessionCommandStatus::Rejected);
        assert_eq!(rows[1].status, SessionCommandStatus::Applied);
        assert!(seen.lock().unwrap().contains(&message.id));
        core.doc_host.shutdown_workers().await;
    }

    #[tokio::test]
    async fn cloud_sleep_waits_for_room_ack() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = crate::doc_host::DocHost::new(
            store.clone(),
            crate::doc_host::DocHostConfig {
                device_id: "cloud".into(),
                default_harness: zeron_proto::HarnessId::Mock,
                edge: None,
            },
        );
        store
            .enqueue_chat_update("c", "batch", b"durable update")
            .unwrap();
        assert_eq!(
            host.quiesce_for_sleep().await.unwrap_err(),
            "chat updates await room acknowledgement"
        );
        store.acknowledge_chat_update("c", "batch").unwrap();
        host.quiesce_for_sleep().await.unwrap();
        host.shutdown_workers().await;
    }

    #[test]
    fn downward_rows_have_no_ttl_and_rescan_after_ledger_mark() {
        let routes = routes();
        let child = SessionDoc::init("c").unwrap();
        let stop = OrchMessage {
            id: "stop:c:4".into(),
            sender_chat: "p".into(),
            target_chat: "c".into(),
            sender_device: "laptop".into(),
            body: OrchBody::Stop { epoch: 4 },
        };
        assert!(send(&routes, "laptop", &child, &stop).unwrap());
        assert!(!send(&routes, "laptop", &child, &stop).unwrap());
        assert_eq!(child.read_commands().unwrap()[0].expires_at, Some(i64::MAX));
        child
            .set_command_status(&stop.id, SessionCommandStatus::Applied, None)
            .unwrap();
        assert_eq!(scan(&routes, "cloud", &child).unwrap(), vec![stop]);
        assert!(require_capability(&routes, "old-engine").is_err());
    }
}

impl Registry for crate::workspace_host::WorkspaceHost {
    fn chat(&self, chat: &str) -> Option<ChatRoute> {
        crate::workspace_host::WorkspaceHost::chat(self, chat)
            .ok()
            .flatten()
            .map(|row| ChatRoute {
                chat: row.id,
                host: row.device_id,
                parent: row.parent_chat_id,
            })
    }
    fn supports_orch(&self, device: &str) -> bool {
        self.read_devices().is_ok_and(|rows| {
            rows.iter()
                .any(|row| row.id == device && row.supports(zeron_proto::capabilities::ORCH_V2))
        })
    }
}
