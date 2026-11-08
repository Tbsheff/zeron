//! Plugin tools: what Zeron plugins offer agents, and the two tools that
//! make and list plugins.
//!
//! A plugin tool's name is dynamic (`<plugin>__<tool>`, the engine's
//! `mcp_name`), so it is not in the static [`catalog`](super::catalog): the
//! listing asks the engine (`ListPluginTools`) every time, a call goes to
//! `CallPluginTool`, and [`Tools::watch_plugin_tools`] follows
//! `WatchPluginTools` so the server can tell the client its list changed.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use zeron_rpc::{RpcError, methods};

use super::{Hints, ToolDef, Tools, parse};
use crate::error::ToolError;

const LIST_TEMPLATES: &str = "list";
const DESCRIBE_APP: &str = "app";
const TEMPLATE_STATUS: &str = "status";
const CREATE_TEMPLATE: &str = "create";
const USE_TEMPLATE: &str = "use";
const DELETE_TEMPLATE: &str = "delete";
const EXPORT_TEMPLATE: &str = "export";

/// How long a listing waits for the engine's plugin tools before it goes
/// out with the static tools only.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Gap before re-watching after the stream ends (an engine restart) or the
/// dial fails (no engine yet); doubles up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_millis(300);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// A tool definition with an owned name and description: the shape
/// `tools/list` sends, for static and plugin tools alike.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl From<ToolDef> for ListedTool {
    fn from(def: ToolDef) -> Self {
        Self {
            name: def.name.to_owned(),
            title: Some(def.title.to_owned()),
            description: def.description.to_owned(),
            input_schema: crate::schema::compact(&def.input_schema),
            output_schema: def.output_schema,
            annotations: Some(def.hints.to_json(def.title)),
            meta: def
                .always_load
                .then(|| json!({ "anthropic/alwaysLoad": true })),
        }
    }
}

/// `ListPluginTools` / `WatchPluginTools` reply.
#[derive(Debug, Clone, Default, Deserialize)]
struct PluginTools {
    #[serde(default)]
    device: PluginDevice,
    #[serde(default)]
    tools: Vec<PluginTool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PluginDevice {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginTool {
    plugin: String,
    #[serde(default)]
    plugin_name: String,
    mcp_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    input: Value,
}

impl PluginTools {
    fn names(&self) -> BTreeSet<String> {
        self.tools.iter().map(|t| t.mcp_name.clone()).collect()
    }

    fn listed(&self) -> Vec<ListedTool> {
        let device = match self.device.name.trim() {
            "" => "this device",
            name => name,
        };
        self.tools
            .iter()
            .map(|tool| {
                let plugin = match tool.plugin_name.trim() {
                    "" => tool.plugin.as_str(),
                    name => name,
                };
                let origin =
                    format!("(From the {plugin} plugin. Works while Zeron is open on {device}.)");
                let description = match tool.description.trim() {
                    "" => origin,
                    text => format!("{text} {origin}"),
                };
                ListedTool {
                    name: tool.mcp_name.clone(),
                    title: None,
                    description,
                    input_schema: crate::schema::compact(&input_schema(&tool.input)),
                    output_schema: None,
                    annotations: None,
                    meta: None,
                }
            })
            .collect()
    }
}

/// MCP wants an object schema; a manifest may leave `input` out or omit its
/// `type`.
fn input_schema(input: &Value) -> Value {
    match input {
        Value::Object(schema) => {
            let mut schema = schema.clone();
            schema
                .entry("type")
                .or_insert_with(|| Value::String("object".into()));
            Value::Object(schema)
        }
        _ => json!({ "type": "object", "properties": {} }),
    }
}

/// Plugin tools are named `<plugin>__<tool>` (the plugin id with `-` → `_`);
/// no static tool has `__` in its name.
pub(crate) fn is_plugin_tool_name(name: &str) -> bool {
    let word = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    };
    name.split_once("__")
        .is_some_and(|(plugin, tool)| word(plugin) && word(tool))
}

/// The engine's error in words an agent can act on.
fn plain(error: RpcError) -> ToolError {
    ToolError::from_rpc(&error, "This Zeron doesn't run plugins yet; update Zeron to use them.")
}

pub(super) const PLUGINS: &str = "plugins";
pub(super) const TEMPLATES: &str = "templates";
pub(super) const NAMES: [&str; 2] = [PLUGINS, TEMPLATES];
const PLUGIN_ACTIONS: [&str; 6] = ["list", "status", "elements", "updates", "create", "install"];
const TEMPLATE_ACTIONS: [&str; 8] = [
    LIST_TEMPLATES,
    TEMPLATE_STATUS,
    DESCRIBE_APP,
    CREATE_TEMPLATE,
    USE_TEMPLATE,
    DELETE_TEMPLATE,
    EXPORT_TEMPLATE,
    "import",
];

const PLUGINS_DESCRIPTION: &str = "\
Zeron plugins on this device. action list. status {id, snapshot?}: where its UI landed (places, \
matched elements, Settings row, errors); snapshot adds a PNG to read. elements: selectors for \
zeron.place and what each provides. updates {id?}: newer versions (only the person installs them). \
create {id, name, permissions?}: a signed folder that reloads as you save; read the reply's `guide` \
first. install {path | url, ref?, folder?}: the person must allow it in Zeron, not you.";

const TEMPLATES_DESCRIPTION: &str = "\
Zeron templates: whole copies of Zeron's Luau interface to rewrite. action list. status {id}: errors \
and out-of-date files. app: screens and the #ids and .classes plugins place by. create {id, name, \
from?}: read the reply's `guide` first. use {id}: switch the window live (zeron: Zeron's own), a \
preview until the person keeps it. delete {id}. export {id}: a signed file. import {path | url}: \
someone else's waits for the person to allow it.";

pub(super) fn catalog() -> Vec<ToolDef> {
    let text = || json!({ "type": "string" });
    vec![
        ToolDef {
            name: PLUGINS,
            title: "Zeron plugins",
            description: PLUGINS_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": PLUGIN_ACTIONS },
                    "id": text(),
                    "name": text(),
                    "description": text(),
                    "permissions": { "type": "array", "items": { "type": "string" }, "description": "e.g. ui.add, ui.change, sessions.read, network, commands; the guide lists all." },
                    "snapshot": { "type": "boolean" },
                    "path": text(),
                    "url": text(),
                    "ref": text(),
                    "folder": text()
                },
                "required": ["action"]
            }),
            output_schema: None,
            hints: Hints::WRITE.destructive().open_world(),
            always_load: false,
            example: r#"{"action":"status","id":"focus","snapshot":true}"#,
        },
        ToolDef {
            name: TEMPLATES,
            title: "Zeron templates",
            description: TEMPLATES_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": TEMPLATE_ACTIONS },
                    "id": text(),
                    "name": text(),
                    "from": text(),
                    "path": text(),
                    "url": text()
                },
                "required": ["action"]
            }),
            output_schema: None,
            hints: Hints::WRITE.destructive().open_world(),
            always_load: false,
            example: r#"{"action":"create","id":"midnight","name":"Midnight"}"#,
        },
    ]
}

#[derive(Deserialize)]
struct ActionArgs {
    action: String,
}

fn action_of(args: &Value, allowed: &[&str]) -> Result<String, ToolError> {
    let action = parse::<ActionArgs>(args.clone())?.action;
    if allowed.contains(&action.as_str()) {
        Ok(action)
    } else {
        Err(ToolError::invalid(format!(
            "unknown action {action:?}; one of: {}",
            allowed.join(", ")
        )))
    }
}

fn without_action(mut args: Value) -> Value {
    if let Some(fields) = args.as_object_mut() {
        fields.remove("action");
    }
    args
}

#[derive(Deserialize)]
struct PackageArgs {
    id: Option<String>,
    path: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct TemplateArgs {
    id: Option<String>,
    name: Option<String>,
    from: Option<String>,
}

#[derive(Deserialize)]
struct CreatePluginArgs {
    id: String,
    name: String,
    description: Option<String>,
    permissions: Option<Vec<String>>,
}

/// What a plugin listing tracks between `tools/list` and the watch.
#[derive(Default)]
pub(super) struct PluginState {
    /// The plugin tool names the client was last shown (by a listing or the
    /// latest watch item). `None` until either happens.
    advertised: std::sync::Mutex<Option<BTreeSet<String>>>,
}

impl PluginState {
    /// Record `names` as what the client knows now; `true` when that
    /// differs from what it knew before.
    fn advertise(&self, names: BTreeSet<String>) -> bool {
        let mut advertised = self.advertised.lock().unwrap_or_else(|e| e.into_inner());
        advertised
            .replace(names.clone())
            .is_some_and(|before| before != names)
    }
}

impl Tools {
    /// The full `tools/list`: the static catalog, then the engine's plugin
    /// tools. Without an engine (or one without plugins) it is the static
    /// catalog alone.
    pub async fn list_all(&self) -> Vec<ListedTool> {
        let mut tools: Vec<ListedTool> = self.list().into_iter().map(ListedTool::from).collect();
        let reply = tokio::time::timeout(
            LIST_TIMEOUT,
            self.zeron.call_rpc(methods::LIST_PLUGIN_TOOLS, json!({})),
        )
        .await;
        match reply {
            Ok(Ok(value)) => match serde_json::from_value::<PluginTools>(value) {
                Ok(plugins) => {
                    self.plugins.advertise(plugins.names());
                    let taken: BTreeSet<String> = tools.iter().map(|t| t.name.clone()).collect();
                    tools.extend(
                        plugins
                            .listed()
                            .into_iter()
                            .filter(|tool| !taken.contains(&tool.name)),
                    );
                }
                Err(error) => tracing::warn!(%error, "mcp: unexpected ListPluginTools reply"),
            },
            Ok(Err(error)) => tracing::debug!(%error, "mcp: no plugin tools"),
            Err(_) => tracing::warn!("mcp: ListPluginTools timed out; listing static tools only"),
        }
        tools
    }

    /// Follow `WatchPluginTools` for as long as the server runs, sending on
    /// `changed` whenever the set of plugin tool names differs from what the
    /// client last saw. Returns when the engine has no such method, or when
    /// `changed` closes.
    pub async fn watch_plugin_tools(&self, changed: mpsc::Sender<()>) {
        let mut retry = FIRST_RETRY;
        loop {
            match self
                .zeron
                .subscribe_checked(methods::WATCH_PLUGIN_TOOLS, json!({}))
                .await
            {
                Ok(mut stream) => {
                    retry = FIRST_RETRY;
                    while let Some(item) = stream.recv().await {
                        let names = match serde_json::from_value::<PluginTools>(item) {
                            Ok(plugins) => plugins.names(),
                            Err(error) => {
                                tracing::warn!(%error, "mcp: unexpected WatchPluginTools item");
                                continue;
                            }
                        };
                        if self.plugins.advertise(names) && changed.send(()).await.is_err() {
                            return;
                        }
                    }
                }
                Err(RpcError::UnknownMethod(_)) => return,
                Err(error) => tracing::debug!(%error, "mcp: WatchPluginTools unavailable"),
            }
            if changed.is_closed() {
                return;
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(MAX_RETRY);
        }
    }

    /// A plugin tool call, attributed to the chat this server speaks for.
    pub(super) async fn call_plugin_tool(&self, name: &str, args: Value) -> Result<Value, ToolError> {
        let mut params = json!({ "mcp_name": name, "args": args });
        if let Some(chat) = &self.zeron.origin().chat_id {
            params["chatId"] = json!(chat);
        }
        let reply = self
            .zeron
            .call_rpc(methods::CALL_PLUGIN_TOOL, params)
            .await
            .map_err(plain)?;
        if reply.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(ToolError::rejected(
                reply
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The plugin tool failed"),
            ));
        }
        Ok(reply.get("value").cloned().unwrap_or(Value::Null))
    }

    pub(super) async fn create_plugin(&self, args: Value) -> Result<Value, ToolError> {
        let args: CreatePluginArgs = parse(args)?;
        let mut params = json!({ "id": args.id, "name": args.name });
        if let Some(description) = args.description {
            params["description"] = json!(description);
        }
        if let Some(permissions) = args.permissions {
            params["permissions"] = json!(permissions);
        }
        let item = self
            .zeron
            .call_rpc(methods::NEW_PLUGIN, params)
            .await
            .map_err(plain)?;
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let dir = item.get("dir").and_then(Value::as_str).unwrap_or_default();
        let mut reply = json!({
            "id": id,
            "dir": dir,
            "files": scaffold_files(dir).await,
            "status": item.get("status"),
            "hint": format!(
                "Edit {dir}/init.luau; Zeron re-signs and reloads it as you save. Use {dir}/zeron.luau's helpers."
            ),
        });
        // The authoring guide, written on this device: read it first.
        if let Some(guide) = item.get("guide").and_then(Value::as_str) {
            reply["guide"] = json!(guide);
            reply["guide_summary"] = item.get("guide_summary").cloned().unwrap_or(Value::Null);
            reply["hint"] = json!(format!(
                "Read {guide} first (how to write a plugin, with examples). Then edit {dir}/init.luau; Zeron re-signs and reloads it as you save. Check it with plugins {{action: status, snapshot: true}}."
            ));
        }
        Ok(reply)
    }

    pub(super) async fn plugins(&self, tool: &str, args: Value) -> Result<Value, ToolError> {
        if tool == PLUGINS {
            return match action_of(&args, &PLUGIN_ACTIONS)?.as_str() {
                "list" => self.list_plugins().await,
                "status" => self.plugin_status(without_action(args)).await,
                "elements" => self.describe_elements().await,
                "updates" => self.update_plugin(without_action(args)).await,
                "create" => self.create_plugin(without_action(args)).await,
                _ => self.install_plugin(without_action(args)).await,
            };
        }
        let action = action_of(&args, &TEMPLATE_ACTIONS)?;
        match action.as_str() {
            EXPORT_TEMPLATE | "import" => self.template_packages(&action, without_action(args)).await,
            _ => self.templates(&action, without_action(args)).await,
        }
    }

    /// The template tools: the engine's `ListTemplates`, `CreateTemplate`,
    /// `UseTemplate`, `TemplateStatus`, `DeleteTemplate` and `DescribeApp`,
    /// as they answer.
    pub(super) async fn templates(&self, tool: &str, args: Value) -> Result<Value, ToolError> {
        let args: TemplateArgs = parse(args)?;
        let (method, needs_id) = match tool {
            LIST_TEMPLATES => (methods::LIST_TEMPLATES, false),
            DESCRIBE_APP => (methods::DESCRIBE_APP, false),
            CREATE_TEMPLATE => (methods::CREATE_TEMPLATE, true),
            USE_TEMPLATE => (methods::USE_TEMPLATE, true),
            TEMPLATE_STATUS => (methods::TEMPLATE_STATUS, true),
            _ => (methods::DELETE_TEMPLATE, true),
        };
        let mut params = json!({});
        if needs_id {
            let Some(id) = args.id.filter(|id| !id.trim().is_empty()) else {
                return Err(ToolError::invalid("Say which template: `id`, from templates {action: list}."));
            };
            params["id"] = json!(id);
        }
        if method == methods::CREATE_TEMPLATE {
            let Some(name) = args.name.filter(|name| !name.trim().is_empty()) else {
                return Err(ToolError::invalid("Give the template a `name`."));
            };
            params["name"] = json!(name);
            if let Some(from) = args.from.filter(|from| !from.trim().is_empty()) {
                params["from"] = json!(from);
            }
        }
        let mut reply = self.zeron.call_rpc(method, params).await.map_err(plain)?;
        if method == methods::USE_TEMPLATE && reply["preview"] == json!(true) {
            reply["next"] = json!(
                "Unless the person has kept this template, Zeron runs it as a preview with a bar to Keep or Revert it. Ask them to look; you can't keep it for them."
            );
        }
        Ok(reply)
    }

    /// `export_template` and `import_template`: the engine's
    /// `ExportTemplate` and `ImportTemplate`. Allowing someone else's is the
    /// person's, in Zeron's own window: no tool answers for them.
    pub(super) async fn template_packages(&self, tool: &str, args: Value) -> Result<Value, ToolError> {
        let args: PackageArgs = parse(args)?;
        let (method, params) = if tool == EXPORT_TEMPLATE {
            let Some(id) = args.id.filter(|id| !id.trim().is_empty()) else {
                return Err(ToolError::invalid("Say which template: `id`, from templates {action: list}."));
            };
            // Always to Zeron's exports folder (the reply says where): a
            // path of an agent's choosing is the person's, and refused.
            (methods::EXPORT_TEMPLATE, json!({ "id": id }))
        } else {
            let path = args.path.filter(|p| !p.trim().is_empty());
            match (path, args.url.filter(|u| !u.trim().is_empty())) {
                (Some(path), None) => (methods::IMPORT_TEMPLATE, json!({ "path": path })),
                (None, Some(url)) => (methods::IMPORT_TEMPLATE, json!({ "url": url })),
                _ => return Err(ToolError::invalid("Say what to import: `path`, a .zeron-template file, or `url`, a link to one.")),
            }
        };
        let mut reply = self.zeron.call_rpc(method, params).await.map_err(plain)?;
        if reply["needs_consent"] == json!(true) {
            reply["next"] = json!(
                "It's waiting for the person to allow it in Zeron (Templates shows it, with who signed it). Ask them to look; you can't allow it for them."
            );
        }
        Ok(reply)
    }

    pub(super) async fn list_plugins(&self) -> Result<Value, ToolError> {
        let reply = self
            .zeron
            .call_rpc(methods::LIST_PLUGINS, json!({}))
            .await
            .map_err(plain)?;
        let plugins = reply
            .get("plugins")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let mut out = json!({
                    "id": p.get("id"),
                    "name": p.get("name"),
                    "status": p.get("status"),
                    "author": p.get("author"),
                    "dir": p.get("dir"),
                });
                // Someone else's: where it came from, and a newer version there.
                for key in ["source", "update"] {
                    if let Some(value) = p.get(key).filter(|v| !v.is_null()) {
                        out[key] = value.clone();
                    }
                }
                out
            })
            .collect::<Vec<_>>();
        Ok(json!({ "plugins": plugins }))
    }

    /// The engine's `PluginStatus`.
    pub(super) async fn plugin_status(&self, args: Value) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            id: String,
            #[serde(default)]
            snapshot: bool,
        }
        let args: Args = parse(args)?;
        self.zeron
            .call_rpc(methods::PLUGIN_STATUS, json!({ "id": args.id, "snapshot": args.snapshot }))
            .await
            .map_err(plain)
    }

    /// `install_plugin`: the engine's `InstallPlugin`. Allowing someone
    /// else's is the person's, in Zeron's window.
    pub(super) async fn install_plugin(&self, args: Value) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            path: Option<String>,
            url: Option<String>,
            #[serde(rename = "ref")]
            git_ref: Option<String>,
            folder: Option<String>,
        }
        let args: Args = parse(args)?;
        let given = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        let params = match (given(args.path), given(args.url)) {
            (Some(path), None) => json!({ "path": path }),
            (None, Some(url)) => json!({ "url": url, "ref": given(args.git_ref), "folder": given(args.folder) }),
            _ => return Err(ToolError::invalid("Give one of `path` (a .zeron-plugin file) or `url` (a link or repository).")),
        };
        let reply = self.zeron.call_rpc(methods::INSTALL_PLUGIN, params).await.map_err(plain)?;
        Ok(installed(reply))
    }

    /// `update_plugin`: `CheckPluginUpdates` only. Installing an update is
    /// the person's (`UpdatePlugin` is closed to agents).
    pub(super) async fn update_plugin(&self, args: Value) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            id: Option<String>,
            #[serde(default)]
            check: bool,
        }
        let args: Args = parse(args)?;
        let _ = args.check;
        let id = args.id.filter(|id| !id.trim().is_empty());
        // Checking only: installing an update is the person's, in Zeron's
        // window (an agent's token can't update a plugin).
        let mut reply = self
            .zeron
            .call_rpc(methods::CHECK_PLUGIN_UPDATES, json!({ "id": id }))
            .await
            .map_err(plain)?;
        if reply["updates"].as_array().is_some_and(|u| !u.is_empty()) {
            reply["next"] = json!(
                "To install an update, the person updates the plugin in Zeron (Settings → Plugins); you can't update it for them. Tell them what you found."
            );
        }
        Ok(reply)
    }

    /// The engine's `DescribeElements`, as the window wrote it.
    pub(super) async fn describe_elements(&self) -> Result<Value, ToolError> {
        self.zeron
            .call_rpc(methods::DESCRIBE_ELEMENTS, json!({}))
            .await
            .map_err(plain)
    }
}

/// An install's reply, said for an agent: the plugin's id, name, version
/// and status, what changed, and what's next.
fn installed(reply: Value) -> Value {
    let plugin = &reply["plugin"];
    let mut out = json!({
        "id": plugin["id"],
        "name": plugin["name"],
        "version": plugin["version"],
        "status": plugin["status"],
        "author": plugin["author"],
        "update": reply["update"],
        "added": reply["added"],
        "removed": reply["removed"],
        "waiting": reply["waiting"],
    });
    if let Some(from) = reply.get("from") {
        out["from"] = from.clone();
    }
    if reply["waiting"] == json!(true) {
        out["next"] = json!(
            "It's waiting for the person to allow it in Zeron (a notice asks them, and Settings → Plugins shows who signed it and what it asks). Ask them to look; you can't allow it for them."
        );
    }
    out
}

/// The files a new plugin's folder holds (top level, no dotfiles), sorted;
/// the scaffold's when the folder can't be read from here.
async fn scaffold_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    if !dir.is_empty()
        && let Ok(mut entries) = tokio::fs::read_dir(dir).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_file = entry.file_type().await.is_ok_and(|t| t.is_file());
            if is_file && !name.starts_with('.') {
                files.push(name);
            }
        }
    }
    if files.is_empty() {
        files = ["init.luau", "zeron-plugin.json", "zeron.luau"]
            .map(str::to_owned)
            .to_vec();
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeron::{Origin, Zeron};
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use zeron_rpc::{RpcReply, RpcService, memory_client};

    fn tools_reply(names: &[&str]) -> Value {
        json!({
            "device": { "id": "dev-local", "name": "Studio" },
            "tools": names.iter().map(|name| json!({
                "plugin": "word-count", "plugin_name": "Word Count", "tool": "count_words",
                "mcp_name": name, "description": "Count words in a text",
                "input": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] },
                "open": true,
            })).collect::<Vec<_>>(),
        })
    }

    /// The engine's plugin half: canned replies, recorded calls.
    #[derive(Default)]
    struct Engine {
        calls: Mutex<Vec<(String, Value)>>,
        /// `WatchPluginTools` items, in order.
        watch: Vec<Value>,
        call_error: Option<&'static str>,
        /// Answer `UnknownMethod` for every plugin method (an older engine).
        no_plugins: bool,
    }

    #[async_trait]
    impl RpcService for Engine {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.to_owned(), params.clone()));
            if self.no_plugins {
                return Err(RpcError::UnknownMethod(method.into()));
            }
            Ok(match method {
                methods::LIST_PLUGIN_TOOLS => {
                    RpcReply::Value(tools_reply(&["word_count__count_words"]))
                }
                methods::WATCH_PLUGIN_TOOLS => RpcReply::Stream(
                    futures::stream::iter(self.watch.clone())
                        .chain(futures::stream::pending())
                        .boxed(),
                ),
                methods::CALL_PLUGIN_TOOL => match self.call_error {
                    Some(message) => return Err(RpcError::Failed(message.into())),
                    None => RpcReply::Value(json!({ "ok": true, "value": { "count": 3 } })),
                },
                methods::NEW_PLUGIN => RpcReply::Value(json!({
                    "id": params["id"], "name": params["name"], "dir": "/nowhere/plugins/focus",
                    "status": "on", "own": true,
                    "guide": "/nowhere/docs/plugins.md", "guide_summary": "How to write a Zeron plugin.",
                })),
                methods::LIST_PLUGINS => RpcReply::Value(json!({
                    "dirs": { "plugins": "/d/plugins", "templates": "/d/templates" },
                    "plugins": [{
                        "id": "focus", "name": "Focus", "version": "1.2.0", "dir": "/d/plugins/focus",
                        "status": "pending", "author": { "kind": "verified", "username": "ada" },
                        "hash": "abc", "permissions": ["ui.add"],
                    }],
                })),
                methods::LIST_TEMPLATES => RpcReply::Value(json!({
                    "dir": "/d/templates", "active": "zeron",
                    "templates": [
                        { "id": "zeron", "name": "Zeron", "builtin": true, "active": true, "status": "ok", "dir": "/d/templates/.zeron" },
                        { "id": "midnight", "name": "Midnight", "builtin": false, "active": false, "status": "errors",
                          "error": "ds/tokens.luau: boom", "dir": "/d/templates/midnight" },
                    ],
                })),
                methods::CREATE_TEMPLATE => RpcReply::Value(json!({
                    "id": params["id"], "dir": "/d/templates/midnight", "files": ["app.luau"], "map": "app.luau is the root",
                })),
                methods::USE_TEMPLATE => match params["id"].as_str() {
                    Some("nope") => {
                        return Err(RpcError::Failed(
                            "There's no template nope. list_templates names them.".into(),
                        ));
                    }
                    _ => RpcReply::Value(json!({ "active": params["id"], "preview": params["id"] != "zeron" })),
                },
                methods::TEMPLATE_STATUS => RpcReply::Value(json!({
                    "id": params["id"], "active": false, "file_errors": { "ds/tokens.luau": "boom" },
                    "out_of_date": [], "files": 40, "inherits": 2,
                })),
                methods::DELETE_TEMPLATE => {
                    RpcReply::Value(json!({ "deleted": params["id"], "active": "zeron" }))
                }
                methods::EXPORT_TEMPLATE => RpcReply::Value(json!({
                    "id": params["id"], "path": params["path"], "files": 12, "key": "k1",
                })),
                methods::IMPORT_TEMPLATE => match params["path"].as_str() {
                    Some("/x/mine.zeron-template") => RpcReply::Value(json!({
                        "installed": "midnight", "name": "Midnight", "author": { "kind": "you" }, "ignored": [],
                    })),
                    Some("/x/bad.zeron-template") => {
                        return Err(RpcError::Failed(
                            "This template was changed after it was signed, so Zeron won't install it.".into(),
                        ));
                    }
                    _ => RpcReply::Value(json!({
                        "pending": "midnight-1", "id": "midnight", "name": "Midnight", "key": "k2",
                        "author": { "kind": "verified", "username": "ada" }, "needs_consent": true,
                    })),
                },
                methods::INSTALL_PLUGIN => RpcReply::Value(json!({
                    "plugin": { "id": "focus", "name": "Focus", "version": "1.0.0", "status": "pending",
                                "author": { "kind": "verified", "username": "ada" } },
                    "update": false, "added": [], "removed": [], "waiting": true,
                })),
                methods::UPDATE_PLUGIN => RpcReply::Value(json!({
                    "plugin": { "id": params["id"], "name": "Focus", "version": "2.0.0", "status": "on",
                                "author": { "kind": "verified", "username": "ada" } },
                    "update": true, "from": "1.0.0", "added": [], "removed": ["links.open"], "waiting": false,
                })),
                methods::CHECK_PLUGIN_UPDATES => RpcReply::Value(json!({
                    "updates": [{ "id": "focus", "name": "Focus", "version": "2.0.0", "added": ["github"], "removed": [] }],
                    "errors": [],
                })),
                methods::PLUGIN_STATUS => RpcReply::Value(json!({
                    "id": params["id"], "status": "on", "open": true,
                    "placements": [{ "at": ".session-row", "how": "after", "count": 3 }],
                })),
                methods::DESCRIBE_ELEMENTS => RpcReply::Value(json!({
                    "elements": [{ "selector": ".session-row", "place": "a sidebar row", "provides": ["session"], "present": true }],
                })),
                methods::DESCRIBE_APP => RpcReply::Value(json!({
                    "screens": [{ "module": "@app/screens/shell/sidebar", "elements": ["#sidebar"] }], "map": "app.luau is the root",
                })),
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn last_call(engine: &Engine) -> (String, Value) {
        engine.calls.lock().unwrap().last().unwrap().clone()
    }

    #[tokio::test]
    async fn template_tools_forward_and_report_errors() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), None);

        let listed = tools.call("templates", json!({ "action": "list" })).await.unwrap();
        assert_eq!(listed["templates"][1]["error"], "ds/tokens.luau: boom");
        assert_eq!(
            last_call(&engine),
            (methods::LIST_TEMPLATES.to_owned(), json!({}))
        );

        let made = tools
            .call(
                "templates",
                json!({ "action": "create", "id": "midnight", "name": "Midnight", "from": "dusk" }),
            )
            .await
            .unwrap();
        assert_eq!(made["dir"], "/d/templates/midnight");
        assert_eq!(
            last_call(&engine),
            (
                methods::CREATE_TEMPLATE.to_owned(),
                json!({ "id": "midnight", "name": "Midnight", "from": "dusk" })
            )
        );
        tools
            .call(
                "templates",
                json!({ "action": "create", "id": "midnight", "name": "Midnight" }),
            )
            .await
            .unwrap();
        assert_eq!(
            last_call(&engine).1,
            json!({ "id": "midnight", "name": "Midnight" })
        );

        // R7: an agent's switch is a preview until the person keeps it.
        let used = tools
            .call("templates", json!({ "action": "use", "id": "midnight" }))
            .await
            .unwrap();
        assert_eq!((used["active"].as_str(), used["preview"].as_bool()), (Some("midnight"), Some(true)));
        assert!(used["next"].as_str().unwrap().contains("can't keep it for them"));
        assert_eq!(
            last_call(&engine),
            (
                methods::USE_TEMPLATE.to_owned(),
                json!({ "id": "midnight" })
            )
        );
        // The engine's words come back as they are.
        assert_eq!(
            tools
                .call("templates", json!({ "action": "use", "id": "nope" }))
                .await
                .unwrap_err()
                .message,
            "There's no template nope. list_templates names them."
        );

        let status = tools
            .call("templates", json!({ "action": "status", "id": "midnight" }))
            .await
            .unwrap();
        assert_eq!(status["file_errors"]["ds/tokens.luau"], "boom");
        assert_eq!(last_call(&engine).0, methods::TEMPLATE_STATUS);

        let deleted = tools
            .call("templates", json!({ "action": "delete", "id": "midnight" }))
            .await
            .unwrap();
        assert_eq!(deleted, json!({ "deleted": "midnight", "active": "zeron" }));
        assert_eq!(
            last_call(&engine),
            (
                methods::DELETE_TEMPLATE.to_owned(),
                json!({ "id": "midnight" })
            )
        );

        let described = tools.call("templates", json!({ "action": "app" })).await.unwrap();
        assert_eq!(described["screens"][0]["elements"], json!(["#sidebar"]));
        assert_eq!(
            last_call(&engine),
            (methods::DESCRIBE_APP.to_owned(), json!({}))
        );
    }

    #[tokio::test]
    async fn template_package_tools_forward_and_never_allow() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), None);

        let exported = tools
            .call("templates", json!({ "action": "export", "id": "midnight", "path": "/Users/x/.ssh/x.zeron-template" }))
            .await
            .unwrap();
        assert_eq!(exported["files"], 12);
        // R29: an agent never names where it goes (Zeron's exports folder).
        assert_eq!(last_call(&engine), (methods::EXPORT_TEMPLATE.to_owned(), json!({ "id": "midnight" })));

        // Yours: installed.
        let mine = tools
            .call("templates", json!({ "action": "import", "path": "/x/mine.zeron-template" }))
            .await
            .unwrap();
        assert_eq!(mine["installed"], "midnight");
        assert!(mine.get("next").is_none());
        assert_eq!(
            last_call(&engine),
            (methods::IMPORT_TEMPLATE.to_owned(), json!({ "path": "/x/mine.zeron-template" }))
        );
        // Someone else's: waiting for the person, and the reply says so.
        let theirs = tools
            .call("templates", json!({ "action": "import", "path": "/x/theirs.zeron-template" }))
            .await
            .unwrap();
        assert_eq!(theirs["needs_consent"], true);
        assert!(theirs["next"].as_str().unwrap().contains("can't allow it for them"));
        // Refused, in the engine's words.
        assert!(
            tools
                .call("templates", json!({ "action": "import", "path": "/x/bad.zeron-template" }))
                .await
                .unwrap_err()
                .contains("changed after it was signed")
        );

        // What's missing never reaches the engine.
        let before = engine.calls.lock().unwrap().len();
        assert!(tools.call("templates", json!({ "action": "export", "path": "/x/a.zeron-template" })).await.unwrap_err().message.contains("`id`"));
        assert!(tools.call("templates", json!({ "action": "import", "path": " " })).await.unwrap_err().message.contains("`path`"));
        assert_eq!(engine.calls.lock().unwrap().len(), before);

        // No tool decides for the person.
        let catalog = super::super::catalog();
        assert!(catalog.iter().all(|d| !d.name.contains("decide") && !d.name.contains("allow")));
        let templates = catalog.iter().find(|d| d.name == TEMPLATES).unwrap();
        for field in ["path", "url"] {
            assert!(templates.input_schema["properties"].get(field).is_some(), "{field}");
        }
        assert!(tools.call("templates", json!({ "action": "import", "path": "/x/a.zeron-template", "url": "ada/midnight" })).await.is_err());
        let before = engine.calls.lock().unwrap().len();
        let _ = tools.call("templates", json!({ "action": "import", "url": "ada/midnight" })).await;
        let calls = engine.calls.lock().unwrap();
        assert_eq!(calls.len(), before + 1);
        assert!(format!("{:?}", calls.last().unwrap()).contains("ada/midnight"));
    }

    #[tokio::test]
    async fn template_tools_need_an_id() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), None);
        for action in [
            CREATE_TEMPLATE,
            USE_TEMPLATE,
            TEMPLATE_STATUS,
            DELETE_TEMPLATE,
        ] {
            let error = tools
                .call("templates", json!({ "action": action, "name": "Midnight" }))
                .await
                .unwrap_err();
            assert!(error.message.contains("`id`"), "{action}: {error}");
            assert_eq!(error.code, crate::error::INVALID_ARGUMENTS);
            assert!(
                tools
                    .call("templates", json!({ "action": action, "id": " ", "name": "Midnight" }))
                    .await
                    .is_err(),
                "{action}"
            );
        }
        let unknown = tools.call("templates", json!({ "action": "rename" })).await.unwrap_err();
        assert!(unknown.message.contains("one of: list, status"), "{unknown}");
        assert!(
            tools
                .call("templates", json!({ "action": "create", "id": "midnight" }))
                .await
                .unwrap_err()
                .message
                .contains("`name`")
        );
        // None of them reached the engine.
        assert!(engine.calls.lock().unwrap().is_empty());
        let catalog = super::super::catalog();
        let templates = catalog.iter().find(|d| d.name == TEMPLATES).unwrap();
        let actions = templates.input_schema["properties"]["action"]["enum"].as_array().unwrap();
        for action in [LIST_TEMPLATES, DESCRIBE_APP, CREATE_TEMPLATE, USE_TEMPLATE, TEMPLATE_STATUS, DELETE_TEMPLATE] {
            assert!(actions.contains(&json!(action)), "{action}");
        }
    }

    fn tools_for(engine: Arc<Engine>, chat: Option<&str>) -> Tools {
        let origin = Origin {
            chat_id: chat.map(str::to_owned),
            ..Origin::default()
        };
        Tools::new(Arc::new(Zeron::with_client(memory_client(engine), origin)))
    }

    #[test]
    fn plugin_tool_names_are_told_apart_from_static_ones() {
        assert!(is_plugin_tool_name("word_count__count_words"));
        assert!(!is_plugin_tool_name("plugins"));
        assert!(!is_plugin_tool_name("__count"));
        assert!(!is_plugin_tool_name("Word__count"));
        for def in super::super::catalog() {
            assert!(!is_plugin_tool_name(def.name), "{}", def.name);
        }
    }

    #[tokio::test]
    async fn tools_list_includes_plugin_tools() {
        let tools = tools_for(Arc::new(Engine::default()), Some("chat-1"));
        let listed = tools.list_all().await;
        assert!(listed.iter().any(|t| t.name == "catalog"));
        assert!(listed.iter().any(|t| t.name == PLUGINS));
        assert!(listed.iter().any(|t| t.name == TEMPLATES));
        let plugin = listed
            .iter()
            .find(|t| t.name == "word_count__count_words")
            .expect("plugin tool listed");
        assert_eq!(
            plugin.description,
            "Count words in a text (From the Word Count plugin. Works while Zeron is open on Studio.)"
        );
        assert_eq!(plugin.input_schema["required"], json!(["text"]));
        assert_eq!(listed.len(), tools.list().len() + 1);
    }

    #[tokio::test]
    async fn an_engine_without_plugins_lists_the_static_tools() {
        let tools = tools_for(
            Arc::new(Engine {
                no_plugins: true,
                ..Engine::default()
            }),
            None,
        );
        assert_eq!(tools.list_all().await.len(), tools.list().len());
    }

    #[tokio::test]
    async fn a_plugin_tool_call_forwards_with_the_chat() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), Some("chat-1"));
        let value = tools
            .call("word_count__count_words", json!({ "text": "a b c" }))
            .await
            .unwrap();
        assert_eq!(value, json!({ "count": 3 }));
        let calls = engine.calls.lock().unwrap();
        let (method, params) = calls.last().unwrap();
        assert_eq!(method, methods::CALL_PLUGIN_TOOL);
        assert_eq!(
            params,
            &json!({
                "mcp_name": "word_count__count_words",
                "args": { "text": "a b c" },
                "chatId": "chat-1",
            })
        );
    }

    #[tokio::test]
    async fn a_plugin_tool_error_is_the_engine_message_and_an_is_error_result() {
        let engine = Arc::new(Engine {
            call_error: Some("Open Zeron on Studio to use count_words"),
            ..Engine::default()
        });
        let tools = tools_for(engine, None);
        assert_eq!(
            tools
                .call("word_count__count_words", json!({}))
                .await
                .unwrap_err()
                .message,
            "Open Zeron on Studio to use count_words"
        );
        let reply = crate::jsonrpc::handle_request(
            &tools,
            json!(7),
            "tools/call",
            json!({ "name": "word_count__count_words", "arguments": {} }),
        )
        .await;
        assert_eq!(reply["result"]["isError"], true);
        assert_eq!(
            reply["result"]["structuredContent"]["message"],
            "Open Zeron on Studio to use count_words"
        );
    }

    #[tokio::test]
    async fn create_plugin_and_list_plugins_forward() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), None);
        let created = tools
            .call(
                "plugins",
                json!({ "action": "create", "id": "focus", "name": "Focus", "permissions": ["ui.add"] }),
            )
            .await
            .unwrap();
        assert_eq!(created["id"], "focus");
        assert_eq!(created["dir"], "/nowhere/plugins/focus");
        assert_eq!(
            created["files"],
            json!(["init.luau", "zeron-plugin.json", "zeron.luau"])
        );
        assert_eq!(
            created["hint"],
            "Read /nowhere/docs/plugins.md first (how to write a plugin, with examples). Then edit /nowhere/plugins/focus/init.luau; Zeron re-signs and reloads it as you save. Check it with plugins {action: status, snapshot: true}."
        );
        assert_eq!(created["guide"], "/nowhere/docs/plugins.md");
        {
            let calls = engine.calls.lock().unwrap();
            let (method, params) = calls.last().unwrap();
            assert_eq!(method, methods::NEW_PLUGIN);
            assert_eq!(
                params,
                &json!({ "id": "focus", "name": "Focus", "permissions": ["ui.add"] })
            );
        }
        let listed = tools.call("plugins", json!({ "action": "list" })).await.unwrap();
        assert_eq!(
            listed,
            json!({ "plugins": [{
                "id": "focus", "name": "Focus", "status": "pending",
                "author": { "kind": "verified", "username": "ada" }, "dir": "/d/plugins/focus",
            }] })
        );
        assert_eq!(
            engine.calls.lock().unwrap().last().unwrap().0,
            methods::LIST_PLUGINS
        );
        let status = tools.call("plugins", json!({ "action": "status", "id": "focus", "snapshot": true })).await.unwrap();
        assert_eq!(status["placements"][0]["count"], 3);
        assert_eq!(
            last_call(&engine),
            (methods::PLUGIN_STATUS.to_owned(), json!({ "id": "focus", "snapshot": true }))
        );
        let map = tools.call("plugins", json!({ "action": "elements" })).await.unwrap();
        assert_eq!(map["elements"][0]["selector"], ".session-row");
        assert_eq!(
            engine.calls.lock().unwrap().last().unwrap().0,
            methods::DESCRIBE_ELEMENTS
        );
    }

    #[tokio::test]
    async fn install_and_update_plugin_leave_consent_to_the_person() {
        let engine = Arc::new(Engine::default());
        let tools = tools_for(engine.clone(), None);
        let installed = tools
            .call("plugins", json!({ "action": "install", "url": "ada/focus", "ref": "v1", "folder": " " }))
            .await
            .unwrap();
        assert_eq!(
            last_call(&engine),
            (methods::INSTALL_PLUGIN.to_owned(), json!({ "url": "ada/focus", "ref": "v1", "folder": null }))
        );
        assert_eq!((installed["status"].as_str(), installed["waiting"].as_bool()), (Some("pending"), Some(true)));
        assert!(installed["next"].as_str().unwrap().contains("can't allow it for them"));
        tools.call("plugins", json!({ "action": "install", "path": "/x/focus.zeron-plugin" })).await.unwrap();
        assert_eq!(last_call(&engine), (methods::INSTALL_PLUGIN.to_owned(), json!({ "path": "/x/focus.zeron-plugin" })));
        assert!(tools.call("plugins", json!({ "action": "install" })).await.unwrap_err().message.contains("`path`"));
        assert!(tools.call("plugins", json!({ "action": "install", "path": "/a", "url": "b/c" })).await.unwrap_err().message.contains("one of"));

        let checked = tools.call("plugins", json!({ "action": "updates", "check": true })).await.unwrap();
        assert_eq!(checked["updates"][0]["added"], json!(["github"]));
        assert_eq!(last_call(&engine), (methods::CHECK_PLUGIN_UPDATES.to_owned(), json!({ "id": null })));
        // R6: updating is the person's. The tool only checks, and says so.
        let updated = tools.call("plugins", json!({ "action": "updates", "id": "focus" })).await.unwrap();
        assert_eq!(updated["updates"][0]["version"], "2.0.0");
        assert!(updated["next"].as_str().unwrap().contains("can't update it for them"));
        assert_eq!(last_call(&engine), (methods::CHECK_PLUGIN_UPDATES.to_owned(), json!({ "id": "focus" })));
        tools.call("plugins", json!({ "action": "updates" })).await.unwrap();
        assert!(
            engine.calls.lock().unwrap().iter().all(|(method, _)| method != methods::UPDATE_PLUGIN),
            "the tool never installs an update"
        );
    }

    #[tokio::test]
    async fn create_plugin_lists_the_folder_it_made() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["zeron-plugin.json", "init.luau", "zeron.luau", ".hidden"] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        assert_eq!(
            scaffold_files(dir.path().to_str().unwrap()).await,
            vec!["init.luau", "zeron-plugin.json", "zeron.luau"]
        );
    }

    #[tokio::test]
    async fn the_watch_says_changed_only_when_the_names_change() {
        let engine = Arc::new(Engine {
            watch: vec![
                tools_reply(&["a__one"]),
                // Same names (an `open` flip, say): no change.
                tools_reply(&["a__one"]),
                tools_reply(&["a__one", "b__two"]),
            ],
            ..Engine::default()
        });
        let tools = Arc::new(tools_for(engine, None));
        let (tx, mut rx) = mpsc::channel(8);
        let watch = tokio::spawn({
            let tools = tools.clone();
            async move { tools.watch_plugin_tools(tx).await }
        });
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a change in time")
            .expect("one change");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "exactly one change"
        );
        watch.abort();
    }

    #[tokio::test]
    async fn the_watch_stops_on_an_engine_without_plugins() {
        let tools = tools_for(
            Arc::new(Engine {
                no_plugins: true,
                ..Engine::default()
            }),
            None,
        );
        let (tx, _rx) = mpsc::channel(1);
        tokio::time::timeout(Duration::from_secs(5), tools.watch_plugin_tools(tx))
            .await
            .expect("returns at once");
    }
}
