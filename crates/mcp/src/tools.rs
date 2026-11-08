//! The tool catalog and its dispatch.
//!
//! Every tool is a thin composition of engine reads/writes from
//! [`Zeron`]; the only logic that lives here is argument resolution (chat
//! by prefix, project by path), sender attribution, and the "how do I
//! deliver a message to a chat in this state" choice the composer makes
//! for humans.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::{
    AccessMode, Chat, ChatConfig, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, Session, SessionStatus,
    Space, UserInputAnswer,
};

use crate::error::ToolError;
use crate::transcript::{RenderOptions, RenderedMessage, render_entries};
use crate::zeron::{HarnessInfo, TurnOutcome, Zeron, session_for, short};

mod defs;
mod plugins;
mod themes;
mod visual;
pub use plugins::ListedTool;
pub(crate) use plugins::is_plugin_tool_name;
use visual::{SHOW_IMAGE, SHOW_VISUAL, VISUAL_REFERENCE, WAIT_ON_BACKGROUND};

const MAX_BATCH: usize = 32;
const CLOUD_HARNESSES: &[HarnessId] = &[HarnessId::ClaudeCode, HarnessId::Codex];
const DEFAULT_WAIT: Duration = Duration::from_secs(600);
const MAX_WAIT: Duration = Duration::from_secs(3600);
const COLLECT_NEXT: &str = "The chats work in parallel now. Collect each reply with wait_for_turn {chat: chatId} (do other work first if you have it). Do not poll read_chat.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl Hints {
    pub const READ: Self = Self {
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    };
    pub const WRITE: Self = Self {
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    };

    pub const fn destructive(self) -> Self {
        Self {
            destructive: true,
            ..self
        }
    }

    pub const fn idempotent(self) -> Self {
        Self {
            idempotent: true,
            ..self
        }
    }

    pub const fn open_world(self) -> Self {
        Self {
            open_world: true,
            ..self
        }
    }

    pub fn to_json(self, title: &str) -> Value {
        json!({
            "title": title,
            "readOnlyHint": self.read_only,
            "destructiveHint": self.destructive,
            "idempotentHint": self.idempotent,
            "openWorldHint": self.open_world,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub hints: Hints,
    pub always_load: bool,
    pub example: &'static str,
}

type Replay = Arc<tokio::sync::OnceCell<Value>>;

pub struct Tools {
    zeron: Arc<Zeron>,
    // Remember successful sends on this MCP connection so wait_for_turn after
    // wait:false also waits for a newly created chat with no session row yet.
    pending_turns: tokio::sync::Mutex<HashMap<String, Arc<PendingTurn>>>,
    replays: tokio::sync::Mutex<HashMap<String, Replay>>,
    /// Which plugin tools the client was last shown ([`Self::list_all`],
    /// [`Self::watch_plugin_tools`]).
    plugins: plugins::PluginState,
}

pub fn renamed(old: &str) -> Option<&'static str> {
    Some(match old {
        "create_chat" => defs::CREATE_CHATS,
        "send_message" => defs::SEND_MESSAGES,
        "get_chat" => defs::READ_CHAT,
        "whoami" | "list_devices" | "list_projects" | "list_harnesses" | "list_models" => defs::CATALOG,
        "list_plugins" | "plugin_status" | "describe_elements" | "update_plugin" | "create_plugin"
        | "install_plugin" => plugins::PLUGINS,
        "list_templates" | "template_status" | "describe_app" | "create_template" | "use_template"
        | "delete_template" | "export_template" | "import_template" => plugins::TEMPLATES,
        "list_themes" | "get_theme" | "create_theme" | "update_theme" | "delete_theme" | "use_theme" => {
            themes::THEMES
        }
        _ => return None,
    })
}

fn catalog() -> Vec<ToolDef> {
    let mut tools = defs::catalog();
    tools.extend(visual::catalog());
    tools.extend(plugins::catalog());
    tools.extend(themes::catalog());
    tools
}

// ---- argument shapes ---------------------------------------------------------

#[derive(Deserialize)]
struct BatchArgs {
    requests: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatArgs {
    chat: String,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CatalogKind {
    #[serde(rename = "self")]
    Me,
    Devices,
    Projects,
    Harnesses,
    Models,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogArgs {
    kind: CatalogKind,
    device: Option<String>,
    harness: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ListChatsArgs {
    project: Option<String>,
    device: Option<String>,
    #[serde(default)]
    include_archived: bool,
    parent: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ChatKind {
    Chat,
    Side,
    Subagent,
}

/// Messages already in the transcript before a send. Replies are matched by
/// id, not by time: `createdAt` is stamped on the host's clock, which can run
/// behind the caller's when the chat lives on another device.
#[derive(Clone, Default)]
struct TurnStart {
    message_ids: Vec<String>,
}

#[derive(Clone)]
struct PendingTurn {
    baseline: Option<Session>,
    start: TurnStart,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CreateChatArgs {
    kind: Option<ChatKind>,
    project: Option<String>,
    device: Option<String>,
    parent: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    sandbox: Option<String>,
    access: Option<String>,
    #[serde(default)]
    plan: bool,
    title: Option<String>,
    branch: Option<String>,
    cwd: Option<String>,
    #[serde(default)]
    cloud: bool,
    prompt: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
    #[serde(rename = "clientRequestId")]
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadChatArgs {
    chat: String,
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    include_reasoning: bool,
    include_tools: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendArgs {
    chat: String,
    text: String,
    mode: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
    #[serde(rename = "clientRequestId")]
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    chat: String,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveArgs {
    chat: String,
    archived: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
    chat: String,
    request_id: Option<String>,
    #[serde(default)]
    answers: Vec<AnswerArg>,
}

#[derive(Deserialize)]
struct AnswerArg {
    question_id: String,
    labels: Vec<String>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::invalid(format!("invalid arguments: {e}")))
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

fn wait_duration(secs: Option<u64>) -> Duration {
    secs.map(Duration::from_secs)
        .unwrap_or(DEFAULT_WAIT)
        .min(MAX_WAIT)
}

#[cfg(test)]
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn parse_enum<T: serde::de::DeserializeOwned>(what: &str, raw: &str) -> Result<T, ToolError> {
    serde_json::from_value(Value::String(raw.trim().to_owned()))
        .map_err(|_| ToolError::invalid(format!("unknown {what}: {raw:?}")))
}

// ---- summaries ---------------------------------------------------------------

/// Live posture of one chat as the tools report it.
fn status_of(session: Option<&Session>) -> (String, Option<i64>) {
    let Some(session) = session else {
        return ("idle".into(), None);
    };
    let age = chrono::Utc::now() - session.updated_at;
    let label = match zeron_proto::view::effective_indicator(Some(session), chrono::Utc::now()) {
        zeron_proto::view::Indicator::Working => "working",
        zeron_proto::view::Indicator::AwaitingInput => "awaitingInput",
        zeron_proto::view::Indicator::Errored => "errored",
        zeron_proto::view::Indicator::None => "idle",
    };
    (label.into(), Some(age.num_seconds().max(0)))
}

fn summarize_chat(chat: &Chat, spaces: &[Space], sessions: &[Session]) -> Value {
    strip_nulls(chat_summary(chat, spaces, sessions))
}

fn chat_summary(chat: &Chat, spaces: &[Space], sessions: &[Session]) -> Value {
    let space = chat
        .space_id
        .as_deref()
        .and_then(|id| spaces.iter().find(|s| s.id == id));
    let session = session_for(sessions, chat);
    let (_, status_age) = status_of(session.as_ref());
    let now = chrono::Utc::now();
    let status = zeron_proto::view::display_status(chat, session.as_ref(), now);
    json!({
        "id": chat.id,
        "title": chat.title,
        "project": space.map(|s| json!({
            "id": s.id,
            "name": s.display_name(),
            "path": s.path,
        })),
        "deviceId": chat.device_id,
        "cwd": chat.cwd.clone().or_else(|| space.map(|s| s.path.clone())),
        "branch": chat.branch,
        "harness": chat.config.as_ref().map(|c| c.harness),
        "model": chat.config.as_ref().and_then(|c| c.model.clone()),
        "reasoning": chat.config.as_ref().and_then(|c| c.reasoning),
        "archived": chat.archived,
        "parentChatId": chat.parent_chat_id,
        "childKind": chat.child_kind,
        "status": status,
        "sessionStatus": session.as_ref().map(|s| s.status),
        "statusStale": session.as_ref().is_some_and(|s| zeron_proto::view::session_stale(s, now)),
        "lastTurnOutcome": session.as_ref().and_then(|s| s.last_turn_outcome),
        "terminalActivity": session.as_ref().and_then(|s| s.terminal_activity.as_ref()),
        "statusAgeSecs": status_age,
        "lastMessageAt": chat.last_message_at,
        "lastMessagePreview": chat.last_message_preview,
        "createdAt": chat.created_at,
    })
}

fn last_pending_input(messages: &[RenderedMessage]) -> Option<Value> {
    messages.iter().rev().find_map(|m| m.pending_input.clone())
}

// ---- dispatch ----------------------------------------------------------------

impl Tools {
    pub fn new(zeron: Arc<Zeron>) -> Self {
        Self {
            zeron,
            pending_turns: Default::default(),
            replays: Default::default(),
            plugins: Default::default(),
        }
    }

    pub fn side_chats_only(&self) -> bool {
        self.zeron.origin().side_chats_only
    }

    pub fn waits_on_background(&self) -> bool {
        let origin = self.zeron.origin();
        origin.chat_id.is_some() && origin.waits_on_background
    }

    pub fn guidance_in_prompt(&self) -> bool {
        self.zeron.origin().guidance_in_prompt
    }

    pub fn guidance_features(&self) -> zeron_proto::agent_guidance::Features {
        zeron_proto::agent_guidance::Features {
            visuals: self.zeron.origin().chat_id.is_some(),
            waits: self.waits_on_background(),
            side_chats_only: self.side_chats_only(),
        }
    }

    pub fn plugin_instructions(&self) -> Option<String> {
        let path = self.zeron.origin().plugin_instructions.as_ref()?;
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }

    pub fn list(&self) -> Vec<ToolDef> {
        let in_chat = self.zeron.origin().chat_id.is_some();
        let waits = self.waits_on_background();
        catalog()
            .into_iter()
            .filter(|tool| match tool.name {
                SHOW_VISUAL | SHOW_IMAGE | VISUAL_REFERENCE => in_chat,
                WAIT_ON_BACKGROUND => waits,
                _ => true,
            })
            .collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.list().iter().any(|t| t.name == name)
    }

    fn example(&self, name: &str) -> &'static str {
        catalog()
            .into_iter()
            .find(|t| t.name == name)
            .map_or("", |t| t.example)
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        let args = if args.is_null() { json!({}) } else { args };
        let result = self.dispatch(name, args).await;
        result.map_err(|error| {
            if error.code == crate::error::INVALID_ARGUMENTS {
                error.with_example(name, self.example(name))
            } else {
                error
            }
        })
    }

    async fn dispatch(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        let result = match name {
            defs::CATALOG => self.catalog(parse(args)?).await,
            defs::LIST_CHATS => self.list_chats(parse(args)?).await,
            defs::READ_CHAT => self.read_chat(parse(args)?).await,
            defs::CREATE_CHATS => self.batch(parse(args)?, true).await,
            defs::SEND_MESSAGES => self.batch(parse(args)?, false).await,
            defs::WAIT_FOR_TURN => self.wait_for_turn(parse(args)?).await,
            defs::INTERRUPT_CHAT => self.interrupt_chat(parse(args)?).await,
            defs::RESPOND_TO_INPUT => self.respond_to_input(parse(args)?).await,
            defs::ARCHIVE_CHAT => self.archive_chat(parse(args)?).await,
            SHOW_VISUAL => return self.show_visual(parse(args)?).await,
            SHOW_IMAGE => self.show_image(parse(args)?).await,
            VISUAL_REFERENCE => return visual::reference(parse(args)?),
            WAIT_ON_BACKGROUND => self.wait_on_background(parse(args)?).await,
            plugin if plugins::NAMES.contains(&plugin) => return self.plugins(plugin, args).await,
            theme if themes::NAMES.contains(&theme) => return self.themes(theme, args).await,
            plugin if is_plugin_tool_name(plugin) => return self.call_plugin_tool(plugin, args).await,
            other => {
                return Err(ToolError::new(
                    crate::error::UNKNOWN_TOOL,
                    format!("unknown tool: {other}"),
                ));
            }
        };
        result.map_err(|e| ToolError::from_anyhow(&e))
    }

    async fn batch(&self, args: BatchArgs, create: bool) -> anyhow::Result<Value> {
        if !(1..=MAX_BATCH).contains(&args.requests.len()) {
            return Err(ToolError::invalid(format!(
                "requests must hold 1 to {MAX_BATCH} items"
            ))
            .into());
        }
        let started = args.requests.iter().any(|r| {
            let field = if create { "prompt" } else { "text" };
            r.get(field).and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty())
                && r.get("wait") != Some(&json!(true))
        });
        let results = futures::future::join_all(
            args.requests
                .into_iter()
                .enumerate()
                .map(|(index, args)| self.batch_item(index, args, create)),
        )
        .await;
        let mut out = json!({ "results": results });
        if started {
            out["next"] = json!(COLLECT_NEXT);
        } else if create && out["results"].as_array().is_some_and(|r| r.iter().any(|i| i["ok"] == true && i.get("turn").is_none())) {
            out["next"] = json!("Give the new chats work with send_messages.");
        }
        Ok(out)
    }

    async fn batch_item(&self, index: usize, args: Value, create: bool) -> Value {
        let item = async {
            if create {
                let args: CreateChatArgs = parse(args)?;
                let key = args.client_request_id.clone().map(|id| format!("create:{id}"));
                self.once(key, || self.create_chat(args)).await
            } else {
                let args: SendArgs = parse(args)?;
                let key = args.client_request_id.clone().map(|id| format!("send:{id}"));
                self.once(key, || self.send_message(args)).await
            }
        };
        match item.await {
            Ok(result) => {
                let mut row = json!({ "index": index, "ok": true });
                if let (Some(row), Value::Object(fields)) = (row.as_object_mut(), result) {
                    row.extend(fields);
                }
                row
            }
            Err(error) => json!({ "index": index, "ok": false, "error": error.to_json() }),
        }
    }

    async fn once<F, Fut>(&self, key: Option<String>, run: F) -> Result<Value, ToolError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Value>>,
    {
        let Some(key) = key else {
            return run().await.map_err(|e| ToolError::from_anyhow(&e));
        };
        let cell = self.replays.lock().await.entry(key).or_default().clone();
        let mut fresh = false;
        let value = cell
            .get_or_try_init(|| async {
                fresh = true;
                run().await
            })
            .await
            .map_err(|e| ToolError::from_anyhow(&e))?;
        let mut value = value.clone();
        if !fresh {
            value["replayed"] = json!(true);
        }
        Ok(value)
    }

    async fn resolve_chat(&self, key: &str) -> anyhow::Result<Chat> {
        if key.trim().eq_ignore_ascii_case("me") {
            let id = self.zeron.origin().chat_id.clone().ok_or_else(|| {
                ToolError::invalid("\"me\" works only from an agent Zeron launched in a chat")
            })?;
            return self.zeron.resolve_chat(&id).await;
        }
        self.zeron.resolve_chat(key).await
    }

    async fn catalog(&self, args: CatalogArgs) -> anyhow::Result<Value> {
        let items = match args.kind {
            CatalogKind::Me => vec![self.catalog_self().await?],
            CatalogKind::Devices => self.catalog_devices().await?,
            CatalogKind::Projects => self.catalog_projects(args.device.as_deref()).await?,
            CatalogKind::Harnesses => self.catalog_harnesses(args.device.as_deref()).await?,
            CatalogKind::Models => {
                let harness = args.harness.as_deref().ok_or_else(|| {
                    ToolError::invalid("kind models needs harness (an id from kind harnesses)")
                })?;
                self.catalog_models(harness, args.device.as_deref()).await?
            }
        };
        let kind = serde_json::to_value(args.kind).unwrap_or_default();
        Ok(json!({ "kind": kind, "items": items }))
    }

    async fn catalog_self(&self) -> anyhow::Result<Value> {
        let origin = self.zeron.origin().clone();
        let local_device = self.zeron.local_device_id().await?;
        let engine = self.zeron.engine_info().await.unwrap_or(Value::Null);
        let chat = match origin.chat_id.as_deref() {
            Some(id) => match self.zeron.resolve_chat(id).await {
                Ok(chat) => {
                    let (spaces, sessions) =
                        tokio::try_join!(self.zeron.spaces(), self.zeron.sessions())?;
                    summarize_chat(&chat, &spaces, &sessions)
                }
                Err(_) => json!({ "id": id }),
            },
            None => Value::Null,
        };
        Ok(strip_nulls(json!({
            "chat": chat,
            "originDeviceId": origin.device_id,
            "localDeviceId": local_device,
            "workspaceScope": engine.get("workspaceScope").cloned().unwrap_or(Value::Null),
            "note": if origin.chat_id.is_some() {
                "Messages you send are attributed to this chat; it cannot message itself."
            } else {
                "Not running inside a chat: messages are sent without attribution."
            },
        })))
    }

    async fn catalog_devices(&self) -> anyhow::Result<Vec<Value>> {
        let (devices, local) =
            tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        Ok(devices
            .iter()
            .map(|d| {
                strip_nulls(json!({
                    "id": d.id,
                    "name": d.name,
                    "platform": d.platform,
                    "local": d.id == local,
                    "lastSeenAt": d.last_seen_at,
                    "version": d.version,
                }))
            })
            .collect())
    }

    async fn catalog_projects(&self, device: Option<&str>) -> anyhow::Result<Vec<Value>> {
        let (mut spaces, devices) = tokio::try_join!(self.zeron.spaces(), self.zeron.devices())?;
        if let Some(device) = device {
            let device = self.zeron.resolve_device_id(Some(device)).await?;
            spaces.retain(|s| s.device_id == device);
        }
        let device_name = |id: &str| devices.iter().find(|d| d.id == id).map(|d| d.name.clone());
        Ok(spaces
            .iter()
            .map(|s| {
                strip_nulls(json!({
                    "id": s.id,
                    "name": s.display_name(),
                    "path": s.path,
                    "deviceId": s.device_id,
                    "deviceName": device_name(&s.device_id),
                    "git": s.git_detected,
                }))
            })
            .collect())
    }

    async fn catalog_harnesses(&self, device: Option<&str>) -> anyhow::Result<Vec<Value>> {
        let device = match device {
            Some(key) => Some(self.zeron.resolve_device_id(Some(key)).await?),
            None => None,
        };
        let harnesses = self.zeron.harnesses_on(device.as_deref()).await?;
        Ok(harnesses
            .iter()
            .map(|h| {
                strip_nulls(json!({
                    "id": h.id,
                    "name": h.name,
                    "available": h.available(),
                    "connected": h.connected(),
                    "signedIn": h.signed_in,
                    "installed": h.installed,
                    "steersMidTurn": h.steers_mid_turn(),
                    "reasoningLevels": h.reasoning_levels,
                    "accessModes": h.access_modes,
                    "supportsPlan": h.supports_plan,
                }))
            })
            .collect())
    }

    async fn catalog_models(&self, harness: &str, device: Option<&str>) -> anyhow::Result<Vec<Value>> {
        let harness: HarnessId = parse_enum("harness", harness)?;
        let device = match device {
            Some(key) => Some(self.zeron.resolve_device_id(Some(key)).await?),
            None => None,
        };
        let models = self.zeron.models_on(harness, device.as_deref()).await?;
        Ok(models
            .iter()
            .map(|m| {
                strip_nulls(json!({
                    "id": m.id,
                    "harness": harness,
                    "label": m.label,
                    "description": m.description,
                    "reasoningLevels": m.reasoning_levels,
                }))
            })
            .collect())
    }

    async fn list_chats(&self, args: ListChatsArgs) -> anyhow::Result<Value> {
        let (mut chats, spaces, sessions) = tokio::try_join!(
            self.zeron.chats(),
            self.zeron.spaces(),
            self.zeron.sessions()
        )?;
        if let Some(project) = args.project.as_deref() {
            let (space, _) = self
                .zeron
                .resolve_target(Some(project), args.device.as_deref())
                .await?;
            let space = space.expect("project provided");
            chats.retain(|c| c.space_id.as_deref() == Some(space.id.as_str()));
        }
        if args.device.is_some() {
            let device = self.zeron.resolve_device_id(args.device.as_deref()).await?;
            chats.retain(|c| c.device_id == device);
        }
        if !args.include_archived {
            chats.retain(|c| !c.archived);
        }
        if let Some(parent) = args.parent.as_deref() {
            let parent = self.resolve_chat(parent).await?;
            chats.retain(|c| c.parent_chat_id.as_deref() == Some(parent.id.as_str()));
        }
        chats.sort_by(|a, b| {
            let a_at = a.last_message_at.unwrap_or(a.created_at);
            let b_at = b.last_message_at.unwrap_or(b.created_at);
            b_at.cmp(&a_at)
        });
        let total = chats.len();
        let limit = args.limit.unwrap_or(30).clamp(1, 500);
        Ok(json!({
            "total": total,
            "chats": chats.iter().take(limit)
                .map(|c| summarize_chat(c, &spaces, &sessions))
                .collect::<Vec<_>>()
        }))
    }

    async fn create_chat(&self, args: CreateChatArgs) -> anyhow::Result<Value> {
        // Parent: the explicit `parent` argument, else the chat this server
        // speaks for. Resolved so a prefix/title works and a typo fails loud.
        let explicit_parent = args
            .parent
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        anyhow::ensure!(
            args.kind != Some(ChatKind::Chat) || explicit_parent.is_none(),
            "kind chat cannot have a parent"
        );
        let parent_chat_id = if args.kind == Some(ChatKind::Chat) {
            None
        } else {
            match explicit_parent {
                Some(key) => Some(self.resolve_chat(key).await?.id),
                None => self.zeron.origin().chat_id.clone(),
            }
            // A hidden voice orchestrator creates top-level sessions.
            .filter(|parent| !zeron_proto::voice::is_orchestrator_chat(parent))
        };
        anyhow::ensure!(
            !matches!(args.kind, Some(ChatKind::Side | ChatKind::Subagent)) || parent_chat_id.is_some(),
            "kind {} requires a parent or origin chat",
            if args.kind == Some(ChatKind::Side) { "side" } else { "subagent" }
        );
        // A child is a subagent unless asked for as a side chat: delegated
        // work lists under its parent's Subagents, not its side chats.
        let kind = match (args.kind, parent_chat_id.is_some()) {
            (_, false) => ChatKind::Chat,
            (Some(ChatKind::Side), true) => ChatKind::Side,
            (_, true) => ChatKind::Subagent,
        };

        // Delegation is one level deep: a chat that has a parent never
        // creates chats, and a new chat's parent is a top-level chat.
        // The calling chat's sandbox and access cap the new one's: a
        // supervised or sandboxed agent must not hand its work to one that
        // asks nothing (the engine caps it again: rpc/agent_gate.rs).
        let mut ceiling = AccessMode::FullAccess;
        let mut sandbox_ceiling = SandboxLevel::DangerFullAccess;
        if let Some(origin) = self.zeron.origin().chat_id.as_deref()
            && !zeron_proto::voice::is_orchestrator_chat(origin)
        {
            let origin = self.zeron.resolve_chat(origin).await?;
            anyhow::ensure!(
                origin.parent_chat_id.is_none(),
                "A delegated chat can't create chats of its own. Do the work in this chat, or ask the chat that started you."
            );
            if let Some(config) = origin.config {
                ceiling = config.access;
                sandbox_ceiling = config.sandbox;
            }
        }
        if let Some(parent) = parent_chat_id.as_deref() {
            let parent = self.zeron.resolve_chat(parent).await?;
            anyhow::ensure!(
                parent.parent_chat_id.is_none(),
                "{} is itself a delegated chat: only top-level chats have side chats.",
                parent.title.as_deref().unwrap_or(&parent.id)
            );
        }
        let (space, device_id) = self
            .zeron
            .resolve_target(args.project.as_deref(), args.device.as_deref())
            .await?;
        let harnesses = self.zeron.harnesses_on(Some(&device_id)).await?;
        let harness = match args.harness.as_deref() {
            // Cloud runs its own agents, with your Cloud accounts.
            Some(raw) if args.cloud => {
                let id: HarnessId = parse_enum("harness", raw)?;
                anyhow::ensure!(
                    CLOUD_HARNESSES.contains(&id),
                    "Cloud runs Codex and Claude Code only"
                );
                id
            }
            None if args.cloud => CLOUD_HARNESSES[0],
            Some(raw) => {
                let id: HarnessId = parse_enum("harness", raw)?;
                anyhow::ensure!(
                    harnesses.iter().any(|h| h.id == id && h.available()),
                    "harness {raw} is not available on device {device_id} (catalog {{kind: \"harnesses\", device}})"
                );
                id
            }
            None => default_harness(&harnesses).with_context(|| format!("device {device_id}"))?,
        };
        // A Cloud machine's catalog is checked when it runs, not here.
        if let Some(model) = args.model.as_deref().filter(|_| !args.cloud) {
            let models = self.zeron.models_on(harness, Some(&device_id)).await?;
            anyhow::ensure!(
                models.iter().any(|m| m.id == model),
                "model {model:?} is not offered by {harness:?} on device {device_id}; available: {}",
                models
                    .iter()
                    .map(|m| m.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let reasoning: Option<ReasoningLevel> = match args.reasoning.as_deref() {
            Some(raw) => Some(parse_enum("reasoning level", raw)?),
            None => None,
        };
        let asked_sandbox: SandboxLevel = match args.sandbox.as_deref() {
            Some(raw) => parse_enum("sandbox", raw)?,
            None => SandboxLevel::WorkspaceWrite,
        };
        let asked_access: AccessMode = match args.access.as_deref() {
            Some(raw) => parse_enum("access", raw)?,
            None => ceiling,
        };
        // Never more than this chat's own: lowered, and the reply says so.
        let (sandbox, access, capped) =
            zeron_proto::cap_access((asked_sandbox, asked_access), (sandbox_ceiling, ceiling));
        let capped = capped.then(|| {
            format!(
                "This chat runs {} in a {} sandbox, so the new chat runs with no more: {}, {} sandbox (asked {}, {}).",
                ceiling.label(),
                sandbox_ceiling.label(),
                access.label(),
                sandbox.label(),
                asked_access.label(),
                asked_sandbox.label(),
            )
        });
        // A Cloud machine's catalog is checked when it runs, not here; an
        // engine predating modes lists none and runs with full access.
        if let Some(info) = harnesses.iter().find(|h| h.id == harness).filter(|_| !args.cloud) {
            let modes = &info.access_modes;
            anyhow::ensure!(
                access.is_full_access() || modes.contains(&access),
                "{harness:?} on device {device_id} cannot run {} (accessModes: {})",
                access.label(),
                modes.iter().map(|m| m.label()).collect::<Vec<_>>().join(", ")
            );
            anyhow::ensure!(
                !args.plan || info.supports_plan,
                "{harness:?} on device {device_id} has no plan mode"
            );
        }
        let config = ChatConfig {
            harness,
            model: args.model.clone(),
            reasoning,
            model_options: Default::default(),
            sandbox,
            access,
            plan: args.plan,
        };

        let chat_id = uuid::Uuid::new_v4().to_string();
        let mut mutate = json!({
            "op": "createChat",
            "chatId": chat_id,
            "deviceId": device_id,
            "config": config,
        });
        if let Some(space) = &space {
            mutate["spaceId"] = json!(space.id);
        }
        if let Some(parent) = &parent_chat_id {
            mutate["parentChatId"] = json!(parent);
            mutate["childKind"] = json!(match kind {
                ChatKind::Side => zeron_proto::ChildKind::Side,
                _ => zeron_proto::ChildKind::Subagent,
            });
        }
        if let Some(branch) = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            mutate["branch"] = json!(branch);
        }
        let mut cwd = args
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned);
        // A projectless chat runs in its own scratch folder on a host that
        // creates them; naming it here keeps createChat and the first run
        // in agreement.
        if space.is_none()
            && cwd.is_none()
            && !args.cloud
            && self.zeron.devices().await?.iter().any(|d| {
                d.id == device_id
                    && d.capabilities
                        .iter()
                        .any(|c| c == zeron_proto::capabilities::PROJECTLESS_SCRATCH_V1)
            })
        {
            cwd = Some(zeron_proto::projectless::scratch_cwd(
                &chat_id,
                chrono::Utc::now(),
            ));
        }
        if let Some(cwd) = &cwd {
            mutate["cwd"] = json!(cwd);
        }
        if args.cloud {
            mutate["cloud"] = json!(true);
        }
        let created = self.zeron.mutate(mutate).await?;
        // A Cloud chat's machine and checkout come back from the engine.
        let (device_id, cwd) = if args.cloud {
            (
                created["deviceId"]
                    .as_str()
                    .map(str::to_owned)
                    .context("the engine didn't name the Cloud machine")?,
                created["cwd"].as_str().map(str::to_owned).or(cwd),
            )
        } else {
            (device_id, cwd)
        };
        if let Some(title) = args
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            self.zeron
                .mutate(json!({ "op": "renameChat", "chatId": chat_id, "title": title }))
                .await?;
        }

        let signed_out = harnesses
            .iter()
            .find(|h| h.id == harness)
            .is_some_and(|h| h.signed_in == Some(false));
        let mut result = json!({
            "chatId": chat_id,
            "kind": kind,
            "deviceId": device_id,
            "project": space.as_ref().map(|s| json!({ "id": s.id, "name": s.display_name(), "path": s.path })),
            "harness": harness,
            "model": args.model,
            "reasoning": reasoning,
            "title": args.title,
            "parentChatId": parent_chat_id,
        });
        if let Some(capped) = &capped {
            result["accessCapped"] = json!(capped);
        }
        if signed_out {
            result["warning"] = json!(format!(
                "{harness:?} does not look signed in on this device, so its first turn may fail. Pick a connected harness (catalog {{kind: \"harnesses\"}}) if it does."
            ));
        }
        if let Some(prompt) = args.prompt.filter(|p| !p.trim().is_empty()) {
            // The row may not have folded into WatchChats yet; build the
            // chat locally from what we just wrote rather than re-reading.
            let chat = Chat {
                id: chat_id.clone(),
                device_id: device_id.clone(),
                title: args.title.clone(),
                archived: false,
                cwd: cwd.clone(),
                branch: args.branch.clone(),
                checkout_id: None,
                source_context: None,
                config: Some(config),
                last_message_preview: None,
                last_message_at: None,
                created_at: chrono::Utc::now(),
                harness_session_id: None,
                harness_session_cwd: None,
                harness_session_harness: None,
                parent_chat_id: parent_chat_id.clone(),
                child_kind: match kind {
                    ChatKind::Side => Some(zeron_proto::ChildKind::Side),
                    ChatKind::Subagent => Some(zeron_proto::ChildKind::Subagent),
                    ChatKind::Chat => None,
                },
                space_id: space.as_ref().map(|s| s.id.clone()),
                last_seen_at: None,
                room_gen: None,
                sleep: None,
            };
            let sent = self
                .deliver(&chat, space.as_ref(), &harnesses, None, prompt, "run")
                .await?;
            result["sent"] = sent;
            let pending = Arc::new(PendingTurn {
                baseline: None,
                // A new chat has no earlier messages.
                start: TurnStart::default(),
            });
            self.pending_turns
                .lock()
                .await
                .insert(chat.id.clone(), pending.clone());
            if args.wait {
                result["turn"] = self
                    .await_pending_turn(&chat, pending, wait_duration(args.timeout_secs))
                    .await?;
            }
        }
        Ok(result)
    }

    async fn read_chat(&self, args: ReadChatArgs) -> anyhow::Result<Value> {
        let chat = self.resolve_chat(&args.chat).await?;
        let (spaces, sessions, entries) = tokio::try_join!(
            self.zeron.spaces(),
            self.zeron.sessions(),
            self.zeron.transcript_on(&chat.id, Some(&chat.device_id))
        )?;
        let rendered = render_entries(
            &entries,
            RenderOptions {
                include_reasoning: args.include_reasoning,
                include_tools: args.include_tools.unwrap_or(true),
            },
        );
        let summary = summarize_chat(&chat, &spaces, &sessions);
        let stale = summary["statusStale"] == true;
        let pending = if stale { None } else { last_pending_input(&rendered) };
        let total = rendered.len();
        let limit = args.limit.unwrap_or(20).min(500);
        let end = total.saturating_sub(args.offset);
        let start = end.saturating_sub(limit);
        let window = &rendered[start..end];
        let mut out = json!({
            "chat": summary,
            "pendingInput": pending,
            "total": total,
            "returned": window.len(),
            "olderRemaining": start,
            "newerSkipped": total - end,
            "messages": window,
        });
        if out["pendingInput"].is_object() {
            out["next"] = json!("It waits on a question: answer with respond_to_input {chat, answers}.");
        }
        Ok(out)
    }

    async fn send_message(&self, args: SendArgs) -> anyhow::Result<Value> {
        let text = args.text.trim();
        if text.is_empty() {
            anyhow::bail!("text is empty");
        }
        let chat = self.resolve_chat(&args.chat).await?;
        if self.zeron.origin().chat_id.as_deref() == Some(chat.id.as_str()) {
            anyhow::bail!(
                "refusing to send a message to your own chat ({})",
                short(&chat.id)
            );
        }
        let (spaces, sessions, harnesses) = tokio::try_join!(
            self.zeron.spaces(),
            self.zeron.sessions(),
            self.zeron.harnesses_on(Some(&chat.device_id))
        )?;
        let space = chat
            .space_id
            .as_deref()
            .and_then(|id| spaces.iter().find(|s| s.id == id));
        let baseline = session_for(&sessions, &chat);
        let mode = args.mode.as_deref().unwrap_or("auto");
        let message_ids = self
            .zeron
            .transcript_on(&chat.id, Some(&chat.device_id))
            .await?
            .into_iter()
            .map(|m| m.id)
            .collect();
        let body = self.attribute(&chat, text).await;
        let mut result = json!({
            "chatId": chat.id,
            "title": chat.title,
        });
        result["sent"] = self
            .deliver(&chat, space, &harnesses, baseline.as_ref(), body, mode)
            .await?;
        let pending = Arc::new(PendingTurn {
            baseline,
            start: TurnStart { message_ids },
        });
        self.pending_turns
            .lock()
            .await
            .insert(chat.id.clone(), pending.clone());
        if args.wait {
            result["turn"] = self
                .await_pending_turn(&chat, pending, wait_duration(args.timeout_secs))
                .await?;
        }
        Ok(result)
    }

    async fn wait_for_turn(&self, args: WaitArgs) -> anyhow::Result<Value> {
        let chat = self.resolve_chat(&args.chat).await?;
        let pending = self.pending_turns.lock().await.get(&chat.id).cloned();
        let turn = match pending {
            Some(pending) => {
                self.await_pending_turn(&chat, pending, wait_duration(args.timeout_secs))
                    .await?
            }
            None => {
                self.await_turn(
                    &chat,
                    None,
                    false,
                    wait_duration(args.timeout_secs),
                    TurnStart::default(),
                )
                .await?
            }
        };
        let mut out = json!({ "chatId": chat.id, "title": chat.title, "turn": turn });
        if out["turn"]["timedOut"] == true {
            out["next"] = json!("Still working. Call wait_for_turn again, or do other work first.");
        } else if out["turn"]["pendingInput"].is_object() {
            out["next"] = json!("It waits on a question: answer with respond_to_input {chat, answers}.");
        }
        Ok(out)
    }

    async fn archive_chat(&self, args: ArchiveArgs) -> anyhow::Result<Value> {
        let chat = self.resolve_chat(&args.chat).await?;
        let archived = args.archived.unwrap_or(true);
        self.zeron
            .mutate(json!({ "op": "setChatArchived", "chatId": chat.id, "archived": archived }))
            .await?;
        Ok(json!({ "chatId": chat.id, "title": chat.title, "archived": archived }))
    }

    async fn interrupt_chat(&self, args: ChatArgs) -> anyhow::Result<Value> {
        let chat = self.resolve_chat(&args.chat).await?;
        let command_id = self
            .zeron
            .queue_command(&chat.id, &SessionCommandPayload::Interrupt {})
            .await?;
        Ok(json!({ "chatId": chat.id, "commandId": command_id }))
    }

    async fn respond_to_input(&self, args: RespondArgs) -> anyhow::Result<Value> {
        let chat = self.resolve_chat(&args.chat).await?;
        let request_id = match args.request_id {
            Some(id) => id,
            None => {
                let entries = self
                    .zeron
                    .transcript_on(&chat.id, Some(&chat.device_id))
                    .await?;
                let rendered = render_entries(&entries, RenderOptions::default());
                last_pending_input(&rendered)
                    .and_then(|p| {
                        p.get("requestId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("chat {} has no pending question", short(&chat.id))
                    })?
            }
        };
        if args.answers.is_empty() {
            anyhow::bail!("answers is empty");
        }
        let answers = args
            .answers
            .into_iter()
            .map(|a| UserInputAnswer {
                question_id: a.question_id,
                labels: a.labels,
            })
            .collect();
        let command_id = self
            .zeron
            .queue_command(
                &chat.id,
                &SessionCommandPayload::RespondInput {
                    request_id: request_id.clone(),
                    answers,
                },
            )
            .await?;
        Ok(json!({ "chatId": chat.id, "requestId": request_id, "commandId": command_id }))
    }

    // ---- shared pieces -------------------------------------------------------

    /// Prefix the sender's identity when this server speaks for a chat, so
    /// the receiving agent (and the human reading that transcript) can tell
    /// an agent-to-agent message from a typed one. Reply instructions always
    /// use the full id: display prefixes need not uniquely identify a chat.
    async fn attribute(&self, target: &Chat, text: &str) -> String {
        let Some(origin_id) = self.zeron.origin().chat_id.as_deref() else {
            return text.to_owned();
        };
        if origin_id == target.id {
            return text.to_owned();
        }
        let title = match self.zeron.resolve_chat(origin_id).await {
            Ok(chat) => chat.title,
            Err(_) => None,
        };
        let label = match title {
            Some(title) if !title.trim().is_empty() => {
                format!("{} ({})", title.trim(), short(origin_id))
            }
            _ => short(origin_id).to_owned(),
        };
        zeron_proto::agent_messages::agent_message(&label, origin_id, text)
    }

    /// Pick and perform the delivery the composer would.
    async fn deliver(
        &self,
        chat: &Chat,
        space: Option<&Space>,
        harnesses: &[HarnessInfo],
        session: Option<&Session>,
        text: String,
        mode: &str,
    ) -> anyhow::Result<Value> {
        let harness = chat
            .config
            .as_ref()
            .map(|c| c.harness)
            .map_or_else(|| default_harness(harnesses), Ok)?;
        let (status, _) = status_of(session);
        // A quiet tool call can outlive the UI's stale-status window. Route
        // through steering and let the host decide whether a live run exists.
        let live = session.map(|s| s.status).unwrap_or(SessionStatus::Idle);
        let chosen = match mode {
            "auto" => match live {
                SessionStatus::Idle | SessionStatus::Errored => "run",
                SessionStatus::Working => "steer",
                SessionStatus::AwaitingInput => anyhow::bail!(
                    "chat {} is waiting for an answer; use respond_to_input (or mode 'queue' to hold this message for after the turn)",
                    short(&chat.id)
                ),
            },
            "run" | "steer" | "queue" => mode,
            other => anyhow::bail!("unknown mode {other:?}; expected auto, run, steer, or queue"),
        };
        let id = match chosen {
            "run" => {
                let config = chat.config.clone();
                let cwd = chat
                    .cwd
                    .clone()
                    .or_else(|| space.map(|s| s.path.clone()))
                    .unwrap_or_else(|| "~".into());
                let request = RunRequest {
                    mcp: None,
                    prompt: text,
                    harness: Some(harness),
                    model: config.as_ref().and_then(|c| c.model.clone()),
                    reasoning: config.as_ref().and_then(|c| c.reasoning),
                    model_options: config
                        .as_ref()
                        .map(|c| c.model_options.clone())
                        .unwrap_or_default(),
                    cwd,
                    sandbox: config
                        .as_ref()
                        .map(|c| c.sandbox)
                        .unwrap_or(SandboxLevel::WorkspaceWrite),
                    auto_approve: false,
                    access: config.as_ref().map(|c| c.access).unwrap_or_default(),
                    plan: config.as_ref().is_some_and(|c| c.plan),
                    resume: None,
                    attachments: Vec::new(),
                    worktree: None,
                    env: Default::default(),
                    extra_mcp: Vec::new(),
                };
                self.zeron
                    .queue_command(
                        &chat.id,
                        &SessionCommandPayload::Run {
                            request,
                            message_id: uuid::Uuid::new_v4().to_string(),
                        },
                    )
                    .await?
            }
            "steer" => {
                self.zeron
                    .queue_command(
                        &chat.id,
                        &SessionCommandPayload::Steer {
                            prompt: text,
                            message_id: Some(uuid::Uuid::new_v4().to_string()),
                        },
                    )
                    .await?
            }
            _ => self.zeron.queue_message(&chat.id, &text).await?,
        };
        Ok(json!({
            "delivery": chosen,
            "id": id,
            "chatStatusAtSend": status,
        }))
    }

    async fn await_pending_turn(
        &self,
        chat: &Chat,
        pending: Arc<PendingTurn>,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let turn = self
            .await_turn(
                chat,
                pending.baseline.as_ref(),
                true,
                timeout,
                pending.start.clone(),
            )
            .await?;
        if turn["timedOut"] == false {
            let mut turns = self.pending_turns.lock().await;
            // A concurrent send may already have installed a newer baseline.
            if turns
                .get(&chat.id)
                .is_some_and(|p| Arc::ptr_eq(p, &pending))
            {
                turns.remove(&chat.id);
            }
        }
        Ok(turn)
    }

    /// Wait, then report the outcome with the assistant messages that
    /// are not in `start`'s baseline.
    async fn await_turn(
        &self,
        chat: &Chat,
        baseline: Option<&Session>,
        expect_turn: bool,
        timeout: Duration,
        start: TurnStart,
    ) -> anyhow::Result<Value> {
        let deadline = Instant::now() + timeout;
        let (mut outcome, session) = self
            .zeron
            .wait_for_turn(chat, baseline, expect_turn, timeout)
            .await?;
        // Registry session updates can arrive before the separate transcript doc.
        // Keep the same deadline while waiting for a response to this send.
        let mut rendered = Vec::new();
        let mut replies = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                if outcome == TurnOutcome::Completed && expect_turn {
                    outcome = TurnOutcome::TimedOut;
                }
                break;
            }
            let entries = match tokio::time::timeout(
                remaining,
                self.zeron.transcript_on(&chat.id, Some(&chat.device_id)),
            )
            .await
            {
                Ok(entries) => entries?,
                Err(_) => {
                    outcome = TurnOutcome::TimedOut;
                    break;
                }
            };
            rendered = render_entries(&entries, RenderOptions::default());
            replies = rendered
                .iter()
                .filter(|m| m.role == zeron_doc::MessageRole::Assistant)
                .filter(|m| !start.message_ids.contains(&m.id))
                .cloned()
                .collect();
            if !expect_turn
                || outcome != TurnOutcome::Completed
                || replies
                    .iter()
                    .any(|m| m.status != Some(zeron_doc::MessageStatus::Streaming))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100).min(remaining)).await;
        }
        let now = chrono::Utc::now();
        let status = zeron_proto::view::display_status(chat, session.as_ref(), now);
        let stale = session
            .as_ref()
            .is_some_and(|s| zeron_proto::view::session_stale(s, now));
        Ok(json!({
            "outcome": outcome,
            "status": status,
            "sessionStatus": session.as_ref().map(|s| s.status),
            "lastTurnOutcome": session.as_ref().and_then(|s| s.last_turn_outcome),
            "statusStale": stale,
            "terminalActivity": session.as_ref().and_then(|s| s.terminal_activity.as_ref()),
            "timedOut": outcome == TurnOutcome::TimedOut,
            "pendingInput": if stale { None } else { last_pending_input(&rendered) },
            "replies": replies,
        }))
    }
}

/// claude-code when it is ready here, else the first ready harness — a
/// provider that is installed but signed out only when nothing else is.
fn default_harness(harnesses: &[HarnessInfo]) -> anyhow::Result<HarnessId> {
    let real = |h: &&HarnessInfo| h.id != HarnessId::Mock;
    harnesses
        .iter()
        .find(|h| h.id == HarnessId::ClaudeCode && h.connected())
        .or_else(|| harnesses.iter().filter(real).find(|h| h.connected()))
        .or_else(|| {
            harnesses
                .iter()
                .find(|h| h.id == HarnessId::ClaudeCode && h.available())
        })
        .or_else(|| {
            harnesses
                .iter()
                .find(|h| h.available() && h.id != HarnessId::Mock)
        })
        .map(|h| h.id)
        .ok_or_else(|| {
            anyhow::anyhow!("no harness is available on this device (catalog {{kind: \"harnesses\"}})")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Tools {
        async fn legacy(&self, name: &str, args: Value) -> Result<Value, ToolError> {
            let one = |tool: &'static str| (tool, json!({ "requests": [args.clone()] }));
            let (tool, call) = match name {
                "create_chat" => one(defs::CREATE_CHATS),
                "send_message" => one(defs::SEND_MESSAGES),
                _ => {
                    let kind = match name {
                        "whoami" => "self",
                        "list_devices" => "devices",
                        "list_projects" => "projects",
                        "list_harnesses" => "harnesses",
                        _ => "models",
                    };
                    let mut call = args.clone();
                    call["kind"] = json!(kind);
                    let out = self.call(defs::CATALOG, call).await?;
                    let items = out["items"].clone();
                    return Ok(match kind {
                        "self" => items[0].clone(),
                        "models" => json!({ "harness": args["harness"], "models": items }),
                        other => json!({ other: items }),
                    });
                }
            };
            let out = self.call(tool, call).await?;
            let row = out["results"][0].clone();
            if row["ok"] == true {
                let mut row = row;
                let fields = row.as_object_mut().unwrap();
                fields.remove("index");
                fields.remove("ok");
                Ok(row)
            } else {
                Err(ToolError {
                    code: crate::error::REJECTED,
                    message: row["error"]["message"].as_str().unwrap_or_default().to_owned(),
                })
            }
        }
    }

    use crate::zeron::Origin;
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::Mutex;
    use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client, methods};

    /// Two devices with distinct catalogs and repeated project paths, plus
    /// two chats with a two-message transcript. Writes are recorded for assertions.
    #[derive(Default)]
    struct World {
        writes: Mutex<Vec<(String, Value)>>,
        dispatch_barrier: Option<tokio::sync::Barrier>,
        beta_parent: Option<String>,
        /// Adds two more chats below Beta: Beta → Gamma → Delta.
        deep: bool,
        /// `ListAgentAccounts` reply; absent = the method is unknown.
        accounts: Option<Value>,
        remote_accounts: Option<Value>,
        beta_remote: bool,
        extra_chats: Vec<Value>,
        catalog_error: Option<&'static str>,
        reads: Mutex<Vec<(String, Value)>>,
    }

    fn stream(item: Value) -> RpcReply {
        RpcReply::Stream(futures::stream::iter(vec![item]).boxed())
    }

    #[async_trait]
    impl RpcService for World {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            if matches!(
                method,
                methods::LIST_HARNESSES | methods::LIST_MODELS | methods::LIST_AGENT_ACCOUNTS
            ) {
                self.reads
                    .lock()
                    .unwrap()
                    .push((method.into(), params.clone()));
                if self.catalog_error == Some(method) {
                    return Err(RpcError::Failed("catalog unavailable".into()));
                }
            }
            let remote = params["targetDeviceId"] == "dev-remote";
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::ENGINE_INFO => RpcReply::Value(json!({
                    "deviceId": "dev-local", "workspaceScope": "local"
                })),
                methods::WATCH_DEVICES => stream(json!([{
                    "id": "dev-local", "name": "Laptop", "platform": "linux",
                    "lastSeenAt": null
                }, {"id": "dev-remote", "name": "Worker", "platform": "linux", "lastSeenAt": null}])),
                methods::WATCH_SPACES => stream(json!([{
                    "id": "space-1", "deviceId": "dev-local", "path": "/repo/comet",
                    "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                }, {
                    "id": "space-remote", "deviceId": "dev-remote", "path": "/repo/comet",
                    "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                }, {
                    "id": "space-unique", "deviceId": "dev-remote", "path": "/repo/unique",
                    "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                }])),
                methods::WATCH_CHATS => {
                    let mut chats = json!([
                    {
                        "id": "chat-alpha-1", "deviceId": "dev-local", "title": "Alpha",
                        "archived": false, "spaceId": "space-1",
                        "config": { "harness": "claude-code", "model": "opus", "reasoning": null, "sandbox": "workspace-write" },
                        "createdAt": "2026-09-01T00:00:00Z"
                    },
                    {
                        "id": "chat-beta-2", "deviceId": if self.beta_remote { "dev-remote" } else { "dev-local" }, "title": "Beta",
                        "parentChatId": self.beta_parent,
                        "archived": false, "spaceId": "space-1",
                        "createdAt": "2026-09-02T00:00:00Z"
                    }
                    ]);
                    if self.deep {
                        for (id, title, parent) in [
                            ("chat-gamma-3", "Gamma", "chat-beta-2"),
                            ("chat-delta-4", "Delta", "chat-gamma-3"),
                        ] {
                            chats.as_array_mut().unwrap().push(json!({
                                "id": id, "deviceId": "dev-local", "title": title,
                                "parentChatId": parent, "archived": false, "spaceId": "space-1",
                                "createdAt": "2026-09-03T00:00:00Z"
                            }));
                        }
                    }
                    chats
                        .as_array_mut()
                        .unwrap()
                        .extend(self.extra_chats.clone());
                    stream(chats)
                }
                methods::LIST_AGENT_ACCOUNTS if remote && self.remote_accounts.is_some() => {
                    RpcReply::Value(self.remote_accounts.clone().unwrap())
                }
                methods::LIST_AGENT_ACCOUNTS if self.accounts.is_some() && !remote => {
                    RpcReply::Value(self.accounts.clone().unwrap())
                }
                methods::WATCH_SESSIONS => stream(json!([])),
                methods::LIST_HARNESSES if remote => RpcReply::Value(json!([
                    {"id":"codex", "name":"Remote Codex", "installed":true},
                    {"id":"cursor", "name":"Disabled Cursor", "installed":true, "enabled":false}
                ])),
                methods::LIST_HARNESSES => RpcReply::Value(json!([
                    { "id": "claude-code", "name": "Claude Code", "supportsSteering": true,
                      "steeringMode": "step-boundary", "reasoningLevels": [], "installed": true, "enabled": true,
                      "accessModes": ["supervised", "auto-accept-edits", "full-access"], "supportsPlan": true },
                    { "id": "codex", "name": "Codex", "supportsSteering": true,
                      "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": false, "enabled": true }
                ])),
                methods::LIST_MODELS if remote => RpcReply::Value(json!([
                    {"id":"remote-model", "label":"Remote model"}
                ])),
                methods::LIST_MODELS => RpcReply::Value(json!([
                    { "id": "opus", "label": "Opus" }, { "id": "sonnet", "label": "Sonnet" }
                ])),
                methods::WATCH_DOC_MESSAGES => stream(json!({ "reset": [
                    { "id": "u1", "role": "user", "createdAt": 1, "deviceId": "dev-local",
                      "parts": [{ "kind": "text", "id": "t", "text": "hi" }] },
                    { "id": "a1", "role": "assistant", "createdAt": 2, "deviceId": "dev-local",
                      "status": "complete",
                      "parts": [{ "kind": "text", "id": "t", "text": "hello back" }] }
                ]})),
                methods::PUBLISH_VISUAL => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params.clone()));
                    RpcReply::Value(json!({ "id": "v:1", "title": params["title"] }))
                }
                methods::PUBLISH_IMAGE => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params.clone()));
                    RpcReply::Value(json!({ "id": "img:1", "name": params["name"] }))
                }
                methods::MUTATE | methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE => {
                    // A Cloud chat gets a machine of its own from the engine.
                    let cloud_create = method == methods::MUTATE
                        && params["op"] == "createChat"
                        && params["cloud"] == true;
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params));
                    if cloud_create {
                        return Ok(RpcReply::Value(json!({
                            "ok": true, "deviceId": "cloud-child", "cwd": "/home/user/app",
                        })));
                    }
                    if method == methods::QUEUE_COMMAND
                        && let Some(barrier) = &self.dispatch_barrier
                    {
                        // No dispatch can finish until both chats have work.
                        // A sequential batch deadlocks here, before any waits.
                        barrier.wait().await;
                    }
                    RpcReply::Value(json!({ "commandId": "cmd-1", "id": "q-1" }))
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn tools(world: Arc<World>, origin: Origin) -> Tools {
        let client = memory_client(world);
        Tools::new(Arc::new(Zeron::with_client(client, origin)))
    }

    #[tokio::test]
    async fn catalog_is_well_formed() {
        let defs = catalog();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), defs.len(), "tool names must be unique");
        for def in &defs {
            assert_eq!(def.input_schema["type"], "object", "{}", def.name);
            assert!(!def.description.is_empty());
            assert!(def.name.len() <= 63, "{}", def.name);
            assert!(
                def.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{}",
                def.name
            );
            assert!(!def.title.is_empty(), "{}", def.name);
            let cap = if def.name == SHOW_VISUAL { 3_072 } else { 1_024 };
            assert!(def.description.len() <= cap, "{}: {}", def.name, def.description.len());
            assert_eq!(def.always_load, def.name == WAIT_ON_BACKGROUND, "{}", def.name);
            let example: Value = serde_json::from_str(def.example).unwrap_or_else(|e| panic!("{}: {e}", def.name));
            assert!(example.is_object(), "{}", def.name);
            if def.hints.read_only {
                assert!(!def.hints.destructive, "{}", def.name);
            }
            if let Some(output) = &def.output_schema {
                assert_eq!(output["type"], "object", "{}", def.name);
            }
        }
        let listed: Vec<ListedTool> = defs.into_iter().map(ListedTool::from).collect();
        for tool in &listed {
            let annotations = tool.annotations.as_ref().unwrap();
            for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
                assert!(annotations[hint].is_boolean(), "{} {hint}", tool.name);
            }
            assert_eq!(annotations["title"], json!(tool.title), "{}", tool.name);
        }
        let always: Vec<&str> = listed
            .iter()
            .filter(|t| t.meta.as_ref().is_some_and(|m| m["anthropic/alwaysLoad"] == true))
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(always, [WAIT_ON_BACKGROUND]);
    }

    #[tokio::test]
    async fn structured_results_match_their_output_schemas() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let defs = catalog();
        let calls = [
            (defs::CATALOG, json!({ "kind": "devices" })),
            (defs::LIST_CHATS, json!({})),
            (defs::READ_CHAT, json!({ "chat": "alpha", "limit": 0 })),
            (defs::CREATE_CHATS, json!({ "requests": [{ "project": "space-1" }] })),
            (defs::WAIT_FOR_TURN, json!({ "chat": "alpha", "timeout_secs": 1 })),
        ];
        for (name, args) in calls {
            let out = tools.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}"));
            let schema = defs.iter().find(|d| d.name == name).unwrap().output_schema.clone().unwrap();
            for key in schema["required"].as_array().unwrap() {
                assert!(out.get(key.as_str().unwrap()).is_some(), "{name} lacks {key}: {out}");
            }
            for (key, prop) in schema["properties"].as_object().unwrap() {
                let (Some(value), Some(kind)) = (out.get(key), prop.get("type")) else { continue };
                let kinds: Vec<&str> = match kind {
                    Value::String(k) => vec![k.as_str()],
                    Value::Array(ks) => ks.iter().filter_map(Value::as_str).collect(),
                    _ => vec![],
                };
                let actual = match value {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                    Value::Number(_) => "number",
                    Value::String(_) => "string",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                };
                assert!(kinds.contains(&actual) || (actual == "integer" && kinds.contains(&"number")), "{name}.{key}: {actual} not in {kinds:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_repeated_client_request_id_replays_instead_of_creating_twice() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let request = json!({ "requests": [{ "project": "space-1", "clientRequestId": "docs-1" }] });
        let first = tools.call(defs::CREATE_CHATS, request.clone()).await.unwrap();
        let again = tools.call(defs::CREATE_CHATS, request).await.unwrap();
        assert_eq!(first["results"][0]["chatId"], again["results"][0]["chatId"]);
        assert!(first["results"][0].get("replayed").is_none());
        assert_eq!(again["results"][0]["replayed"], true);
        let creates = world.writes.lock().unwrap().iter().filter(|(_, p)| p["op"] == "createChat").count();
        assert_eq!(creates, 1);
    }

    #[tokio::test]
    async fn a_mistyped_chat_gets_a_did_you_mean_and_a_code() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let error = tools.call(defs::READ_CHAT, json!({ "chat": "Alpah" })).await.unwrap_err();
        assert_eq!(error.code, crate::error::NOT_FOUND);
        assert!(error.message.contains("Did you mean \"Alpha\""), "{error}");
        let me = tools.call(defs::READ_CHAT, json!({ "chat": "me" })).await.unwrap_err();
        assert_eq!(me.code, crate::error::INVALID_ARGUMENTS);
    }

    #[tokio::test]
    async fn an_async_create_says_how_to_collect_and_never_to_end_the_turn() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let out = tools
            .call(defs::CREATE_CHATS, json!({ "requests": [{ "project": "space-1", "prompt": "go" }] }))
            .await
            .unwrap();
        let next = out["next"].as_str().unwrap();
        assert!(next.contains("wait_for_turn") && !next.contains("end your turn"), "{next}");
        let idle = tools
            .call(defs::CREATE_CHATS, json!({ "requests": [{ "project": "space-1" }] }))
            .await
            .unwrap();
        assert!(idle["next"].as_str().unwrap().contains("send_messages"));
    }

    #[test]
    fn visual_reference_answers_topics_and_its_examples_pass_the_check() {
        let list = visual::reference(Default::default()).unwrap();
        assert!(list["topics"].as_array().unwrap().len() >= 10);
        let basics = visual::reference(serde_json::from_value(json!({ "topic": "basics" })).unwrap()).unwrap();
        let body = basics["reference"].as_str().unwrap();
        let code = body.split("```lua\n").nth(1).unwrap().split("```").next().unwrap();
        zeron_visual::check(code, 1).unwrap();
        let skill = zeron_proto::visual_reference::skill_markdown();
        for name in zeron_visual::TOKEN_NAMES {
            assert!(skill.contains(name), "{name}");
        }
        assert!(visual::reference(serde_json::from_value(json!({ "topic": "nope" })).unwrap()).is_err());
    }

    #[test]
    fn the_show_visual_example_passes_its_own_check() {
        let start = visual::SHOW_VISUAL_DESCRIPTION.find("local s = ui.state").unwrap();
        let end = start
            + visual::SHOW_VISUAL_DESCRIPTION[start..]
                .find("\n\nZeron runs the script")
                .unwrap();
        let report = zeron_visual::check(&visual::SHOW_VISUAL_DESCRIPTION[start..end], 1).unwrap();
        assert_eq!(report.interactions, 2, "both tabs");
    }

    #[tokio::test]
    async fn show_visual_is_offered_only_inside_a_chat() {
        let world = Arc::new(World::default());
        let outside = tools(world.clone(), Origin::default());
        assert!(!outside.has("show_visual"));
        assert!(
            outside
                .call(
                    "show_visual",
                    json!({ "script": "return ui.text 'x'", "title": "x" })
                )
                .await
                .is_err()
        );

        let inside = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                device_id: Some("dev-local".into()),
                ..Default::default()
            },
        );
        assert!(inside.has("show_visual"));
        // The guidance names every color the runtime accepts.
        for name in zeron_visual::TOKEN_NAMES {
            assert!(visual::SHOW_VISUAL_DESCRIPTION.contains(name), "{name}");
        }
        let shown = inside
            .call(
                "show_visual",
                json!({ "script": "return ui.text 'chart'", "title": "Chart" }),
            )
            .await
            .unwrap();
        assert_eq!(shown["visual"]["title"], "Chart");
        let writes = world.writes.lock().unwrap();
        let (method, params) = writes.last().unwrap();
        assert_eq!(method, methods::PUBLISH_VISUAL);
        assert_eq!(params["chatId"], "chat-alpha-1");
        assert_eq!(params["script"], "return ui.text 'chart'");
    }

    #[tokio::test]
    async fn show_image_sends_the_file_only_from_inside_a_chat() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("before.png");
        std::fs::write(&file, b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let path = file.to_string_lossy().into_owned();
        let world = Arc::new(World::default());
        let outside = tools(world.clone(), Origin::default());
        assert!(!outside.has("show_image"));
        assert!(!outside.has("wait_on_background"), "a wait is a chat's agent's to say");
        assert!(
            outside
                .call("show_image", json!({ "path": path }))
                .await
                .is_err()
        );

        let inside = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                device_id: Some("dev-local".into()),
                ..Default::default()
            },
        );
        assert!(inside.has("show_image"));
        // (Offered only where the runtime wakes its agent: its own test.)
        assert!(!inside.has("wait_on_background"));
        let shown = inside
            .call("show_image", json!({ "path": path }))
            .await
            .unwrap();
        assert_eq!(shown["image"]["name"], "before.png");
        {
            let writes = world.writes.lock().unwrap();
            let (method, params) = writes.last().unwrap();
            assert_eq!(method, methods::PUBLISH_IMAGE);
            assert_eq!(params["chatId"], "chat-alpha-1");
            let data = base64::engine::general_purpose::STANDARD
                .decode(params["data"].as_str().unwrap())
                .unwrap();
            assert_eq!(data, b"\x89PNG\r\n\x1a\nfixture");
        }

        let missing = inside
            .call("show_image", json!({ "path": "/nonexistent/shot.png" }))
            .await
            .unwrap_err();
        assert!(
            missing.contains("can't read /nonexistent/shot.png"),
            "{missing}"
        );
        let huge = dir.path().join("huge.png");
        std::fs::File::create(&huge)
            .unwrap()
            .set_len(zeron_proto::IMAGE_MAX_BYTES as u64 + 1)
            .unwrap();
        let error = inside
            .call("show_image", json!({ "path": huge.to_string_lossy() }))
            .await
            .unwrap_err();
        assert!(error.contains("over 10 MB"), "{error}");
        assert_eq!(
            world.writes.lock().unwrap().len(),
            1,
            "nothing else was sent"
        );
    }

    #[tokio::test]
    async fn list_and_read_chats() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let listed = tools.call("list_chats", json!({})).await.unwrap();
        assert_eq!(listed["total"], 2);
        // Newest activity first: Beta was created later.
        assert_eq!(listed["chats"][0]["title"], "Beta");
        assert_eq!(listed["chats"][1]["project"]["name"], "comet");
        assert_eq!(listed["chats"][1]["status"], "idle");

        let read = tools
            .call("read_chat", json!({ "chat": "alpha" }))
            .await
            .unwrap();
        assert_eq!(read["total"], 2);
        assert_eq!(read["messages"][1]["text"], "hello back");

        let read = tools
            .call("read_chat", json!({ "chat": "chat-alpha", "limit": 1 }))
            .await
            .unwrap();
        assert_eq!(read["returned"], 1);
        assert_eq!(read["olderRemaining"], 1);
        assert_eq!(read["messages"][0]["id"], "a1");
    }

    #[tokio::test]
    async fn send_attributes_and_refuses_self() {
        let world = Arc::new(World::default());
        let tools = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-beta-2".into()),
                device_id: Some("dev-local".into()),
                ..Default::default()
            },
        );
        let err = tools
            .legacy("send_message", json!({ "chat": "beta", "text": "loop" }))
            .await
            .unwrap_err();
        assert!(err.contains("own chat"), "{err}");

        let sent = tools
            .legacy(
                "send_message",
                json!({ "chat": "alpha", "text": "please review" }),
            )
            .await
            .unwrap();
        assert_eq!(sent["sent"]["delivery"], "run");
        let writes = world.writes.lock().unwrap();
        let (method, params) = writes.last().expect("a queued command");
        assert_eq!(method, methods::QUEUE_COMMAND);
        assert_eq!(params["chatId"], "chat-alpha-1");
        assert_eq!(params["command"]["kind"], "run");
        let prompt = params["command"]["request"]["prompt"].as_str().unwrap();
        assert!(
            prompt.starts_with("[Message from Zeron chat Beta (chat-bet)"),
            "{prompt}"
        );
        assert!(prompt.ends_with("please review"));
        assert!(prompt.contains("tool, chat chat-beta-2.]"), "{prompt}");
        assert_eq!(params["command"]["request"]["cwd"], "/repo/comet");
        assert_eq!(params["command"]["request"]["harness"], "claude-code");
        assert_eq!(params["command"]["request"]["model"], "opus");
    }

    #[tokio::test]
    async fn replies_use_the_full_id_with_multiple_voice_orchestrators() {
        let first = "voice-orchestrator-11111111-1111-4111-8111-111111111111";
        let second = "voice-orchestrator-22222222-2222-4222-8222-222222222222";
        let world = Arc::new(World {
            extra_chats: [first, second]
                .into_iter()
                .map(|id| {
                    json!({
                        "id": id, "deviceId": "dev-local", "title": "Voice session",
                        "archived": false, "createdAt": "2026-10-01T00:00:00Z"
                    })
                })
                .collect(),
            ..Default::default()
        });
        let orchestrator = tools(
            world.clone(),
            Origin {
                chat_id: Some(second.into()),
                device_id: None,
                ..Default::default()
            },
        );
        let error = orchestrator
            .zeron
            .resolve_chat("voice-or")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("2 chats start with"), "{error}");
        orchestrator
            .legacy(
                "send_message",
                json!({
                    "chat": "alpha", "text": "please review"
                }),
            )
            .await
            .unwrap();
        let reply_id = {
            let writes = world.writes.lock().unwrap();
            let prompt = writes.last().unwrap().1["command"]["request"]["prompt"]
                .as_str()
                .unwrap();
            prompt
                .split_once("tool, chat ")
                .unwrap()
                .1
                .split_once(".]\n\n")
                .unwrap()
                .0
                .to_owned()
        };
        assert_eq!(reply_id, second);
        let worker = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                device_id: None,
                ..Default::default()
            },
        );
        worker
            .legacy(
                "send_message",
                json!({
                    "chat": reply_id, "text": "review complete"
                }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.last().unwrap().1["chatId"], second);
    }

    #[tokio::test]
    async fn auto_steers_busy_chats_even_at_turn_boundaries_or_after_long_quiet_tools() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let chats = tools.zeron.chats().await.unwrap();
        let session = Session {
            chat_id: chats[0].id.clone(),
            device_id: "dev-local".into(),
            status: SessionStatus::Working,
            started_at: None,
            updated_at: chrono::Utc::now() - chrono::Duration::minutes(10),
            last_completed_turn: None,
            last_turn_outcome: None,
            terminal_activity: None,
            running_subagents: 0,
            background_tasks: Vec::new(),
            waiting: None,
        };
        // No mid-turn capability is required for a live mailbox delivery.
        let sent = tools
            .deliver(
                &chats[0],
                None,
                &[],
                Some(&session),
                "follow up".into(),
                "auto",
            )
            .await
            .unwrap();
        assert_eq!(sent["delivery"], "steer");
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, methods::QUEUE_COMMAND);
        assert_eq!(writes[0].1["command"]["kind"], "steer");
    }

    #[tokio::test]
    async fn create_chat_writes_the_row_and_validates_model() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let err = tools
            .legacy(
                "create_chat",
                json!({ "project": "space-1", "model": "nope" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("not offered"), "{err}");

        let created = tools
            .legacy(
                "create_chat",
                json!({ "project": "space-1", "model": "sonnet", "title": "Review", "prompt": "go" }),
            )
            .await
            .unwrap();
        let chat_id = created["chatId"].as_str().unwrap().to_owned();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 3, "createChat, renameChat, run");
        assert_eq!(writes[0].1["op"], "createChat");
        assert_eq!(writes[0].1["chatId"], chat_id);
        assert_eq!(writes[0].1["spaceId"], "space-1");
        assert!(
            writes[0].1.get("parentChatId").is_none(),
            "no origin, no parent"
        );
        assert_eq!(writes[0].1["config"]["harness"], "claude-code");
        assert_eq!(writes[0].1["config"]["model"], "sonnet");
        assert_eq!(writes[1].1["op"], "renameChat");
        assert_eq!(writes[2].0, methods::QUEUE_COMMAND);
        assert_eq!(writes[2].1["command"]["request"]["prompt"], "go");
    }

    #[tokio::test]
    async fn create_chat_records_the_origin_as_parent() {
        let world = Arc::new(World::default());
        let tools = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-beta-2".into()),
                device_id: None,
                ..Default::default()
            },
        );
        let created = tools
            .legacy("create_chat", json!({ "project": "space-1" }))
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-beta-2");
        assert_eq!(
            world.writes.lock().unwrap()[0].1["parentChatId"],
            "chat-beta-2"
        );

        // An explicit parent (by title) overrides the origin.
        let created = tools
            .legacy(
                "create_chat",
                json!({ "project": "space-1", "parent": "Alpha" }),
            )
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-alpha-1");
        let err = tools
            .legacy(
                "create_chat",
                json!({ "project": "space-1", "parent": "nope" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("no chat matches"), "{err}");
    }

    #[tokio::test]
    async fn wait_on_an_idle_chat_returns_immediately() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let waited = tools
            .call(
                "wait_for_turn",
                json!({ "chat": "alpha", "timeout_secs": 5 }),
            )
            .await
            .unwrap();
        assert_eq!(waited["turn"]["outcome"], "completed");
        assert_eq!(waited["turn"]["replies"][0]["text"], "hello back");
    }

    #[tokio::test]
    async fn send_with_wait_keeps_waiting_until_a_turn_lands() {
        // The stub never grows a session row, so a send-then-wait must run
        // to its deadline rather than declaring the (unstarted) run done.
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let started = std::time::Instant::now();
        let sent = tools
            .legacy(
                "send_message",
                json!({ "chat": "alpha", "text": "hi", "wait": true, "timeout_secs": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(sent["turn"]["outcome"], "timedOut");
        assert!(started.elapsed() >= Duration::from_millis(900));
    }

    #[tokio::test]
    async fn batches_dispatch_all_chats_before_waiting_and_keep_partial_results() {
        for (name, requests) in [
            (
                "create_chats",
                json!([
                    {"prompt": "first", "wait": true, "timeout_secs": 1},
                    {"harness": "not-a-harness"},
                    {"prompt": "second", "wait": true, "timeout_secs": 1},
                ]),
            ),
            (
                "send_messages",
                json!([
                    {"chat": "alpha", "text": "first", "wait": true, "timeout_secs": 1},
                    {"chat": "missing", "text": "invalid"},
                    {"chat": "beta", "text": "second", "wait": true, "timeout_secs": 1},
                ]),
            ),
        ] {
            let world = Arc::new(World {
                dispatch_barrier: Some(tokio::sync::Barrier::new(2)),
                ..Default::default()
            });
            let tools = tools(world.clone(), Origin::default());
            let reply = tokio::time::timeout(
                Duration::from_secs(3),
                crate::jsonrpc::handle_request(
                    &tools,
                    json!(42),
                    "tools/call",
                    json!({"name": name, "arguments": {"requests": requests}}),
                ),
            )
            .await
            .expect("both dispatches must proceed while other requests are waiting");
            assert_eq!(reply["result"]["isError"], false, "{reply}");
            let results = &reply["result"]["structuredContent"]["results"];
            for i in [0, 2] {
                assert_eq!(results[i]["index"], i);
                assert_eq!(results[i]["ok"], true, "{reply}");
                assert_eq!(results[i]["turn"]["outcome"], "timedOut");
            }
            assert_eq!(results[1]["index"], 1);
            assert_eq!(results[1]["ok"], false);
            assert!(results[1]["error"]["code"].is_string());
            let writes = world.writes.lock().unwrap();
            let dispatched: Vec<_> = writes
                .iter()
                .filter(|(method, _)| method == methods::QUEUE_COMMAND)
                .collect();
            assert_eq!(dispatched.len(), 2);
            assert_ne!(dispatched[0].1["chatId"], dispatched[1].1["chatId"]);
        }
    }

    #[tokio::test]
    async fn invalid_batch_sizes_have_no_side_effects() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        for name in ["create_chats", "send_messages"] {
            for requests in [vec![], vec![json!({"prompt": "hello"}); MAX_BATCH + 1]] {
                assert!(
                    tools
                        .call(name, json!({"requests": requests}))
                        .await
                        .is_err()
                );
            }
        }
        assert!(world.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn wait_on_background_is_offered_only_where_the_runtime_wakes_its_agent() {
        let world = Arc::new(World::default());
        let offered = |chat: Option<&str>, waits: bool| {
            let tools = tools(
                world.clone(),
                Origin {
                    chat_id: chat.map(str::to_owned),
                    waits_on_background: waits,
                    ..Default::default()
                },
            );
            (tools.has("wait_on_background"), tools.waits_on_background())
        };
        // A chat on a runtime that brings its agent back (Claude Code, Muse).
        assert_eq!(offered(Some("chat-alpha-1"), true), (true, true));
        // A chat on one that doesn't (Codex): a turn that ends is the agent
        // done, so there's nothing to say.
        assert_eq!(offered(Some("chat-alpha-1"), false), (false, false));
        // No chat at all.
        assert_eq!(offered(None, true), (false, false));
        // Not offered, not callable either.
        let codex = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                ..Default::default()
            },
        );
        let call = crate::jsonrpc::handle_request(
            &codex,
            json!(1),
            "tools/call",
            json!({ "name": "wait_on_background", "arguments": { "task_ids": ["b1"] } }),
        )
        .await;
        assert!(call["error"]["message"].as_str().unwrap().contains("unknown tool"), "{call}");
    }

    fn origin(chat: &str) -> Origin {
        Origin {
            chat_id: Some(chat.into()),
            device_id: None,
            ..Default::default()
        }
    }

    fn tools_from(world: &Arc<World>, chat: &str) -> Tools {
        tools(world.clone(), origin(chat))
    }

    #[tokio::test]
    async fn a_chat_creates_chats_with_no_more_access_than_its_own() {
        let supervised = json!({
            "id": "chat-careful", "deviceId": "dev-local", "title": "Careful",
            "archived": false, "spaceId": "space-1",
            "config": { "harness": "claude-code", "sandbox": "workspace-write", "access": "supervised" },
            "createdAt": "2026-09-01T00:00:00Z"
        });
        let world = Arc::new(World {
            extra_chats: vec![supervised],
            ..Default::default()
        });
        let tools = tools_from(&world, "chat-careful");
        tools.legacy("create_chat", json!({})).await.unwrap();
        tools
            .legacy("create_chat", json!({"access": "supervised", "plan": true}))
            .await
            .unwrap();
        // Asking for more is lowered to its own, silently, and said in the
        // reply: no full-access child, no wider sandbox.
        let capped = tools
            .legacy("create_chat", json!({"access": "full-access", "sandbox": "danger-full-access"}))
            .await
            .unwrap();
        assert!(capped["accessCapped"].as_str().unwrap().contains("no more"), "{capped}");
        let writes = world.writes.lock().unwrap();
        let created: Vec<&Value> = writes
            .iter()
            .filter(|(_, w)| w["op"] == "createChat")
            .map(|(_, w)| w)
            .collect();
        assert_eq!(created.len(), 3);
        assert_eq!(created[0]["config"]["access"], "supervised", "inherited");
        assert_eq!(created[1]["config"]["plan"], true);
        assert_eq!(created[2]["config"]["access"], "supervised", "capped");
        assert_eq!(created[2]["config"]["sandbox"], "workspace-write", "capped");
        drop(writes);
        // A top-level chat with no mode of its own runs with full access,
        // which its chats inherit (the field is omitted on the wire).
        tools_from(&world, "chat-alpha-1")
            .legacy("create_chat", json!({}))
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        let last = writes
            .iter()
            .rev()
            .find(|(_, w)| w["op"] == "createChat")
            .unwrap();
        assert!(last.1["config"].get("access").is_none());
    }

    #[tokio::test]
    async fn delegation_is_one_level_deep() {
        let world = Arc::new(World {
            beta_parent: Some("chat-alpha-1".into()),
            deep: true,
            ..Default::default()
        });
        // A top-level chat delegates.
        let created = tools(world.clone(), origin("chat-alpha-1"))
            .legacy("create_chat", json!({}))
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-alpha-1");
        // A chat that has a parent creates nothing: no side chat, no
        // standalone chat, no child of another chat.
        for (from, args) in [
            ("chat-beta-2", json!({})),
            ("chat-beta-2", json!({"kind":"chat"})),
            ("chat-beta-2", json!({"parent":"Alpha"})),
            ("chat-delta-4", json!({})),
        ] {
            let refused = tools(world.clone(), origin(from))
                .legacy("create_chat", args)
                .await
                .unwrap_err();
            assert!(
                refused.contains("can't create chats of its own"),
                "{refused}"
            );
        }

        // Naming a delegate as the parent is refused too, and the batch form
        // reports it per item.
        let root = tools(world.clone(), Origin::default());
        assert!(
            root.legacy("create_chat", json!({"parent":"Beta"}))
                .await
                .unwrap_err()
                .contains("itself a delegated chat")
        );
        let batch = root
            .call(
                "create_chats",
                json!({"requests":[{"parent":"Beta"}, {"parent":"Alpha"}]}),
            )
            .await
            .unwrap();
        let results = batch["results"].as_array().unwrap();
        assert_eq!(results[0]["ok"], false);
        assert_eq!(results[1]["ok"], true);
        assert!(
            world
                .writes
                .lock()
                .unwrap()
                .iter()
                .all(|(_, w)| w["parentChatId"] != "chat-beta-2")
        );
    }

    #[tokio::test]
    async fn a_cloud_child_runs_on_its_own_machine_with_clouds_agents() {
        let world = Arc::new(World::default());
        let alpha = tools(world.clone(), origin("chat-alpha-1"));
        let created = alpha
            .legacy("create_chat", json!({"cloud": true}))
            .await
            .unwrap();
        // The engine names the new machine and its checkout.
        assert_eq!(created["deviceId"], "cloud-child");
        assert_eq!(
            created["harness"], "claude-code",
            "Cloud's first agent by default"
        );
        let sent = world
            .writes
            .lock()
            .unwrap()
            .iter()
            .find(|(method, w)| method == methods::MUTATE && w["op"] == "createChat")
            .map(|(_, w)| w.clone())
            .unwrap();
        assert_eq!(sent["cloud"], true);
        assert_eq!(sent["parentChatId"], "chat-alpha-1");
        // Only Cloud's agents.
        let refused = alpha
            .legacy("create_chat", json!({"cloud": true, "harness": "cursor"}))
            .await
            .unwrap_err();
        assert!(refused.contains("Codex and Claude Code"), "{refused}");
    }

    #[tokio::test]
    async fn connected_means_installed_enabled_and_not_known_signed_out() {
        let accounts = |active: bool| {
            json!({ "accounts": [{
                "id": "a1", "harness": "claude-code", "active": active,
                "email": null, "planLabel": null
            }], "warnings": [] })
        };
        // No accounts endpoint: login state unknown, nothing held against it.
        let unknown = tools(Arc::new(World::default()), Origin::default());
        let listed = unknown.legacy("list_harnesses", json!({})).await.unwrap();
        let claude = &listed["harnesses"][0];
        assert_eq!(claude["signedIn"], Value::Null);
        assert_eq!(claude["connected"], true);
        // Codex is not installed here: never connected.
        assert_eq!(listed["harnesses"][1]["connected"], false);

        let signed_out = Arc::new(World {
            accounts: Some(accounts(false)),
            ..Default::default()
        });
        let tools_out = tools(signed_out.clone(), Origin::default());
        let listed = tools_out.legacy("list_harnesses", json!({})).await.unwrap();
        assert_eq!(listed["harnesses"][0]["signedIn"], false);
        assert_eq!(listed["harnesses"][0]["connected"], false);
        assert_eq!(listed["harnesses"][0]["available"], true);
        let created = tools_out
            .legacy("create_chat", json!({"harness": "claude-code"}))
            .await
            .unwrap();
        assert!(
            created["warning"]
                .as_str()
                .unwrap()
                .contains("not look signed in"),
            "{created}"
        );

        let signed_in = Arc::new(World {
            accounts: Some(accounts(true)),
            ..Default::default()
        });
        let tools_in = tools(signed_in, Origin::default());
        let listed = tools_in.legacy("list_harnesses", json!({})).await.unwrap();
        assert_eq!(listed["harnesses"][0]["signedIn"], true);
        assert_eq!(listed["harnesses"][0]["connected"], true);
        let created = tools_in.legacy("create_chat", json!({})).await.unwrap();
        assert!(created.get("warning").is_none());
    }

    #[tokio::test]
    async fn remote_login_state_and_nested_kinds_use_the_selected_host() {
        let world = Arc::new(World {
            beta_parent: Some("chat-alpha-1".into()),
            deep: true,
            accounts: Some(json!({"accounts": [], "warnings": []})),
            remote_accounts: Some(json!({"accounts": [{
                "id": "remote-account", "harness": "codex", "active": true,
                "email": null, "planLabel": null
            }], "warnings": []})),
            ..Default::default()
        });
        let top = tools(world.clone(), origin("chat-alpha-1"));
        let listed = top
            .legacy("list_harnesses", json!({"device":"Worker"}))
            .await
            .unwrap();
        assert_eq!(listed["harnesses"][0]["signedIn"], true);
        assert_eq!(listed["harnesses"][0]["connected"], true);
        assert_eq!(listed["harnesses"][1]["connected"], false);
        let created = top
            .legacy("create_chat", json!({"kind":"side", "device":"Worker"}))
            .await
            .unwrap();
        assert_eq!(created["kind"], "side");
        assert_eq!(created["parentChatId"], "chat-alpha-1");
        assert_eq!(created["deviceId"], "dev-remote");
        assert_eq!(created["harness"], "codex");
        assert!(created.get("warning").is_none(), "{created}");
        // Delegates create nothing, whatever the kind or host; the batch
        // form reports it per item.
        for from in ["chat-beta-2", "chat-delta-4"] {
            let batch = tools(world.clone(), origin(from))
                .call(
                    "create_chats",
                    json!({"requests":[
                        {"kind":"side", "device":"Worker"},
                        {"kind":"chat", "device":"Worker"},
                        {"parent":"Alpha", "device":"Worker"}
                    ]}),
                )
                .await
                .unwrap();
            for result in batch["results"].as_array().unwrap() {
                assert_eq!(result["ok"], false);
                assert!(
                    result["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("can't create chats of its own"),
                    "{result}"
                );
            }
        }
        let reads = world.reads.lock().unwrap();
        assert!(
            reads
                .iter()
                .any(|(method, params)| method == methods::LIST_AGENT_ACCOUNTS
                    && params["targetDeviceId"] == "dev-remote")
        );
        assert!(
            reads
                .iter()
                .all(|(_, params)| params["targetDeviceId"] == "dev-remote")
        );
    }

    #[tokio::test]
    async fn kinds_default_children_to_subagents_and_reject_invalid_arguments_before_writing() {
        let world = Arc::new(World::default());
        let root = tools(world.clone(), Origin::default());
        for args in [
            json!({"kind":"side"}),
            json!({"kind":"subagent"}),
            json!({"kind":"unknown"}),
            json!({"kind":"chat", "parent":"Alpha", "title":"must not write"}),
        ] {
            assert!(root.legacy("create_chat", args).await.is_err());
        }
        assert!(world.writes.lock().unwrap().is_empty());
        let from_chat = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                device_id: None,
                ..Default::default()
            },
        );
        for (args, kind, parent) in [
            (json!({"kind":"chat"}), "chat", Value::Null),
            (json!({"kind":"chat", "parent":"  "}), "chat", Value::Null),
            (json!({"kind":"side"}), "side", json!("chat-alpha-1")),
            (json!({"kind":"subagent"}), "subagent", json!("chat-alpha-1")),
            // A child of your own chat, kind omitted: a subagent.
            (json!({}), "subagent", json!("chat-alpha-1")),
            (json!({"parent":"Beta"}), "subagent", json!("chat-beta-2")),
            (json!({"kind":"side", "parent":"Beta"}), "side", json!("chat-beta-2")),
        ] {
            let created = from_chat.legacy("create_chat", args).await.unwrap();
            assert_eq!(created["kind"], kind);
            assert_eq!(created["parentChatId"], parent);
            let writes = world.writes.lock().unwrap();
            let row = &writes.last().unwrap().1;
            if kind == "chat" {
                assert!(row.get("parentChatId").is_none());
                assert!(row.get("childKind").is_none());
            } else {
                assert_eq!(row["parentChatId"], parent);
                // The row says what the child is to its parent.
                assert_eq!(row["childKind"], kind);
            }
        }
        assert_eq!(
            root.legacy("create_chat", json!({})).await.unwrap()["kind"],
            "chat"
        );
        assert_eq!(
            root.legacy("create_chat", json!({"kind":"side", "parent":"Alpha"}))
                .await
                .unwrap()["kind"],
            "side"
        );
        // An explicit parent from outside any chat: a subagent of it.
        assert_eq!(
            root.legacy("create_chat", json!({"parent":"Alpha"})).await.unwrap()["kind"],
            "subagent"
        );
    }

    #[tokio::test]
    async fn project_and_device_resolution_scopes_names_and_exact_paths() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        for args in [
            json!({"project":"comet"}),
            json!({"project":"/repo/comet"}),
            json!({"project":"space-1", "device":"Worker"}),
            json!({"project":"missing", "device":"Worker"}),
            json!({"device":"missing"}),
        ] {
            let error = tools.legacy("create_chat", args).await.unwrap_err();
            assert!(world.writes.lock().unwrap().is_empty(), "{error}");
        }
        let error = tools
            .legacy("create_chat", json!({"project":"/repo/comet"}))
            .await
            .unwrap_err();
        for candidate in ["space-1", "space-remote", "dev-local", "dev-remote"] {
            assert!(error.contains(candidate), "{error}");
        }
        for project in ["comet", "/repo/comet", "space-remote"] {
            let created = tools
                .legacy("create_chat", json!({"project":project,"device":"Worker"}))
                .await
                .unwrap();
            assert_eq!(created["deviceId"], "dev-remote");
            assert_eq!(created["project"]["id"], "space-remote");
            assert_eq!(created["harness"], "codex");
        }
        let created = tools
            .legacy("create_chat", json!({"project":"unique"}))
            .await
            .unwrap();
        assert_eq!(created["deviceId"], "dev-remote");
        let created = tools
            .legacy("create_chat", json!({"device":"dev-remote"}))
            .await
            .unwrap();
        assert!(created["project"].is_null());
        assert_eq!(created["deviceId"], "dev-remote");
        let created = tools.legacy("create_chat", json!({})).await.unwrap();
        assert_eq!(created["deviceId"], "dev-local");
    }

    #[tokio::test]
    async fn discovery_and_creation_use_the_selected_hosts_catalogs() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        assert_eq!(
            tools.legacy("list_projects", json!({})).await.unwrap()["projects"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let projects = tools
            .legacy("list_projects", json!({"device":"Worker"}))
            .await
            .unwrap();
        assert_eq!(projects["projects"].as_array().unwrap().len(), 2);
        assert!(
            projects["projects"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["deviceId"] == "dev-remote")
        );
        assert_eq!(
            tools.legacy("list_harnesses", json!({})).await.unwrap()["harnesses"][0]["id"],
            "claude-code"
        );
        assert_eq!(
            tools
                .legacy("list_harnesses", json!({"device":"Worker"}))
                .await
                .unwrap()["harnesses"][0]["id"],
            "codex"
        );
        assert_eq!(
            tools
                .legacy("list_models", json!({"harness":"codex", "device":"Worker"}))
                .await
                .unwrap()["models"][0]["id"],
            "remote-model"
        );
        assert_eq!(
            tools
                .legacy("list_models", json!({"harness":"claude-code"}))
                .await
                .unwrap()["models"][0]["id"],
            "opus"
        );
        for args in [
            json!({"device":"Worker", "harness":"claude-code"}),
            json!({"device":"Worker", "harness":"cursor"}),
            json!({"device":"Laptop", "harness":"codex"}),
            json!({"device":"Laptop", "harness":"mock"}),
            json!({"device":"Worker", "harness":"codex", "model":"opus"}),
        ] {
            assert!(tools.legacy("create_chat", args).await.is_err());
            assert!(world.writes.lock().unwrap().is_empty());
        }
        let created = tools
            .legacy(
                "create_chat",
                json!({"device":"Worker", "harness":"codex", "model":"remote-model"}),
            )
            .await
            .unwrap();
        assert_eq!(created["harness"], "codex");
        assert_eq!(created["model"], "remote-model");
        for method in [methods::LIST_HARNESSES, methods::LIST_MODELS] {
            let broken = Arc::new(World {
                catalog_error: Some(method),
                ..Default::default()
            });
            let tools = self::tools(broken.clone(), Origin::default());
            let error = tools
                .legacy(
                    "create_chat",
                    json!({"device":"Worker", "model":"remote-model"}),
                )
                .await
                .unwrap_err();
            assert!(
                error.contains("dev-remote") && error.contains("catalog unavailable"),
                "{error}"
            );
            assert!(broken.writes.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn remote_send_selects_the_remote_default_harness() {
        let world = Arc::new(World {
            beta_remote: true,
            ..Default::default()
        });
        let tools = tools(world.clone(), Origin::default());
        tools
            .legacy("send_message", json!({"chat":"Beta", "text":"go"}))
            .await
            .unwrap();
        assert_eq!(
            world.reads.lock().unwrap().last().unwrap().1["targetDeviceId"],
            "dev-remote"
        );
        assert_eq!(
            world.writes.lock().unwrap().last().unwrap().1["command"]["request"]["harness"],
            "codex"
        );
    }

    #[tokio::test]
    async fn mixed_batch_preserves_order_and_errors_per_request() {
        let world = Arc::new(World::default());
        let tools = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-alpha-1".into()),
                device_id: None,
                ..Default::default()
            },
        );
        let result = tools
            .call(
                "create_chats",
                json!({"requests":[
                    {"kind":"chat", "device":"Worker", "project":"comet"},
                    {"kind":"chat", "parent":"Alpha"},
                    {"kind":"side", "project":"space-1"},
                    {"kind":"bogus"}
                ]}),
            )
            .await
            .unwrap();
        let results = &result["results"];
        assert_eq!(results[0]["kind"], "chat");
        assert_eq!(results[0]["deviceId"], "dev-remote");
        assert_eq!(results[1]["ok"], false);
        assert_eq!(results[2]["kind"], "side");
        assert_eq!(results[2]["parentChatId"], "chat-alpha-1");
        assert_eq!(results[3]["ok"], false);
        for i in 0..4 {
            assert_eq!(results[i]["index"], i);
        }
        assert_eq!(world.writes.lock().unwrap().len(), 2);
    }

    /// Sessions and transcripts are independently delivered. The command reply
    /// is deliberately slow, so the assistant starts before QueueCommand returns.
    struct DelayedTurn {
        world: World,
        sent: Mutex<Option<(i64, Instant, String)>>,
        reply_delay: Duration,
        never_reply: bool,
        /// Host clock offset: a remote engine stamps createdAt on its own clock.
        host_skew_millis: i64,
    }

    #[async_trait]
    impl RpcService for DelayedTurn {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            match method {
                methods::WATCH_CHATS => {
                    let reply = self.world.handle(method, params).await?;
                    let RpcReply::Stream(mut rows) = reply else {
                        unreachable!()
                    };
                    let mut rows = rows.next().await.unwrap();
                    if let Some((_, _, chat)) = self.sent.lock().unwrap().as_ref()
                        && chat != "chat-beta-2"
                    {
                        rows.as_array_mut().unwrap().push(json!({
                            "id": chat, "deviceId":"dev-remote", "archived":false,
                            "createdAt":chrono::Utc::now(), "config":{"harness":"codex", "sandbox":"workspace-write"}
                        }));
                    }
                    Ok(stream(rows))
                }
                methods::QUEUE_COMMAND => {
                    let reply = self.world.handle(method, params.clone()).await?;
                    *self.sent.lock().unwrap() = Some((
                        now_millis(),
                        Instant::now(),
                        params["chatId"].as_str().unwrap().into(),
                    ));
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    Ok(reply)
                }
                methods::WATCH_SESSIONS => {
                    let sent = self.sent.lock().unwrap().clone();
                    let Some((_, _, chat)) = sent else {
                        return Ok(stream(json!([])));
                    };
                    Ok(RpcReply::Stream(
                        futures::stream::once(async { json!([]) })
                            .chain(futures::stream::once(async move {
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                json!([{"chatId":chat, "deviceId":"dev-remote", "status":"idle",
                                "updatedAt":chrono::Utc::now(), "lastCompletedTurn":"new-turn"}])
                            }))
                            .boxed(),
                    ))
                }
                methods::WATCH_DOC_MESSAGES => {
                    let sent = self.sent.lock().unwrap().clone();
                    // Only the existing chat has history. Its previous reply
                    // shares the send's timestamp, so only its id tells them apart.
                    let mut messages = json!([]);
                    if params["chatId"] == "chat-beta-2" {
                        let created_at = sent.as_ref().map_or_else(now_millis, |(t, _, _)| *t);
                        messages.as_array_mut().unwrap().push(json!({
                            "id":"previous-reply", "role":"assistant", "createdAt":created_at,
                            "deviceId":"dev-remote", "status":"complete", "parts":[{"kind":"text", "id":"t", "text":"old reply"}]
                        }));
                    }
                    if let Some((timestamp, started, _)) = sent
                        && started.elapsed() >= self.reply_delay
                        && !self.never_reply
                    {
                        messages.as_array_mut().unwrap().push(json!({
                            "id":"new-reply", "role":"assistant", "createdAt":timestamp + self.host_skew_millis,
                            "deviceId":"dev-remote", "status":"complete", "parts":[{"kind":"text", "id":"t", "text":"new reply"}]
                        }));
                    }
                    Ok(stream(json!({"reset":messages})))
                }
                _ => self.world.handle(method, params).await,
            }
        }
    }

    #[tokio::test]
    async fn wait_allows_a_missing_session_and_a_late_remote_transcript() {
        for create in [true, false] {
            let service = Arc::new(DelayedTurn {
                world: World {
                    beta_remote: true,
                    ..Default::default()
                },
                sent: Mutex::new(None),
                reply_delay: Duration::from_millis(450),
                never_reply: false,
                host_skew_millis: 0,
            });
            let tools = Tools::new(Arc::new(Zeron::with_client(
                memory_client(service),
                Origin::default(),
            )));
            let (name, args) = if create {
                (
                    "create_chat",
                    json!({"kind":"chat", "device":"Worker", "prompt":"go", "wait":true, "timeout_secs":2}),
                )
            } else {
                (
                    "send_message",
                    json!({"chat":"Beta", "text":"go", "wait":true, "timeout_secs":2}),
                )
            };
            let result = tools.legacy(name, args).await.unwrap();
            assert_eq!(result["turn"]["outcome"], "completed", "{result}");
            assert_eq!(
                result["turn"]["replies"].as_array().unwrap().len(),
                1,
                "{result}"
            );
            assert_eq!(result["turn"]["replies"][0]["text"], "new reply");
        }
    }

    #[tokio::test]
    async fn standalone_wait_after_nonblocking_send_keeps_the_send_baseline() {
        for create in [true, false] {
            let service = Arc::new(DelayedTurn {
                world: World {
                    beta_remote: true,
                    ..Default::default()
                },
                sent: Mutex::new(None),
                reply_delay: Duration::from_millis(450),
                never_reply: false,
                host_skew_millis: 0,
            });
            let tools = Tools::new(Arc::new(Zeron::with_client(
                memory_client(service),
                Origin::default(),
            )));
            let sent = if create {
                tools
                    .legacy(
                        "create_chat",
                        json!({"kind":"chat", "device":"Worker", "prompt":"go", "wait":false}),
                    )
                    .await
                    .unwrap()
            } else {
                tools
                    .legacy(
                        "send_message",
                        json!({"chat":"Beta", "text":"go", "wait":false}),
                    )
                    .await
                    .unwrap()
            };
            let result = tools
                .call(
                    "wait_for_turn",
                    json!({"chat":sent["chatId"], "timeout_secs":2}),
                )
                .await
                .unwrap();
            assert_eq!(result["turn"]["outcome"], "completed", "{result}");
            assert_eq!(result["turn"]["replies"].as_array().unwrap().len(), 1);
            assert_eq!(result["turn"]["replies"][0]["text"], "new reply");
        }
    }

    /// A remote host stamps replies on its own clock. One running behind the
    /// caller must not hide the reply to this send.
    #[tokio::test]
    async fn wait_returns_the_reply_from_a_host_whose_clock_is_behind() {
        for create in [true, false] {
            let service = Arc::new(DelayedTurn {
                world: World {
                    beta_remote: true,
                    ..Default::default()
                },
                sent: Mutex::new(None),
                reply_delay: Duration::from_millis(100),
                never_reply: false,
                host_skew_millis: -30_000,
            });
            let tools = Tools::new(Arc::new(Zeron::with_client(
                memory_client(service),
                Origin::default(),
            )));
            let (name, args) = if create {
                (
                    "create_chat",
                    json!({"kind":"chat", "device":"Worker", "prompt":"go", "wait":true, "timeout_secs":2}),
                )
            } else {
                (
                    "send_message",
                    json!({"chat":"Beta", "text":"go", "wait":true, "timeout_secs":2}),
                )
            };
            let result = tools.legacy(name, args).await.unwrap();
            assert_eq!(result["turn"]["outcome"], "completed", "{result}");
            let replies = result["turn"]["replies"].as_array().unwrap();
            assert!(replies.iter().any(|r| r["text"] == "new reply"), "{result}");
        }
    }

    #[tokio::test]
    async fn completed_session_without_new_transcript_times_out_without_old_reply() {
        let service = Arc::new(DelayedTurn {
            world: World {
                beta_remote: true,
                ..Default::default()
            },
            sent: Mutex::new(None),
            reply_delay: Duration::ZERO,
            never_reply: true,
            host_skew_millis: 0,
        });
        let tools = Tools::new(Arc::new(Zeron::with_client(
            memory_client(service),
            Origin::default(),
        )));
        let result = tools
            .legacy(
                "send_message",
                json!({"chat":"Beta", "text":"go", "wait":true, "timeout_secs":1}),
            )
            .await
            .unwrap();
        assert_eq!(result["turn"]["outcome"], "timedOut");
        assert_eq!(result["turn"]["replies"], json!([]));
    }

    #[tokio::test]
    async fn remote_replies_do_not_require_synchronized_clocks() {
        for host_skew_millis in [-1_000, 1_000] {
            for wait in [true, false] {
                let service = Arc::new(DelayedTurn {
                    world: World {
                        beta_remote: true,
                        ..Default::default()
                    },
                    sent: Mutex::new(None),
                    reply_delay: Duration::from_millis(450),
                    never_reply: false,
                    host_skew_millis,
                });
                let tools = Tools::new(Arc::new(Zeron::with_client(
                    memory_client(service),
                    Origin::default(),
                )));
                let sent = tools
                    .legacy(
                        "send_message",
                        json!({
                            "chat":"Beta", "text":"go", "wait":wait, "timeout_secs":2
                        }),
                    )
                    .await
                    .unwrap();
                let result = if wait {
                    sent
                } else {
                    tools
                        .call(
                            "wait_for_turn",
                            json!({
                                "chat":sent["chatId"], "timeout_secs":2
                            }),
                        )
                        .await
                        .unwrap()
                };
                assert_eq!(
                    result["turn"]["outcome"], "completed",
                    "offset={host_skew_millis}, wait={wait}: {result}"
                );
                assert_eq!(
                    result["turn"]["replies"].as_array().unwrap().len(),
                    1,
                    "{result}"
                );
                assert_eq!(result["turn"]["replies"][0]["id"], "new-reply");
            }
        }
    }

    #[tokio::test]
    async fn initialize_ends_with_the_run_plugin_instructions_when_the_host_wrote_them() {
        let world = Arc::new(World::default());
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chat.md");
        std::fs::write(
            &file,
            "## Plugins\n\n### Notes tool\n\nCall notes_tool__count_words.\n",
        )
        .unwrap();
        let said = |origin: Origin| {
            let tools = tools(world.clone(), origin);
            async move {
                let init =
                    crate::jsonrpc::handle_request(&tools, json!(1), "initialize", json!({})).await;
                init["result"]["instructions"].as_str().unwrap().to_owned()
            }
        };
        let with = said(Origin {
            chat_id: Some("chat-alpha-1".into()),
            plugin_instructions: Some(file.clone()),
            ..Default::default()
        })
        .await;
        assert!(
            with.ends_with("### Notes tool\n\nCall notes_tool__count_words."),
            "{with}"
        );
        let without = said(Origin {
            chat_id: Some("chat-alpha-1".into()),
            ..Default::default()
        })
        .await;
        assert!(!without.contains("## Plugins"));
        let gone = said(Origin {
            chat_id: Some("chat-alpha-1".into()),
            plugin_instructions: Some(dir.path().join("missing.md")),
            ..Default::default()
        })
        .await;
        assert_eq!(gone, without);
        let delivered = said(Origin {
            chat_id: Some("chat-alpha-1".into()),
            plugin_instructions: Some(file.clone()),
            guidance_in_prompt: true,
            ..Default::default()
        })
        .await;
        assert_eq!(delivered, zeron_proto::agent_guidance::MCP_INSTRUCTIONS);
    }

    #[tokio::test]
    async fn initialize_and_list_over_jsonrpc() {
        let world = Arc::new(World::default());
        // A server whose agent launched without its own subagent tools
        // tells it to delegate through chats; any other doesn't.
        let rule = "delegate with Zeron's `create_chats` instead.";
        for side_chats_only in [true, false] {
            let origin = Origin {
                side_chats_only,
                ..Default::default()
            };
            let init = crate::jsonrpc::handle_request(
                &tools(world.clone(), origin),
                json!(1),
                "initialize",
                json!({}),
            )
            .await;
            let instructions = init["result"]["instructions"].as_str().unwrap();
            assert_eq!(instructions.contains(rule), side_chats_only);
        }
        let tools = tools(world, Origin::default());
        let init = crate::jsonrpc::handle_request(
            &tools,
            json!(1),
            "initialize",
            json!({ "protocolVersion": "2025-03-26" }),
        )
        .await;
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "zeron");
        let list =
            crate::jsonrpc::handle_request(&tools, json!(2), "tools/list", Value::Null).await;
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 10);
        let bad = crate::jsonrpc::handle_request(
            &tools,
            json!(3),
            "tools/call",
            json!({ "name": "nope" }),
        )
        .await;
        assert_eq!(bad["error"]["code"], -32602);
        let whoami = crate::jsonrpc::handle_request(
            &tools,
            json!(4),
            "tools/call",
            json!({ "name": "catalog", "arguments": { "kind": "self" } }),
        )
        .await;
        assert_eq!(whoami["result"]["isError"], false);
        assert_eq!(
            whoami["result"]["structuredContent"]["items"][0]["localDeviceId"],
            "dev-local"
        );
        let text = whoami["result"]["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains('\n'), "compact JSON text: {text}");
        let invalid = crate::jsonrpc::handle_request(
            &tools,
            json!(5),
            "tools/call",
            json!({ "name": "read_chat", "arguments": { "chat": "alpha", "lmit": 3 } }),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
        let error = &invalid["result"]["structuredContent"];
        assert_eq!(error["code"], "invalid_arguments");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("unknown field `lmit`") && message.contains("Valid call: read_chat {"), "{message}");
    }
}
