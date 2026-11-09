use super::{Hints, ToolDef, Tools};
use crate::error::ToolError;
use serde_json::{Value, json};

pub(super) const NAME: &str = "transfer_context";
pub(super) fn catalog() -> Vec<ToolDef> {
    vec![ToolDef {
        name: NAME,
        title: "Transfer chat context",
        description: "Pin completed messages from this chat for another chat. Use action fork to create a side chat, merge_back to queue this fork's result for its parent, or status to read transfers. Fork and transfer deliver context at the target's next run. Use a stable clientRequestId for retries. Native tools and attachments are not copied.",
        input_schema: json!({"type":"object", "properties": {"action":{"type":"string","enum":["fork","merge_back","status","transfer"]},"targetChatId":{"type":"string"}, "clientRequestId":{"type":"string"}, "baseMessageId":{"type":"string"}, "pointMessageId":{"type":"string"}}, "required":["action"], "additionalProperties":false}),
        output_schema: None,
        hints: Hints::WRITE.idempotent(),
        always_load: false,
        example: r#"{"action":"merge_back","clientRequestId":"handoff-1"}"#,
    }]
}
impl Tools {
    pub(super) async fn transfer_context(&self, mut args: Value) -> Result<Value, ToolError> {
        let origin = self.zeron.origin();
        if !origin.orchestrator {
            return Err(ToolError::invalid("context transfers need orchestrator v2"));
        }
        let chat = origin
            .chat_id
            .as_deref()
            .ok_or_else(|| ToolError::invalid("context transfers need an agent chat"))?;
        let object = args
            .as_object_mut()
            .ok_or_else(|| ToolError::invalid("expected an object"))?;
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("transfer")
            .to_owned();
        if action == "fork" {
            let target = object
                .get("targetChatId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    ToolError::invalid("fork needs a stable targetChatId for retries")
                })?;
            return self
                .zeron
                .call_rpc(
                    zeron_rpc::methods::FORK_SIDE_CHAT,
                    json!({"chatId":target,"sourceChatId":chat}),
                )
                .await
                .map_err(|e| ToolError::invalid(e.to_string()));
        }
        object.insert("originChatId".into(), json!(chat));
        self.zeron
            .call_rpc("orch.transfer_context", args)
            .await
            .map_err(|e| ToolError::invalid(e.to_string()))
    }
}
