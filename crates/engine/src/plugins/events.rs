use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value as Json, json};
use zeron_proto::{AgentEvent, Chat, DoneStatus, Session, SessionStatus, ToolCall};

pub const SAID_FIELDS: &[&str] = &["text", "input", "output", "error", "message", "summary"];
pub const MAX_SAID_BYTES: usize = 16 * 1024;

pub type Told = (&'static str, Json);
type Seen = (SessionStatus, Option<DateTime<Utc>>);

#[derive(Default)]
pub struct Heard {
    device: String,
    titles: HashMap<String, String>,
    chats: Option<HashSet<String>>,
    sessions: Option<HashMap<String, Seen>>,
    tools: HashMap<(String, String), String>,
    said: HashMap<String, String>,
}

impl Heard {
    pub fn new(device: &str) -> Self {
        Self {
            device: device.to_owned(),
            ..Self::default()
        }
    }

    pub fn prompt(&self, chat: &str, text: &str) -> Told {
        (
            "message.user",
            json!({ "session": chat, "text": clip(text) }),
        )
    }

    pub fn agent(&mut self, chat: &str, event: &AgentEvent) -> Option<Told> {
        let session = chat.to_owned();
        Some(match event {
            AgentEvent::TextDelta { text } => {
                let said = self.said.entry(session).or_default();
                if said.len() < MAX_SAID_BYTES {
                    said.push_str(text);
                }
                return None;
            }
            AgentEvent::AssistantMessageCompleted { .. } => {
                let text = self.said.remove(chat).unwrap_or_default();
                let mut data = json!({ "session": session });
                if !text.is_empty() {
                    data["text"] = json!(clip(&text));
                }
                ("message.assistant", data)
            }
            AgentEvent::ToolCall { id, call } => {
                let name = tool_name(call);
                self.tools
                    .insert((session.clone(), id.clone()), name.clone());
                let mut input = serde_json::to_value(call).unwrap_or(Json::Null);
                if let Some(object) = input.as_object_mut() {
                    object.remove("kind");
                }
                (
                    "tool.call",
                    json!({ "session": session, "call": id, "name": name, "input": clip_json(input) }),
                )
            }
            AgentEvent::ToolResult {
                id,
                is_error,
                output,
                ..
            } => {
                let name = self
                    .tools
                    .remove(&(session.clone(), id.clone()))
                    .unwrap_or_default();
                let mut data =
                    json!({ "session": session, "call": id, "name": name, "ok": !is_error });
                if let Some(output) = output {
                    data["output"] = json!(clip(output));
                }
                ("tool.result", data)
            }
            AgentEvent::UserMessage { text } => self.prompt(chat, text),
            AgentEvent::InputRequested { questions, .. } => {
                let kind = if questions.iter().any(|q| !q.options.is_empty()) {
                    "choice"
                } else {
                    "text"
                };
                let mut data = json!({ "session": session, "kind": kind });
                if let Some(first) = questions.first() {
                    data["summary"] = json!(clip(&first.question));
                }
                ("input.requested", data)
            }
            AgentEvent::InputResolved { .. } => ("input.resolved", json!({ "session": session })),
            AgentEvent::Error { message } => (
                "agent.error",
                json!({ "session": session, "message": clip(message) }),
            ),
            AgentEvent::Done { status, error, .. } => {
                self.said.remove(chat);
                self.tools.retain(|(c, _), _| c != chat);
                let mut data = json!({ "session": session, "status": done_word(*status) });
                if let Some(error) = error {
                    data["error"] = json!(clip(error));
                }
                if let Some(title) = self.titles.get(chat) {
                    data["title"] = json!(title);
                }
                ("turn.done", data)
            }
            _ => return None,
        })
    }

    pub fn chats(&mut self, chats: &[Chat]) -> Vec<Told> {
        let here: Vec<&Chat> = chats
            .iter()
            .filter(|c| c.device_id == self.device)
            .collect();
        self.titles = here
            .iter()
            .filter_map(|c| {
                c.title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .map(|t| (c.id.clone(), t))
            })
            .collect();
        let ids: HashSet<String> = here.iter().map(|c| c.id.clone()).collect();
        let Some(known) = self.chats.replace(ids) else {
            return Vec::new();
        };
        here.into_iter()
            .filter(|c| !c.archived && !known.contains(&c.id))
            .map(|c| {
                let mut data = json!({ "session": c.id });
                if let Some(title) = c.title.as_ref().filter(|t| !t.is_empty()) {
                    data["title"] = json!(title);
                }
                if let Some(project) = &c.space_id {
                    data["project"] = json!(project);
                }
                ("session.created", data)
            })
            .collect()
    }

    pub fn sessions(&mut self, sessions: &[Session]) -> Vec<Told> {
        let now: HashMap<String, Seen> = sessions
            .iter()
            .filter(|s| s.device_id == self.device)
            .map(|s| (s.chat_id.clone(), (s.status, s.started_at)))
            .collect();
        let Some(was) = self.sessions.replace(now.clone()) else {
            return Vec::new();
        };
        let mut told = Vec::new();
        let mut ids: Vec<&String> = now.keys().collect();
        ids.sort();
        for id in ids {
            let (status, started) = now[id];
            let (previous, started_before) =
                was.get(id).copied().unwrap_or((SessionStatus::Idle, None));
            if status != previous {
                told.push((
                    "session.status",
                    json!({ "session": id, "status": status_word(status), "previous": status_word(previous) }),
                ));
            }
            if started.is_some() && started != started_before {
                told.push(("turn.started", json!({ "session": id })));
            }
        }
        told
    }
}

pub fn redact(data: &Json) -> Json {
    let mut out = data.clone();
    if let Some(object) = out.as_object_mut() {
        object.retain(|key, _| !SAID_FIELDS.contains(&key.as_str()));
    }
    out
}

pub fn status_word(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Working => "working",
        SessionStatus::AwaitingInput => "awaiting_input",
        SessionStatus::Errored => "errored",
    }
}

pub fn done_word(status: DoneStatus) -> &'static str {
    match status {
        DoneStatus::Completed => "completed",
        DoneStatus::Interrupted => "interrupted",
        DoneStatus::Errored => "errored",
    }
}

fn tool_name(call: &ToolCall) -> String {
    match call {
        ToolCall::Mcp { server, tool, .. } => format!("{server}__{tool}"),
        ToolCall::Unknown { name, .. } => name.clone(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v["kind"].as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_SAID_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_SAID_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn clip_json(value: Json) -> Json {
    match value {
        Json::String(s) => Json::String(clip(&s)),
        Json::Array(items) => Json::Array(items.into_iter().map(clip_json).collect()),
        Json::Object(map) => Json::Object(
            map.into_iter()
                .map(|(k, v)| (k, clip_json(v)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests;
