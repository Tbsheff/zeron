use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{ChildAsk, DEFAULT_WAIT, Hints, MAX_WAIT, ToolDef, Tools, parse};
use crate::error::ToolError;

pub(super) const DELEGATE_TASK: &str = "delegate_task";
pub(super) const TASK_STATUS: &str = "task_status";
pub(super) const TASK_CANCEL: &str = "task_cancel";
pub(super) const NAMES: &[&str] = &[DELEGATE_TASK, TASK_STATUS, TASK_CANCEL];

pub const INVALID_REQUEST: &str = "invalid_request";
pub const ORCHESTRATION_ERROR: &str = "orchestration_error";

const ENGINE_CODES: &[&str] = &[
    "capability_denied",
    "parent_not_active",
    "provider_unavailable",
    "model_unavailable",
    "runtime_mode_escalation_denied",
    "interaction_mode_escalation_denied",
    "task_not_found",
    "task_not_cancellable",
    "thread_not_found",
    "run_not_found",
    "thread_not_sendable",
    "thread_not_interruptible",
    INVALID_REQUEST,
    ORCHESTRATION_ERROR,
    "thread_credential_required",
    "target_required",
];

const TASK_MAX: usize = 120_000;
const TITLE_MAX: usize = 512;
const REQUEST_ID_MAX: usize = 256;
const REASON_MAX: usize = 2_000;
const RPC_MARGIN: Duration = Duration::from_secs(30);
const RPC_LIMIT: Duration = Duration::from_secs(60);

const DELEGATE_DESCRIPTION: &str = "Start one child with only `task`, no chat history. Include needed facts; exclude secrets. worktree uses a separate checkout. Keep taskId for status/cancel. Default async pushes the result when done; end your turn. Use wait if this turn needs the result. A child question ends wait; answer with respond_to_input {chat: childThreadId}.";

const STATUS_DESCRIPTION: &str = "List app/native tasks, or read taskId. Wait for completion wake before reading a running task. Reading final summary marks it delivered.";

const CANCEL_DESCRIPTION: &str = "Stop one delegated task by `taskId`: its running and queued turns and its own delegates. A finished task keeps its result.";

fn task_output() -> Value {
    json!({
        "type": "object",
        "properties": {
            "taskId": { "type": "string" }, "childThreadId": { "type": "string" },
            "status": { "enum": ["queued", "running", "waiting", "completed", "failed", "cancelled", "interrupted"] },
            "origin": {"enum":["app","native"]}, "background":{"type":["boolean","null"]},
            "summary": { "type": ["string", "null"] }, "hasPendingQuestion": { "type": "boolean" },
            "waitTimedOut": { "type": "boolean" }, "next": { "type": "string" }
        },
        "required": ["taskId", "childThreadId", "status", "summary", "hasPendingQuestion", "waitTimedOut"]
    })
}

fn task_id() -> Value {
    json!({ "taskId": { "type": "string" } })
}

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: DELEGATE_TASK,
            title: "Delegate a child task",
            description: DELEGATE_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string" },
                    "target": { "type": "object", "properties": {
                        "harness": { "type": "string" }, "model": { "type": "string" },
                        "project": { "type": "string" }, "device": { "type": "string" }
                    } },
                    "title": { "type": "string" },
                    "role": { "enum": ["implementation", "research", "review", "design", "test", "general"] },
                    "isolation": { "enum": ["shared", "worktree"] },
                    "mode": { "enum": ["async", "wait"] },
                    "timeoutMs": { "type": "integer", "description": "wait only; default 600000. A timeout does not cancel the child." },
                    "clientRequestId": { "type": "string" }
                },
                "required": ["task"]
            }),
            output_schema: Some(task_output()),
            hints: Hints::WRITE.destructive().open_world(),
            always_load: false,
            example: r#"{"task":"Add tests for the --dry-run flag in crates/cli.","title":"Tests"}"#,
        },
        ToolDef {
            name: TASK_STATUS,
            title: "Get delegated task status",
            description: STATUS_DESCRIPTION,
            input_schema: json!({ "type": "object", "properties": task_id() }),
            output_schema: Some(
                json!({"type":"object","$defs":{"task":task_output()},"oneOf":[{"$ref":"#/$defs/task"},{"properties":{"tasks":{"type":"array","items":{"$ref":"#/$defs/task"}}},"required":["tasks"]}]}),
            ),
            hints: Hints::WRITE.idempotent(),
            always_load: false,
            example: r#"{"taskId":"task-1"}"#,
        },
        ToolDef {
            name: TASK_CANCEL,
            title: "Cancel delegated task",
            description: CANCEL_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "taskId": { "type": "string" }, "reason": { "type": "string" },
                    "clientRequestId": { "type": "string" }
                },
                "required": ["taskId"]
            }),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "taskId": { "type": "string" },
                    "status": { "enum": ["cancel_requested", "completed", "failed", "cancelled", "interrupted"] }
                },
                "required": ["taskId", "status"]
            })),
            hints: Hints::WRITE.destructive(),
            always_load: false,
            example: r#"{"taskId":"task-1","reason":"No longer needed"}"#,
        },
    ]
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Async,
    Wait,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Isolation {
    #[default]
    Shared,
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Implementation,
    Research,
    Review,
    Design,
    Test,
    General,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TargetArgs {
    harness: Option<String>,
    model: Option<String>,
    project: Option<String>,
    device: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DelegateArgs {
    task: String,
    #[serde(default)]
    target: TargetArgs,
    title: Option<String>,
    role: Option<Role>,
    #[serde(default)]
    isolation: Isolation,
    #[serde(default)]
    mode: Mode,
    timeout_ms: Option<u64>,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StatusArgs {
    task_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CancelArgs {
    task_id: String,
    reason: Option<String>,
    client_request_id: Option<String>,
}

fn check_len(what: &str, value: Option<&str>, max: usize) -> Result<(), ToolError> {
    match value {
        Some(text) if text.len() > max => Err(ToolError::invalid(format!(
            "{what} is {} bytes; the limit is {max}",
            text.len()
        ))),
        _ => Ok(()),
    }
}

fn check_task_id(task_id: &str) -> Result<(), ToolError> {
    if task_id.trim().is_empty() {
        return Err(ToolError::invalid("taskId is empty"));
    }
    Ok(())
}

fn check_delegate(args: &DelegateArgs) -> Result<(), ToolError> {
    if args.task.trim().is_empty() {
        return Err(ToolError::invalid("task is empty"));
    }
    check_len("task", Some(&args.task), TASK_MAX)?;
    check_len("title", args.title.as_deref(), TITLE_MAX)?;
    check_len(
        "clientRequestId",
        args.client_request_id.as_deref(),
        REQUEST_ID_MAX,
    )
}

fn wait_ms(timeout_ms: Option<u64>) -> u64 {
    let max = MAX_WAIT.as_millis() as u64;
    timeout_ms
        .unwrap_or(DEFAULT_WAIT.as_millis() as u64)
        .min(max)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn delegate_params(origin: &str, args: &DelegateArgs, child: Value) -> Value {
    let mut params =
        json!({ "originChatId": origin, "task": args.task, "mode": args.mode, "child": child });
    if let Some(title) = non_empty(args.title.as_deref()) {
        params["title"] = json!(title);
    }
    if let Some(role) = args.role {
        params["role"] = json!(role);
    }
    if args.mode == Mode::Wait {
        params["timeoutMs"] = json!(wait_ms(args.timeout_ms));
    }
    if let Some(id) = &args.client_request_id {
        params["clientRequestId"] = json!(id);
    }
    params
}

fn rpc_limit(params: &Value) -> Duration {
    params["timeoutMs"]
        .as_u64()
        .map_or(RPC_LIMIT, |ms| Duration::from_millis(ms) + RPC_MARGIN)
}

fn fix_for(code: &str) -> &'static str {
    match code {
        "capability_denied" => {
            "Depth or fan-out cap reached; do the work in this chat or wait for a child to finish."
        }
        "parent_not_active" => {
            "This chat has no active orchestrated turn; delegate from a running turn."
        }
        "provider_unavailable" => "Pick a connected harness (catalog {kind: \"harnesses\"}).",
        "model_unavailable" => "Pick a model from catalog {kind: \"models\", harness}.",
        "task_not_found" => "Use a taskId that delegate_task returned to this chat.",
        "task_not_cancellable" => "The task has settled; read its result with task_status.",
        "target_required" => "Name target.project or target.device.",
        _ => "",
    }
}

pub(super) fn engine_failure(message: &str) -> ToolError {
    let known = message.split_once(": ").and_then(|(code, text)| {
        ENGINE_CODES
            .iter()
            .find(|known| **known == code.trim())
            .map(|known| (*known, text))
    });
    let (code, text) = known.unwrap_or((ORCHESTRATION_ERROR, message));
    let fix = fix_for(code);
    if fix.is_empty() {
        ToolError::new(code, text)
    } else {
        ToolError::new(code, format!("{text} {fix}"))
    }
}

fn orch_error(error: &RpcError) -> ToolError {
    match error {
        RpcError::Failed(message) => engine_failure(message),
        RpcError::UnknownMethod(_) => ToolError::new(
            ORCHESTRATION_ERROR,
            "This engine runs no orchestrator (ZERON_ORCH_V2 off, or an older engine). Use create_chats.",
        ),
        other => ToolError::from_rpc(other, ""),
    }
}

impl Tools {
    pub(super) fn delegates(&self) -> bool {
        let origin = self.zeron.origin();
        origin.orchestrator && origin.chat_id.is_some()
    }

    pub(super) async fn delegate(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        match name {
            DELEGATE_TASK => self.delegate_task(parse(args.clone())?, args).await,
            TASK_STATUS => self.task_status(parse(args)?).await,
            _ => self.task_cancel(parse(args)?).await,
        }
    }

    fn origin_chat(&self) -> Result<String, ToolError> {
        self.zeron.origin().chat_id.clone().ok_or_else(|| {
            ToolError::new(
                INVALID_REQUEST,
                "Delegated tasks work only from an agent Zeron launched in a chat.",
            )
        })
    }

    async fn delegate_task(&self, args: DelegateArgs, observed: Value) -> Result<Value, ToolError> {
        check_delegate(&args)?;
        let origin = self.origin_chat()?;
        let child = self.delegate_child(&args.target, args.isolation).await?;
        let mut params = delegate_params(&origin, &args, child);
        params["observedInput"] = observed;
        let limit = rpc_limit(&params);
        self.orch_call(methods::ORCH_DELEGATE, params, limit).await
    }

    async fn delegate_child(
        &self,
        target: &TargetArgs,
        isolation: Isolation,
    ) -> Result<Value, ToolError> {
        let ceiling = self.access_ceiling(false).await?;
        let ask = ChildAsk {
            project: target.project.as_deref(),
            device: target.device.as_deref(),
            harness: target.harness.as_deref(),
            model: target.model.as_deref(),
            ..Default::default()
        };
        let resolved = Box::pin(self.resolve_child(ask, ceiling)).await?;
        let chat_id = uuid::Uuid::new_v4().to_string();
        let mut child =
            json!({ "chatId": chat_id, "deviceId": resolved.device_id, "config": resolved.config });
        if let Some(space) = &resolved.space {
            child["spaceId"] = json!(space.id);
        }
        if isolation == Isolation::Worktree {
            child["worktree"] = worktree_spec(resolved.space.as_ref())?;
        } else if let Some(cwd) = self
            .scratch_cwd(resolved.space.as_ref(), &resolved.device_id, &chat_id)
            .await?
        {
            child["cwd"] = json!(cwd);
        }
        Ok(child)
    }

    async fn task_status(&self, args: StatusArgs) -> Result<Value, ToolError> {
        if let Some(id) = args.task_id.as_deref() {
            check_task_id(id)?;
        }
        let params = json!({ "originChatId": self.origin_chat()?, "taskId": args.task_id });
        self.orch_call(methods::ORCH_TASK_STATUS, params, RPC_LIMIT)
            .await
    }

    async fn task_cancel(&self, args: CancelArgs) -> Result<Value, ToolError> {
        check_task_id(&args.task_id)?;
        check_len("reason", args.reason.as_deref(), REASON_MAX)?;
        check_len(
            "clientRequestId",
            args.client_request_id.as_deref(),
            REQUEST_ID_MAX,
        )?;
        let mut params = json!({ "originChatId": self.origin_chat()?, "taskId": args.task_id });
        if let Some(reason) = non_empty(args.reason.as_deref()) {
            params["reason"] = json!(reason);
        }
        if let Some(id) = &args.client_request_id {
            params["clientRequestId"] = json!(id);
        }
        self.orch_call(methods::ORCH_TASK_CANCEL, params, RPC_LIMIT)
            .await
    }

    pub(super) async fn orch_call(
        &self,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, ToolError> {
        match tokio::time::timeout(limit, self.zeron.call_rpc(method, params)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(orch_error(&error)),
            Err(_) => Err(ToolError::new(
                ORCHESTRATION_ERROR,
                format!(
                    "The engine did not answer {method} within {} s. The task may still run; check it with task_status.",
                    limit.as_secs()
                ),
            )),
        }
    }
}

fn worktree_spec(space: Option<&zeron_proto::Space>) -> Result<Value, ToolError> {
    match space {
        Some(space) if space.git_detected => Ok(json!(zeron_proto::WorktreeSpec {
            repo_path: space.path.clone(),
            base: "HEAD".into(),
            space_id: Some(space.id.clone()),
        })),
        _ => Err(ToolError::new(
            INVALID_REQUEST,
            "isolation worktree needs a git project. Name one in target.project, or use isolation shared.",
        )),
    }
}
