use std::path::Path;

use serde_json::{Value as Json, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::agent_messages::quote_headers;
use zeron_proto::{
    AccessMode, Chat, ChatConfig, HarnessId, ReadWorkspaceFileRequest, SandboxLevel,
    WorkspaceReadOnlyReason, WorkspaceTarget, cap_access,
};

use super::service::Acting;
use crate::workspace_files::WORKSPACE_FILE_RPC_TIMEOUT;
use crate::{DocHost, WorkspaceFiles, WorkspaceHost};

pub const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;
pub const PLUGIN_CHAT_ACCESS: (SandboxLevel, AccessMode) =
    (SandboxLevel::WorkspaceWrite, AccessMode::Supervised);

#[derive(Clone)]
pub struct Actions {
    pub device_id: String,
    pub workspace: WorkspaceHost,
    pub doc_host: DocHost,
    pub files: WorkspaceFiles,
    pub default_harness: HarnessId,
}

impl Actions {
    pub fn act(&self, plugin: &str, action: &str, args: Json) -> Acting {
        let (this, plugin, action) = (self.clone(), plugin.to_owned(), action.to_owned());
        Box::pin(async move {
            match action.as_str() {
                "chats.send" => this.send(&plugin, &args),
                "chats.create" => this.create(&plugin, &args),
                "chats.stop" => this.stop(&args),
                "files.read" => this.read(&args).await,
                other => Err(format!("zeron.{other} isn't something a service can do")),
            }
        })
    }

    fn chat(&self, args: &Json, what: &str) -> Result<Chat, String> {
        let id =
            field(args, "session").ok_or_else(|| format!("zeron.{what}: say which session"))?;
        self.workspace
            .chat(id)
            .map_err(|e| format!("zeron.{what}: {e}"))?
            .ok_or_else(|| format!("zeron.{what}: there's no session {id}"))
    }

    fn queue(&self, plugin: &str, chat_id: &str, text: &str, what: &str) -> Result<(), String> {
        self.doc_host
            .queue_plugin_message(chat_id, &quote_headers(text), plugin)
            .map(drop)
            .map_err(|e| format!("zeron.{what}: {e}"))
    }

    fn send(&self, plugin: &str, args: &Json) -> Result<Json, String> {
        let chat = self.chat(args, "chats.send")?;
        let text = said(args, "text").ok_or("zeron.chats.send: say what to send")?;
        self.queue(plugin, &chat.id, text, "chats.send")?;
        Ok(Json::Bool(true))
    }

    fn stop(&self, args: &Json) -> Result<Json, String> {
        let chat = self.chat(args, "chats.stop")?;
        self.doc_host
            .queue_command(&chat.id, SessionCommandPayload::Interrupt {})
            .map_err(|e| format!("zeron.chats.stop: {e}"))?;
        Ok(Json::Bool(true))
    }

    fn create(&self, plugin: &str, args: &Json) -> Result<Json, String> {
        let failed = |e: crate::EngineError| format!("zeron.chats.create: {e}");
        let project = field(args, "project");
        let device = field(args, "device");
        if let Some(project) = project {
            let space = self
                .workspace
                .space(project)
                .map_err(failed)?
                .ok_or_else(|| format!("zeron.chats.create: there's no project {project}"))?;
            if device.is_some_and(|d| d != space.device_id) {
                return Err(format!(
                    "zeron.chats.create: project {project} isn't on that device"
                ));
            }
            if on_cloud(&space.device_id) {
                return Err("zeron.chats.create: a plugin can't start Cloud chats".into());
            }
        }
        if device.is_some_and(on_cloud) {
            return Err("zeron.chats.create: a plugin can't start Cloud chats".into());
        }
        let device = match (project, device) {
            (Some(_), _) => None,
            (None, Some(device)) if device != self.device_id => {
                let known = self.workspace.read_devices().map_err(failed)?;
                if !known.iter().any(|d| d.id == device) {
                    return Err(format!("zeron.chats.create: there's no device {device}"));
                }
                Some(device)
            }
            (None, _) => Some(self.device_id.as_str()),
        };
        let harness = field(args, "harness")
            .map(|raw| {
                serde_json::from_value::<HarnessId>(json!(raw))
                    .map_err(|_| format!("zeron.chats.create: there's no harness \"{raw}\""))
            })
            .transpose()?;
        let model = field(args, "model");
        if model.is_some() && harness.is_none() {
            return Err("zeron.chats.create: a model needs its harness".into());
        }
        let (sandbox, access) = self.capped_at_calling_chats(args)?;
        let config = ChatConfig {
            harness: harness.unwrap_or(self.default_harness),
            model: model.map(str::to_owned),
            reasoning: None,
            model_options: Default::default(),
            sandbox,
            access,
            plan: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.workspace
            .create_chat_hosted(&id, project, device, None, Some(config), None, None, None)
            .map_err(failed)?;
        if let Some(title) = field(args, "title") {
            self.workspace.rename_chat(&id, title).map_err(failed)?;
        }
        if let Some(prompt) = said(args, "prompt") {
            self.queue(plugin, &id, prompt, "chats.create")?;
        }
        Ok(json!(id))
    }

    fn capped_at_calling_chats(&self, args: &Json) -> Result<(SandboxLevel, AccessMode), String> {
        let mut most = PLUGIN_CHAT_ACCESS;
        for calling in args["calling"].as_array().into_iter().flatten() {
            let id = calling.as_str().unwrap_or_default();
            let chat = self
                .workspace
                .chat(id)
                .map_err(|e| format!("zeron.chats.create: {e}"))?;
            let ceiling = match chat.map(|c| c.config) {
                Some(Some(config)) => (config.sandbox, config.access),
                Some(None) => (SandboxLevel::WorkspaceWrite, AccessMode::FullAccess),
                None => (SandboxLevel::ReadOnly, AccessMode::Supervised),
            };
            let (sandbox, access, _) = cap_access(most, ceiling);
            most = (sandbox, access);
        }
        Ok(most)
    }

    async fn read(&self, args: &Json) -> Result<Json, String> {
        let chat = self.chat(args, "files.read")?;
        let path = args["path"].as_str().unwrap_or_default();
        if path.is_empty() {
            return Err("zeron.files.read: say which file".into());
        }
        if path.starts_with(['/', '\\']) || Path::new(path).is_absolute() {
            return Err(format!(
                "zeron.files.read: \"{path}\" must be relative to the session's folder"
            ));
        }
        if chat.device_id != self.device_id {
            return Err("zeron.files.read: chat is on another device".into());
        }
        let request = ReadWorkspaceFileRequest {
            target: WorkspaceTarget {
                chat_id: Some(chat.id),
                space_id: None,
                checkout_path: None,
            },
            path: path.to_owned(),
        };
        let file = tokio::time::timeout(WORKSPACE_FILE_RPC_TIMEOUT, self.files.read_file(request))
            .await
            .map_err(|_| "zeron.files.read: timed out".to_owned())?
            .map_err(|e| format!("zeron.files.read: {e}"))?;
        if file.read_only_reason == Some(WorkspaceReadOnlyReason::Symlink) {
            return Err(format!("zeron.files.read: \"{path}\" is a symlink"));
        }
        if file.size > MAX_READ_BYTES {
            return Err(format!("zeron.files.read: \"{path}\" is over 2 MB"));
        }
        file.text
            .map(Json::String)
            .ok_or_else(|| format!("zeron.files.read: \"{path}\" isn't a text file"))
    }
}

fn field<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|v| !v.is_empty())
}

fn on_cloud(device: &str) -> bool {
    device.starts_with(zeron_proto::CLOUD_DEVICE_PREFIX)
}

fn said<'a>(args: &'a Json, key: &str) -> Option<&'a str> {
    args[key].as_str().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests;
