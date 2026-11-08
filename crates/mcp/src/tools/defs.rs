use serde_json::{Value, json};

use super::{Hints, MAX_BATCH, ToolDef};

pub(super) const CATALOG: &str = "catalog";
pub(super) const LIST_CHATS: &str = "list_chats";
pub(super) const READ_CHAT: &str = "read_chat";
pub(super) const CREATE_CHATS: &str = "create_chats";
pub(super) const SEND_MESSAGES: &str = "send_messages";
pub(super) const WAIT_FOR_TURN: &str = "wait_for_turn";
pub(super) const INTERRUPT_CHAT: &str = "interrupt_chat";
pub(super) const ARCHIVE_CHAT: &str = "archive_chat";
pub(super) const RESPOND_TO_INPUT: &str = "respond_to_input";

fn chat_key() -> Value {
    json!({ "type": "string", "description": "Id, id prefix, or exact title." })
}

fn timeout() -> Value {
    json!({ "type": "integer", "maximum": 3600 })
}

fn with_chat(extra: Value) -> Value {
    let mut properties = json!({ "chat": chat_key() });
    if let (Some(base), Some(more)) = (properties.as_object_mut(), extra.as_object()) {
        base.extend(more.clone());
    }
    json!({ "type": "object", "properties": properties, "required": ["chat"] })
}

fn batch(item: Value) -> Value {
    json!({
        "type": "object",
        "properties": { "requests": { "type": "array", "minItems": 1, "maxItems": MAX_BATCH, "items": item } },
        "required": ["requests"]
    })
}

fn results_output() -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": { "type": "array", "items": { "type": "object", "properties": {
                "index": { "type": "integer" }, "ok": { "type": "boolean" }, "chatId": { "type": "string" },
                "error": { "type": "object" }, "turn": { "type": "object" }
            }, "required": ["index", "ok"] } },
            "next": { "type": "string" }
        },
        "required": ["results"]
    })
}

fn turn_output() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chatId": { "type": "string" }, "title": { "type": ["string", "null"] },
            "turn": { "type": "object", "properties": {
                "outcome": { "type": "string" }, "status": { "type": "string" },
                "timedOut": { "type": "boolean" }, "replies": { "type": "array" }
            } },
            "next": { "type": "string" }
        },
        "required": ["chatId", "turn"]
    })
}

const CATALOG_DESCRIPTION: &str = "Where chats can run. kind: self (your chat and device), devices, projects (folders on a device), harnesses (agent providers; connected ones take work from any provider), models (needs harness). Read-only. No device: the local engine. Use ids when names repeat.";

const LIST_CHATS_DESCRIPTION: &str = "List chats, newest activity first: id, title, project, device, harness, model, status (working, awaitingInput, completed, errored, idle), parent. Read-only. Default 30, no archived. parent \"me\": the chats you created.";

const READ_CHAT_DESCRIPTION: &str = "A chat's status and newest messages (tool calls one line each), plus pendingInput when it asks a question. Read-only. Default 20 messages; limit 0: status only; offset pages back. To wait for a reply, use wait_for_turn; do not poll this.";

const CREATE_CHATS_DESCRIPTION: &str = "Start 1-32 chats at once, a prompt each; they run in parallel. No kind: a subagent of your chat. No harness: a connected one, claude-code first. No wait: returns at once; collect replies with wait_for_turn. Results keep request order; one failure undoes nothing. Subagents cannot create chats. A repeated clientRequestId returns the first result. For chats that exist: send_messages.";

const SEND_MESSAGES_DESCRIPTION: &str = "Send 1-32 messages to chats that exist, at once, from your chat. No mode: auto (starts an idle chat, steers a busy one); queue holds it. No wait: returns at once; collect with wait_for_turn. Results keep request order. A repeated clientRequestId returns the first result. Not for a chat awaiting input (respond_to_input), nor two dependent messages to one chat.";

const WAIT_FOR_TURN_DESCRIPTION: &str = "Block until a chat's turn ends (completed, awaitingInput, interrupted, errored, timedOut) and return its new replies. After create_chats or send_messages it waits for that turn. Read-only. Default 600 s, max 3600. On awaitingInput, use respond_to_input. Stale or offline is not done.";

const INTERRUPT_CHAT_DESCRIPTION: &str = "Stop a chat's running turn. The chat stays; message it later with send_messages.";

const ARCHIVE_CHAT_DESCRIPTION: &str = "Hide a chat from the sidebar; archived false restores it. Use it to tidy chats you created. Deletes nothing.";

const RESPOND_TO_INPUT_DESCRIPTION: &str = "Answer the question a blocked chat asks (pendingInput in read_chat or wait_for_turn). labels: option labels, or free text for an open question. No request_id: the current question.";

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: CATALOG,
            title: "Zeron catalog",
            description: CATALOG_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["self", "devices", "projects", "harnesses", "models"] },
                    "device": { "type": "string" },
                    "harness": { "type": "string" }
                },
                "required": ["kind"]
            }),
            output_schema: Some(json!({
                "type": "object",
                "properties": { "kind": { "type": "string" }, "items": { "type": "array", "items": { "type": "object" } } },
                "required": ["kind", "items"]
            })),
            hints: Hints::READ,
            always_load: false,
            example: r#"{"kind":"harnesses"}"#,
        },
        ToolDef {
            name: LIST_CHATS,
            title: "List chats",
            description: LIST_CHATS_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string" },
                    "device": { "type": "string" },
                    "parent": { "type": "string" },
                    "include_archived": { "type": "boolean" },
                    "limit": { "type": "integer", "maximum": 500 }
                }
            }),
            output_schema: Some(json!({
                "type": "object",
                "properties": { "total": { "type": "integer" }, "chats": { "type": "array", "items": { "type": "object" } } },
                "required": ["total", "chats"]
            })),
            hints: Hints::READ,
            always_load: false,
            example: r#"{"parent":"me"}"#,
        },
        ToolDef {
            name: READ_CHAT,
            title: "Read chat",
            description: READ_CHAT_DESCRIPTION,
            input_schema: with_chat(json!({
                "limit": { "type": "integer", "maximum": 500 },
                "offset": { "type": "integer" },
                "include_reasoning": { "type": "boolean" },
                "include_tools": { "type": "boolean" }
            })),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "chat": { "type": "object" }, "pendingInput": { "type": ["object", "null"] },
                    "total": { "type": "integer" }, "olderRemaining": { "type": "integer" },
                    "messages": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["chat", "messages"]
            })),
            hints: Hints::READ,
            always_load: false,
            example: r#"{"chat":"Docs lane","limit":5}"#,
        },
        ToolDef {
            name: CREATE_CHATS,
            title: "Create chats",
            description: CREATE_CHATS_DESCRIPTION,
            input_schema: batch(json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "title": { "type": "string" },
                    "kind": { "type": "string", "enum": ["subagent", "side", "chat"], "description": "side: the person's side chat. chat: standalone." },
                    "harness": { "type": "string" },
                    "model": { "type": "string" },
                    "reasoning": { "type": "string" },
                    "project": { "type": "string" },
                    "device": { "type": "string" },
                    "parent": { "type": "string" },
                    "access": { "type": "string", "enum": ["supervised", "auto-accept-edits", "full-access"], "description": "Capped at yours." },
                    "sandbox": { "type": "string", "enum": ["read-only", "workspace-write", "danger-full-access"], "description": "Capped at yours." },
                    "plan": { "type": "boolean" },
                    "cwd": { "type": "string", "description": "Worktree path." },
                    "branch": { "type": "string" },
                    "cloud": { "type": "boolean", "description": "Own Cloud machine (Codex, Claude Code)." },
                    "wait": { "type": "boolean" },
                    "timeout_secs": timeout(),
                    "clientRequestId": { "type": "string" }
                }
            })),
            output_schema: Some(results_output()),
            hints: Hints::WRITE.open_world(),
            always_load: false,
            example: r#"{"requests":[{"title":"Docs","prompt":"Update docs/api.md for the new flag."},{"title":"Tests","prompt":"Add tests for the new flag."}]}"#,
        },
        ToolDef {
            name: SEND_MESSAGES,
            title: "Send messages",
            description: SEND_MESSAGES_DESCRIPTION,
            input_schema: batch(json!({
                "type": "object",
                "properties": {
                    "chat": chat_key(),
                    "text": { "type": "string" },
                    "mode": { "type": "string", "enum": ["auto", "run", "steer", "queue"] },
                    "wait": { "type": "boolean" },
                    "timeout_secs": timeout(),
                    "clientRequestId": { "type": "string" }
                },
                "required": ["chat", "text"]
            })),
            output_schema: Some(results_output()),
            hints: Hints::WRITE.open_world(),
            always_load: false,
            example: r#"{"requests":[{"chat":"Docs","text":"Also cover the CLI flag."}]}"#,
        },
        ToolDef {
            name: WAIT_FOR_TURN,
            title: "Wait for turn",
            description: WAIT_FOR_TURN_DESCRIPTION,
            input_schema: with_chat(json!({ "timeout_secs": timeout() })),
            output_schema: Some(turn_output()),
            hints: Hints::READ,
            always_load: false,
            example: r#"{"chat":"Docs"}"#,
        },
        ToolDef {
            name: INTERRUPT_CHAT,
            title: "Interrupt chat",
            description: INTERRUPT_CHAT_DESCRIPTION,
            input_schema: with_chat(json!({})),
            output_schema: None,
            hints: Hints::WRITE.destructive().idempotent(),
            always_load: false,
            example: r#"{"chat":"Docs"}"#,
        },
        ToolDef {
            name: ARCHIVE_CHAT,
            title: "Archive chat",
            description: ARCHIVE_CHAT_DESCRIPTION,
            input_schema: with_chat(json!({ "archived": { "type": "boolean" } })),
            output_schema: None,
            hints: Hints::WRITE.idempotent(),
            always_load: false,
            example: r#"{"chat":"Docs"}"#,
        },
        ToolDef {
            name: RESPOND_TO_INPUT,
            title: "Respond to input",
            description: RESPOND_TO_INPUT_DESCRIPTION,
            input_schema: with_chat(json!({
                "request_id": { "type": "string" },
                "answers": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question_id": { "type": "string" },
                            "labels": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["question_id", "labels"]
                    }
                }
            })),
            output_schema: None,
            hints: Hints::WRITE,
            always_load: false,
            example: r#"{"chat":"Docs","answers":[{"question_id":"q1","labels":["Yes"]}]}"#,
        },
    ]
}
