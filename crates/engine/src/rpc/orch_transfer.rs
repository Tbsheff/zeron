use super::EngineRpc;
use serde::Deserialize;
use serde_json::Value;
use zeron_rpc::{RpcError, RpcReply};

pub const METHOD: &str = "orch.transfer_context";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    origin_chat_id: String,
    #[serde(default)]
    target_chat_id: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    client_request_id: String,
    #[serde(default)]
    base_message_id: Option<String>,
    #[serde(default)]
    point_message_id: Option<String>,
}
impl EngineRpc {
    pub(super) fn orch_transfer(&self, params: Value) -> Result<RpcReply, RpcError> {
        if let zeron_rpc::Caller::Agent { chat } = zeron_rpc::caller() {
            let chat=chat.ok_or_else(||RpcError::Failed("capability_denied: calling chat required".into()))?;
            if let Some(wire)=self.sessions.orchestrator() {
                return wire.with_agent_admission(&self.sessions,&chat,|error|Err(RpcError::Failed(error.to_string())),||self.orch_transfer_admitted(params));
            }
        }
        self.orch_transfer_admitted(params)
    }
    fn orch_transfer_admitted(&self, params: Value) -> Result<RpcReply, RpcError> {
        if self.sessions.orchestrator().is_none() {
            return Err(RpcError::Failed(
                "capability_denied: context transfers need orchestrator v2".into(),
            ));
        }
        let mut p: Params =
            serde_json::from_value(params).map_err(|e| RpcError::BadParams(e.to_string()))?;
        if let zeron_rpc::Caller::Agent { chat } = zeron_rpc::caller() {
            if chat.as_deref() != Some(p.origin_chat_id.as_str()) {
                return Err(RpcError::Failed(
                    "capability_denied: source must be the calling chat".into(),
                ));
            }
        }
        if p.action == "merge_back" {
            let source = self
                .workspace
                .chat(&p.origin_chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("source chat not found".into()))?;
            p.target_chat_id = source
                .parent_chat_id
                .ok_or_else(|| RpcError::BadParams("merge_back needs a forked chat".into()))?;
        }
        if p.target_chat_id.is_empty() {
            p.target_chat_id = p.origin_chat_id.clone();
        }
        if matches!(zeron_rpc::caller(), zeron_rpc::Caller::Agent { .. })
            && p.target_chat_id != p.origin_chat_id
        {
            let source = self
                .workspace
                .chat(&p.origin_chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("source chat not found".into()))?;
            let target = self
                .workspace
                .chat(&p.target_chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("target chat not found".into()))?;
            if source.parent_chat_id.as_deref() != Some(target.id.as_str())
                && target.parent_chat_id.as_deref() != Some(source.id.as_str())
            {
                return Err(RpcError::Failed("capability_denied: transfer target must be this chat, its parent, or its child".into()));
            }
        }
        if p.action == "status" {
            let target = self
                .doc_host
                .open(&p.target_chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            let views: Vec<_> = crate::orchestrator::transfer::status(target.doc()).into_values().map(|t| serde_json::json!({"transferId":t.id,"sourceChatId":t.source,"targetChatId":t.target,"baseMessageId":t.base,"pointMessageId":t.point,"status":t.status,"selectedMessageIds":t.selected,"sourceItemCount":t.items.len()})).collect();
            return RpcReply::value(&views);
        }
        if !matches!(p.action.as_str(), "" | "transfer" | "merge_back") {
            return Err(RpcError::BadParams(
                "action must be fork, merge_back, status, or transfer".into(),
            ));
        }
        if p.client_request_id.is_empty() || p.client_request_id.len() > 256 {
            return Err(RpcError::BadParams(
                "clientRequestId must contain 1 to 256 bytes".into(),
            ));
        }
        for id in [&p.origin_chat_id, &p.target_chat_id] {
            let chat = self
                .workspace
                .chat(id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("chat not found".into()))?;
            if id == &p.target_chat_id && chat.device_id != self.doc_host.device_id() {
                return Err(RpcError::Failed(
                    "transfer must run on the target host".into(),
                ));
            }
        }
        let source = self
            .doc_host
            .open(&p.origin_chat_id)
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        let frozen_source = self.sessions.orchestrator().filter(|wire|wire.driver().mechanism_capture_enabled()).map(|wire|crate::orchestrator::transfer::freeze_source(wire.driver(),&self.doc_host,&source,&p.origin_chat_id)).transpose().map_err(RpcError::Failed)?;
        let source_view = frozen_source.as_ref().unwrap_or(source.doc());
        let entries = source_view
            .read_entries()
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        let stable = entries
            .iter()
            .rposition(|e| {
                e.role == zeron_doc::MessageRole::Assistant
                    && e.status == Some(zeron_doc::MessageStatus::Complete)
            })
            .ok_or_else(|| RpcError::Failed("source has no completed response".into()))?;
        let point = p.point_message_id.as_deref().unwrap_or(&entries[stable].id);
        if !entries[..=stable].iter().any(|e| e.id == point) {
            return Err(RpcError::BadParams(
                "point must be a stable source message".into(),
            ));
        }
        let target = self
            .doc_host
            .open(&p.target_chat_id)
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        let id = format!("{}:{}", p.origin_chat_id, p.client_request_id);
        if p.action == "merge_back" && p.base_message_id.is_none() {
            p.base_message_id = crate::orchestrator::transfer::fork_base(source_view);
        }
        let wire = self.sessions.orchestrator().expect("v2 gate checked");
        let before = crate::orchestrator::transfer::begin_operation(
            wire.driver(),
            &self.doc_host,
            &target,
            &p.target_chat_id,
        )
        .map_err(RpcError::Failed)?;
        let operation_args = if before.is_some() {
            serde_json::json!({
                "id":id,"source_chat":p.origin_chat_id,"target_chat":p.target_chat_id,
                "base":p.base_message_id,"point":point,"source":crate::orchestrator::transfer::source_fact(&entries[..=stable]),
                "queue_id":if p.action == "merge_back" {Some(format!("merge-back:{id}"))}else{None},
                "hold_for_turn_end":p.action == "merge_back","target_local":true,"issuer":self.doc_host.device_id(),
                "caller_relation_checked":matches!(zeron_rpc::caller(), zeron_rpc::Caller::Agent { .. })
            })
        } else {
            serde_json::Value::Null
        };
        let transfer = crate::orchestrator::transfer::pin(
            target.doc(),
            &id,
            &p.origin_chat_id,
            &p.target_chat_id,
            p.base_message_id,
            point,
            &entries[..=stable],
        )
        .map_err(RpcError::Failed)?;
        if p.action == "merge_back" {
            let row_id = format!("merge-back:{id}");
            let has_row = target
                .doc()
                .read_queue()
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .iter()
                .any(|q| q.id == row_id)
                || target
                    .doc()
                    .read_entries()
                    .map_err(|e| RpcError::Failed(e.to_string()))?
                    .iter()
                    .any(|e| e.id == row_id);
            if !has_row {
                let mut row = zeron_doc::QueuedMessage::new(
                    &row_id,
                    "Review the completed work from the fork and continue the parent task.",
                    self.doc_host.device_id(),
                );
                row.hold_for_turn_end = true;
                target
                    .doc()
                    .push_queued(&row)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
            }
            crate::orchestrator::transfer::merge_marker(target.doc(), &transfer)
                .map_err(RpcError::Failed)?;
        }
        if wire.driver().mechanism_capture_enabled() { crate::orchestrator::transfer::record_lineage(target.doc(),source_view,&id,if p.action=="merge_back" {"merge_back"}else{"provider_switch"},&p.origin_chat_id,&p.target_chat_id,point,transfer.base.as_deref(),&[]).map_err(RpcError::Failed)?; }
        self.doc_host
            .persist_fork(&target)
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        crate::orchestrator::transfer::end_operation(
            wire.driver(),
            &self.doc_host,
            &p.target_chat_id,
            before,
            if p.action == "merge_back" {
                "MergeBackPinAndQueue"
            } else {
                "TransferPin"
            },
            operation_args,
            true,
        )
        .map_err(RpcError::Failed)?;
        if p.action == "merge_back" {
            self.doc_host.kick_drains();
        }
        RpcReply::value(
            &serde_json::json!({"transferId":transfer.id,"sourceChatId":transfer.source,"targetChatId":transfer.target,"pointMessageId":transfer.point,"status":transfer.status,"sourceItemCount":transfer.items.len()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use zeron_rpc::{Caller, RpcService, methods, with_caller};
    async fn setup() -> (tempfile::TempDir, crate::EngineCore) {
        let temp = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(temp.path()).unwrap(),
            Arc::new(crate::HarnessRegistry::new()),
            zeron_proto::HarnessId::Codex,
            None,
        )
        .unwrap();
        core.doc_host.docs_store().transact(|tx|crate::orchestrator::store::put_meta(tx,"mechanism_capture","1")).unwrap();
        core.enable_orchestrator_v2(crate::orchestrator::OrchOptions::default())
            .unwrap();
        for id in ["a", "b", "unrelated"] {
            core.rpc_service().handle(methods::MUTATE, json!({"op":"createChat","chatId":id,"deviceId":core.device_id,"parentChatId": if id == "b" { Some("a") } else { None }, "config":{"harness":"codex","sandbox":"workspace-write","access":"full-access"}})).await.unwrap();
        }
        let handle = core.doc_host.open("a").unwrap();
        handle
            .doc()
            .push_message(&zeron_doc::SessionMessageEntry {
                id: "m1".into(),
                role: zeron_doc::MessageRole::Assistant,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: "t1".into(),
                    text: "completed fixture".into(),
                }],
                created_at: 1,
                device_id: core.device_id.clone(),
                status: Some(zeron_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        (temp, core)
    }
    #[tokio::test]
    async fn rejects_forged_source_and_pins_stable_range_once() {
        let (_temp, core) = setup().await;
        let rpc = core.rpc_service();
        let params = json!({"originChatId":"a","targetChatId":"b","clientRequestId":"x"});
        assert!(
            with_caller(
                Caller::Agent {
                    chat: Some("a".into())
                },
                rpc.handle(
                    METHOD,
                    json!({"action":"status","originChatId":"a","targetChatId":"unrelated"})
                )
            )
            .await
            .is_err()
        );
        assert!(with_caller(Caller::Agent { chat:Some("a".into()) }, rpc.handle(METHOD, json!({"action":"transfer","originChatId":"a","targetChatId":"unrelated","clientRequestId":"bad-target"}))).await.is_err());

        assert!(
            with_caller(
                Caller::Agent {
                    chat: Some("b".into())
                },
                rpc.handle(METHOD, params.clone())
            )
            .await
            .is_err()
        );
        rpc.handle(METHOD, params.clone()).await.unwrap();
        rpc.handle(METHOD, params).await.unwrap();
        let target = core.doc_host.open("b").unwrap();
        let transfers = crate::orchestrator::transfer::status(target.doc());
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers["a:x"].point, "m1");
        assert!(rpc.handle(METHOD, json!({"originChatId":"a","targetChatId":"b","clientRequestId":"bad","pointMessageId":"missing"})).await.is_err());
    }
    #[tokio::test]
    async fn captures_actual_fork_and_idempotent_merge_back() {
        let (_temp, core) = setup().await;
        let rpc = core.rpc_service();
        rpc.handle(methods::FORK_SIDE_CHAT, json!({"sourceChatId":"a","chatId":"fork-child"})).await.unwrap();
        let child = core.doc_host.open("fork-child").unwrap();
        let mut entry = core.doc_host.open("a").unwrap().doc().read_entries().unwrap().remove(0);
        entry.id = "child-complete".into();
        entry.parts = vec![zeron_doc::MessagePart::Text { id:"child-text".into(), text:"child delta".into() }];
        child.doc().push_message(&entry).unwrap();
        core.doc_host.persist_fork(&child).unwrap();
        let params = json!({"action":"merge_back","originChatId":"fork-child","clientRequestId":"merge-once"});
        rpc.handle(METHOD, params.clone()).await.unwrap();
        rpc.handle(METHOD, params).await.unwrap();
        let parent = core.doc_host.open("a").unwrap();
        let transfers = crate::orchestrator::transfer::status(parent.doc());
        let transfer = &transfers["fork-child:merge-once"];
        assert_eq!(transfer.base.as_deref(),Some("m1"));
        assert_eq!(transfer.items.iter().filter(|i|!i.text.is_empty()).map(|i|i.id.as_str()).collect::<Vec<_>>(),vec!["child-complete"]);
        let queue = parent.doc().read_queue().unwrap();
        let entries = parent.doc().read_entries().unwrap();
        let row = "merge-back:fork-child:merge-once";
        assert_eq!(queue.iter().filter(|q|q.id==row).count()+entries.iter().filter(|e|e.id==row).count(),1);
        assert!(queue.iter().filter(|q|q.id==row).all(|q|q.hold_for_turn_end));
        crate::orchestrator::transfer::export_capture_for_test(core.sessions.orchestrator().unwrap().driver(),"captures_actual_fork_and_idempotent_merge_back","engine-integration");
        core.sessions.shutdown().await;
    }

    #[tokio::test]
    async fn frozen_source_excludes_concurrent_completed_append() {
        let (_temp,core)=setup().await;
        let wire=core.sessions.orchestrator().unwrap();
        let driver=wire.driver();
        let source=core.doc_host.open("a").unwrap();
        let frozen=crate::orchestrator::transfer::freeze_source(driver,&core.doc_host,&source,"a").unwrap();
        let original_version=frozen.doc().state_vv();
        let live=source.doc_arc();
        let mut entry=frozen.read_entries().unwrap().remove(0);
        entry.id="concurrent-complete".into();
        std::thread::spawn(move ||live.push_message(&entry).unwrap()).join().unwrap();
        let entries=frozen.read_entries().unwrap();
        assert_eq!(entries.iter().map(|e|e.id.as_str()).collect::<Vec<_>>(),vec!["m1"]);
        assert_eq!(frozen.doc().state_vv(),original_version);
        assert_eq!(source.doc().read_entries().unwrap().last().unwrap().id,"concurrent-complete");
        let target=core.doc_host.open("b").unwrap();
        for entry in &entries {target.doc().push_message(entry).unwrap();}
        crate::orchestrator::transfer::pin(target.doc(),"frozen","a","b",None,"m1",&entries).unwrap();
        crate::orchestrator::transfer::record_lineage(target.doc(),&frozen,"frozen","fork","a","b","m1",None,&entries).unwrap();
        core.doc_host.persist_fork(&target).unwrap();
        let fact=crate::orchestrator::transfer::durable_fact(target.doc(),"b").unwrap();
        assert_eq!(fact["lineage"]["frozen"]["source_point"]["last_item_id"],"m1");
        assert_eq!(fact["lineage"]["frozen"]["copied_items"].as_array().unwrap().len(),1);
        assert_eq!(crate::orchestrator::transfer::status(target.doc())["frozen"].point,"m1");
        crate::orchestrator::transfer::export_capture_for_test(driver,"frozen_source_excludes_concurrent_completed_append","focused-mock-engine");
        core.sessions.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_transfer_and_fork_from_unadmitted_agent() {
        let (_temp,core)=setup().await;
        let rpc=core.rpc_service();
        for (method,params) in [(METHOD,json!({"action":"status","originChatId":"a"})),(METHOD,json!({"originChatId":"a","targetChatId":"b","clientRequestId":"stale"})),(methods::FORK_SIDE_CHAT,json!({"sourceChatId":"a","chatId":"stale-fork"}))] {
            assert!(with_caller(Caller::Agent {chat:Some("a".into())},rpc.handle(method,params)).await.is_err());
        }
        assert!(core.workspace.chat("stale-fork").unwrap().is_none());
        assert!(crate::orchestrator::transfer::status(core.doc_host.open("b").unwrap().doc()).is_empty());
        core.sessions.shutdown().await;
    }

}
