use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcError, RpcReply, methods};

use super::EngineRpc;
use crate::orchestrator::wire::{DelegateRequest, OrchError};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskParams {
    #[serde(default)]
    origin_chat_id: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    client_request_id: Option<String>,
}

fn failed(e: OrchError) -> RpcError {
    RpcError::Failed(e.to_string())
}

fn bad(e: serde_json::Error) -> RpcError {
    RpcError::BadParams(e.to_string())
}

impl EngineRpc {
    pub(super) async fn orch_message(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<Option<RpcReply>, RpcError> {
        if !matches!(method, methods::QUEUE_MESSAGE | methods::QUEUE_COMMAND) {
            return Ok(None);
        }
        let zeron_rpc::Caller::Agent { chat: Some(origin) } = zeron_rpc::caller() else {
            return Ok(None);
        };
        let Some(wire) = self.sessions.orchestrator() else {
            return Ok(None);
        };
        let Some(child) = params["chatId"].as_str() else {
            return Ok(None);
        };
        let Some(th) = wire.thread_state(child).filter(|th| th.task.is_some()) else {
            return Ok(None);
        };
        let Some(parent) = wire.thread_state(&origin) else {
            return Err(RpcError::Failed(
                "capability_denied: Only the task parent sends follow-up work".into(),
            ));
        };
        let prompt = if method == methods::QUEUE_MESSAGE {
            params["text"].as_str()
        } else {
            match params["command"]["kind"]
                .as_str()
                .or_else(|| params["command"]["type"].as_str())
            {
                Some("run") => params["command"]["request"]["prompt"].as_str(),
                Some("steer") => params["command"]["prompt"].as_str(),
                _ => return Ok(None),
            }
        };
        if th.parent != Some(parent.id) {
            return Err(RpcError::Failed(
                "capability_denied: Only the task parent sends follow-up work".into(),
            ));
        }
        let prompt = prompt.ok_or_else(|| RpcError::BadParams("Follow-up needs text".into()))?;
        if [
            &params["attachments"],
            &params["command"]["request"]["attachments"],
        ]
        .iter()
        .any(|value| value.as_array().is_some_and(|a| !a.is_empty()))
        {
            return Err(RpcError::BadParams(
                "Task follow-ups accept text only".into(),
            ));
        }
        let id = params["clientRequestId"]
            .as_str()
            .or_else(|| params["command"]["messageId"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let view = wire
            .followup(&self.sessions, &origin, child, prompt, &id)
            .await
            .map_err(failed)?;
        let mut value = serde_json::to_value(view).map_err(bad)?;
        value["id"] = json!(id);
        value["commandId"] = json!(id);
        Ok(Some(RpcReply::Value(value)))
    }

    pub(super) async fn orch_call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcReply, RpcError> {
        let Some(wire) = self.sessions.orchestrator() else {
            return Err(RpcError::Failed(
                "capability_denied: delegate_task needs the orchestrator (ZERON_ORCH_V2=1)".into(),
            ));
        };
        let sessions = &self.sessions;
        if method == methods::ORCH_DELEGATE {
            let req: DelegateRequest = serde_json::from_value(params).map_err(bad)?;
            let view = if req.followup {
                let id = req
                    .client_request_id
                    .as_deref()
                    .ok_or_else(|| RpcError::BadParams("Follow-up needs clientRequestId".into()))?;
                wire.followup(
                    sessions,
                    &req.origin_chat_id,
                    &req.child.chat_id,
                    &req.task,
                    id,
                )
                .await
                .map_err(failed)?
            } else {
                wire.delegate(sessions, req).await.map_err(failed)?
            };
            return RpcReply::value(&view);
        }
        let p: TaskParams = serde_json::from_value(params).map_err(bad)?;
        if method == methods::ORCH_TASK_STATUS {
            let Some(task_id) = p.task_id.as_deref() else {
                return RpcReply::value(
                    &json!({"tasks":wire.task_list(sessions,&p.origin_chat_id).map_err(failed)?}),
                );
            };
            let view = wire
                .task_status(sessions, &p.origin_chat_id, task_id)
                .map_err(failed)?;
            return RpcReply::value(&view);
        }
        let (task_id, status) = wire
            .task_cancel(
                sessions,
                &p.origin_chat_id,
                p.task_id
                    .as_deref()
                    .ok_or_else(|| RpcError::BadParams("taskId is required".into()))?,
                p.client_request_id.as_deref(),
            )
            .await
            .map_err(failed)?;
        RpcReply::value(&json!({ "taskId": task_id, "status": status }))
    }
}
