use super::*;
use zeron_proto::UserInputQuestion;

fn chat(id: &str, device: &str, title: Option<&str>) -> Chat {
    serde_json::from_value(json!({
        "id": id, "deviceId": device, "title": title, "archived": false,
        "spaceId": "proj-1", "createdAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

fn session(id: &str, device: &str, status: &str, started: Option<&str>) -> Session {
    serde_json::from_value(json!({
        "chatId": id, "deviceId": device, "status": status,
        "startedAt": started, "updatedAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

#[test]
fn agent_events_map_to_plain_payloads() {
    let mut heard = Heard::new("dev-1");
    heard.chats(&[chat("c1", "dev-1", Some("Fix the build"))]);
    let told = |heard: &mut Heard, event: AgentEvent| heard.agent("c1", &event);

    assert_eq!(
        told(&mut heard, AgentEvent::TextDelta { text: "Hel".into() }),
        None
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::ReasoningDelta { text: "hmm".into() }
        ),
        None
    );
    assert_eq!(
        told(&mut heard, AgentEvent::TextDelta { text: "lo".into() }),
        None
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::AssistantMessageCompleted {
                assistant_message_id: "m1".into()
            }
        ),
        Some((
            "message.assistant",
            json!({ "session": "c1", "text": "Hello" })
        ))
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::ToolCall {
                id: "t1".into(),
                call: ToolCall::Exec {
                    command: "cargo test".into()
                }
            }
        ),
        Some((
            "tool.call",
            json!({ "session": "c1", "call": "t1", "name": "exec", "input": { "command": "cargo test" } })
        ))
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::ToolCall {
                id: "t2".into(),
                call: ToolCall::Mcp {
                    server: "zeron".into(),
                    tool: "list_chats".into(),
                    input: None
                },
            }
        )
        .unwrap()
        .1["name"],
        "zeron__list_chats"
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::ToolResult {
                id: "t1".into(),
                is_error: true,
                output: Some("1 failed".into()),
                diff: None,
                exit_code: Some(1)
            }
        ),
        Some((
            "tool.result",
            json!({ "session": "c1", "call": "t1", "name": "exec", "ok": false, "output": "1 failed" })
        ))
    );
    assert_eq!(
        told(&mut heard, AgentEvent::UserMessage { text: "hi".into() }),
        Some(("message.user", json!({ "session": "c1", "text": "hi" })))
    );
    let question = UserInputQuestion {
        id: "q1".into(),
        header: "Plan".into(),
        question: "Which way?".into(),
        options: vec!["A".into(), "B".into()],
        multi_select: false,
        prefill: None,
        multiline: false,
    };
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::InputRequested {
                request_id: "r1".into(),
                questions: vec![question]
            }
        ),
        Some((
            "input.requested",
            json!({ "session": "c1", "kind": "choice", "summary": "Which way?" })
        ))
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::InputResolved {
                request_id: "r1".into(),
                answers: None
            }
        ),
        Some(("input.resolved", json!({ "session": "c1" })))
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::Error {
                message: "rate limited".into()
            }
        ),
        Some((
            "agent.error",
            json!({ "session": "c1", "message": "rate limited" })
        ))
    );
    assert_eq!(
        told(
            &mut heard,
            AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some("boom".into()),
                session_id: None,
                failure: None,
            }
        ),
        Some((
            "turn.done",
            json!({ "session": "c1", "status": "errored", "error": "boom", "title": "Fix the build" })
        ))
    );
    let nested = AgentEvent::Subagent {
        parent_tool_use_id: "t9".into(),
        event: Box::new(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
            failure: None,
        }),
    };
    assert_eq!(
        told(&mut heard, nested),
        None,
        "a subagent's events stay its own"
    );
}

#[test]
fn words_are_pinned() {
    assert_eq!(status_word(SessionStatus::AwaitingInput), "awaiting_input");
    assert_eq!(status_word(SessionStatus::Idle), "idle");
    assert_eq!(status_word(SessionStatus::Working), "working");
    assert_eq!(status_word(SessionStatus::Errored), "errored");
    assert_eq!(done_word(DoneStatus::Completed), "completed");
    assert_eq!(done_word(DoneStatus::Interrupted), "interrupted");
}

#[test]
fn without_messages_read_what_was_said_is_left_out() {
    let said = json!({
        "session": "c1", "call": "t1", "name": "exec", "ok": true, "status": "errored",
        "text": "x", "input": { "command": "rm" }, "output": "y", "error": "z", "message": "m", "summary": "s",
    });
    assert_eq!(
        redact(&said),
        json!({ "session": "c1", "call": "t1", "name": "exec", "ok": true, "status": "errored" })
    );
}

#[test]
fn long_text_is_clipped_on_a_char_boundary() {
    let heard = Heard::new("dev-1");
    let long = "é".repeat(MAX_SAID_BYTES);
    let (_, data) = heard.prompt("c1", &long);
    let text = data["text"].as_str().unwrap();
    assert!(text.len() <= MAX_SAID_BYTES && text.len() > MAX_SAID_BYTES - 2);
}

#[test]
fn statuses_tell_changes_and_turn_starts_on_this_device_only() {
    let mut heard = Heard::new("dev-1");
    let first = heard.sessions(&[session(
        "old",
        "dev-1",
        "working",
        Some("2026-01-01T00:00:00Z"),
    )]);
    assert!(first.is_empty(), "the first look is the baseline");
    let told = heard.sessions(&[
        session(
            "old",
            "dev-1",
            "awaitingInput",
            Some("2026-01-01T00:00:00Z"),
        ),
        session("new", "dev-1", "working", Some("2026-01-01T00:01:00Z")),
        session("far", "dev-2", "working", Some("2026-01-01T00:01:00Z")),
    ]);
    assert_eq!(
        told,
        vec![
            (
                "session.status",
                json!({ "session": "new", "status": "working", "previous": "idle" })
            ),
            ("turn.started", json!({ "session": "new" })),
            (
                "session.status",
                json!({ "session": "old", "status": "awaiting_input", "previous": "working" })
            ),
        ]
    );
    let told = heard.sessions(&[
        session("old", "dev-1", "working", Some("2026-01-01T00:02:00Z")),
        session("new", "dev-1", "idle", None),
    ]);
    assert_eq!(
        told,
        vec![
            (
                "session.status",
                json!({ "session": "new", "status": "idle", "previous": "working" })
            ),
            (
                "session.status",
                json!({ "session": "old", "status": "working", "previous": "awaiting_input" })
            ),
            ("turn.started", json!({ "session": "old" })),
        ]
    );
}

#[test]
fn a_chat_hosted_here_is_told_once_as_created() {
    let mut heard = Heard::new("dev-1");
    assert!(
        heard.chats(&[chat("a", "dev-1", None)]).is_empty(),
        "the first look is the baseline"
    );
    let told = heard.chats(&[
        chat("a", "dev-1", None),
        chat("b", "dev-1", Some("New")),
        chat("c", "dev-2", None),
    ]);
    assert_eq!(
        told,
        vec![(
            "session.created",
            json!({ "session": "b", "title": "New", "project": "proj-1" })
        )]
    );
    assert!(
        heard
            .chats(&[
                chat("a", "dev-1", Some("Titled")),
                chat("b", "dev-1", Some("New"))
            ])
            .is_empty()
    );
}
