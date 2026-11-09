use super::*;

const NEXT: &str = "Completion wakes this thread, so end your turn. Do not wait on it or poll it.";

fn running() -> Value {
    json!({
        "taskId": "task-1", "childThreadId": "child-1", "status": "running",
        "summary": null, "hasPendingQuestion": false, "waitTimedOut": false, "next": NEXT
    })
}

fn orchestrating(chat: &str) -> Origin {
    Origin {
        orchestrator: true,
        ..origin(chat)
    }
}

fn world(reply: Result<Value, &'static str>) -> Arc<World> {
    Arc::new(World {
        orch_reply: Some(reply),
        beta_parent: Some("chat-alpha-1".into()),
        extra_chats: vec![json!({
            "id": "chat-careful", "deviceId": "dev-local", "title": "Careful",
            "archived": false, "spaceId": "space-1",
            "config": { "harness": "claude-code", "sandbox": "read-only", "access": "supervised" },
            "createdAt": "2026-09-01T00:00:00Z"
        })],
        ..Default::default()
    })
}

fn orch_writes(world: &World, method: &str) -> Vec<Value> {
    world
        .writes
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| p.clone())
        .collect()
}

fn names(tools: &Tools) -> Vec<&'static str> {
    tools.list().iter().map(|t| t.name).collect()
}

#[tokio::test]
async fn the_flag_off_lists_todays_tools_and_refuses_the_delegate_tools() {
    let world = world(Ok(running()));
    let off = tools(world.clone(), origin("chat-alpha-1"));
    let today: Vec<&str> = catalog()
        .iter()
        .map(|t| t.name)
        .filter(|n| !delegate::NAMES.contains(n) && *n != WAIT_ON_BACKGROUND)
        .collect();
    assert_eq!(names(&off), today);
    assert_eq!(today.len(), 15);
    let refused = off
        .call("delegate_task", json!({ "task": "x" }))
        .await
        .unwrap_err();
    assert_eq!(refused.code, crate::error::UNKNOWN_TOOL);
    let no_chat = tools(
        world.clone(),
        Origin {
            orchestrator: true,
            ..Default::default()
        },
    );
    assert!(names(&no_chat).iter().all(|n| !delegate::NAMES.contains(n)));
    assert!(!no_chat.guidance_features().delegation && !off.guidance_features().delegation);
    assert!(orch_writes(&world, methods::ORCH_DELEGATE).is_empty());
}

#[tokio::test]
async fn the_flag_on_lists_the_three_tools_with_annotations_and_output_schemas() {
    let on = tools(world(Ok(running())), orchestrating("chat-alpha-1"));
    assert!(on.guidance_features().delegation);
    let listed: Vec<ListedTool> = on.list().into_iter().map(ListedTool::from).collect();
    assert_eq!(listed.len(), 18);
    let hints = |name: &str| {
        let tool = listed.iter().find(|t| t.name == name).unwrap();
        assert_eq!(
            tool.output_schema.as_ref().unwrap()["type"],
            "object",
            "{name}"
        );
        let a = tool.annotations.clone().unwrap();
        (
            a["title"].clone(),
            [
                &a["readOnlyHint"],
                &a["destructiveHint"],
                &a["idempotentHint"],
                &a["openWorldHint"],
            ]
            .map(|v| v == true),
        )
    };
    assert_eq!(
        hints("delegate_task"),
        (json!("Delegate a child task"), [false, true, false, true])
    );
    assert_eq!(
        hints("task_status"),
        (
            json!("Get delegated task status"),
            [false, false, true, false]
        )
    );
    assert_eq!(
        hints("task_cancel"),
        (json!("Cancel delegated task"), [false, true, false, false])
    );
}

#[tokio::test]
async fn delegate_task_resolves_the_target_and_passes_the_engine_result_through() {
    let world = world(Ok(running()));
    let on = tools(world.clone(), orchestrating("chat-alpha-1"));
    let out = on
        .call(
            "delegate_task",
            json!({
                "task": "Add tests.", "title": " Tests ", "role": "test", "mode": "wait", "timeoutMs": 5000,
                "clientRequestId": "r1", "target": { "project": "space-1", "model": "opus" }
            }),
        )
        .await
        .unwrap();
    assert_eq!(out, running());
    let sent = orch_writes(&world, methods::ORCH_DELEGATE);
    assert_eq!(sent.len(), 1);
    let sent = &sent[0];
    let child = &sent["child"];
    assert!(uuid::Uuid::parse_str(child["chatId"].as_str().unwrap()).is_ok());
    let expected = json!({
        "originChatId": "chat-alpha-1", "task": "Add tests.", "title": "Tests", "role": "test",
        "mode": "wait", "timeoutMs": 5000, "clientRequestId": "r1",
        "child": {
            "chatId": child["chatId"], "deviceId": "dev-local", "spaceId": "space-1",
            "config": { "harness": "claude-code", "model": "opus", "reasoning": null, "modelOptions": {}, "sandbox": "workspace-write" }
        }
    });
    assert_eq!(sent, &expected);
    assert!(
        world
            .writes
            .lock()
            .unwrap()
            .iter()
            .all(|(_, p)| p["op"] != "createChat" && p["op"] != "renameChat")
    );
}

#[tokio::test]
async fn delegate_task_defaults_to_async_caps_access_and_ignores_the_one_level_rule() {
    let world = world(Ok(running()));
    tools(world.clone(), orchestrating("chat-careful"))
        .call(
            "delegate_task",
            json!({ "task": "Read the docs.", "timeoutMs": 9 }),
        )
        .await
        .unwrap();
    tools(world.clone(), orchestrating("chat-beta-2"))
        .call(
            "delegate_task",
            json!({ "task": "Nested.", "mode": "wait", "timeoutMs": 99_999_999 }),
        )
        .await
        .unwrap();
    let sent = orch_writes(&world, methods::ORCH_DELEGATE);
    assert_eq!(sent[0]["mode"], "async");
    assert!(sent[0].get("timeoutMs").is_none() && sent[0].get("role").is_none());
    assert_eq!(sent[0]["child"]["config"]["access"], "supervised");
    assert_eq!(sent[0]["child"]["config"]["sandbox"], "read-only");
    assert_eq!(sent[1]["originChatId"], "chat-beta-2");
    assert_eq!(sent[1]["timeoutMs"], 3_600_000);
}

#[tokio::test]
async fn worktree_isolation_sends_a_worktree_spec_for_a_git_project_only() {
    let world = world(Ok(running()));
    let on = tools(world.clone(), orchestrating("chat-alpha-1"));
    on.call("delegate_task", json!({ "task": "Branch work.", "isolation": "worktree", "target": { "project": "space-1" } }))
        .await
        .unwrap();
    let sent = orch_writes(&world, methods::ORCH_DELEGATE);
    assert_eq!(
        sent[0]["child"]["worktree"],
        json!({ "repoPath": "/repo/comet", "base": "HEAD", "spaceId": "space-1" })
    );
    let refused = on
        .call(
            "delegate_task",
            json!({ "task": "Branch work.", "isolation": "worktree" }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, delegate::INVALID_REQUEST);
    assert!(refused.message.contains("target.project"), "{refused}");
    assert_eq!(orch_writes(&world, methods::ORCH_DELEGATE).len(), 1);
}

#[tokio::test]
async fn bad_arguments_and_targets_fail_before_the_engine_is_asked() {
    let world = world(Ok(running()));
    let on = tools(world.clone(), orchestrating("chat-alpha-1"));
    for args in [
        json!({ "task": " " }),
        json!({ "task": "x", "mode": "later" }),
        json!({ "task": "x", "title": "t".repeat(513) }),
        json!({ "task": "x".repeat(120_001) }),
        json!({ "task": "x", "target": { "model": "gpt-9" } }),
        json!({ "task": "x", "target": { "harness": "codex" } }),
    ] {
        assert!(
            on.call("delegate_task", args.clone()).await.is_err(),
            "{args}"
        );
    }
    assert!(orch_writes(&world, methods::ORCH_DELEGATE).is_empty());
}

#[tokio::test]
async fn engine_failures_become_their_code_and_a_fix() {
    let on = tools(
        world(Err("capability_denied: depth 2 reached")),
        orchestrating("chat-alpha-1"),
    );
    let error = on
        .call("delegate_task", json!({ "task": "x" }))
        .await
        .unwrap_err();
    assert_eq!(error.code, "capability_denied");
    assert!(
        error
            .message
            .starts_with("depth 2 reached Depth or fan-out cap reached"),
        "{error}"
    );
    assert_eq!(error.to_json()["code"], "capability_denied");
    let other = delegate::engine_failure("boom: no such thing");
    assert_eq!(
        (other.code, other.message.as_str()),
        (delegate::ORCHESTRATION_ERROR, "boom: no such thing")
    );
    let missing = tools(
        world(Err("task_not_found: task-9")),
        orchestrating("chat-alpha-1"),
    )
    .call("task_status", json!({ "taskId": "task-9" }))
    .await
    .unwrap_err();
    assert_eq!(missing.code, "task_not_found");
}

#[tokio::test]
async fn task_status_and_task_cancel_send_their_params() {
    let cancelled = json!({ "taskId": "task-1", "status": "cancel_requested" });
    let world = world(Ok(cancelled.clone()));
    let on = tools(world.clone(), orchestrating("chat-alpha-1"));
    on.call("task_status", json!({ "taskId": "task-1" }))
        .await
        .unwrap();
    let out = on
        .call(
            "task_cancel",
            json!({ "taskId": "task-1", "reason": "Done", "clientRequestId": "c1" }),
        )
        .await
        .unwrap();
    assert_eq!(out, cancelled);
    on.call("task_cancel", json!({ "taskId": "task-2" }))
        .await
        .unwrap();
    assert_eq!(
        orch_writes(&world, methods::ORCH_TASK_STATUS),
        [json!({ "originChatId": "chat-alpha-1", "taskId": "task-1" })]
    );
    assert_eq!(
        orch_writes(&world, methods::ORCH_TASK_CANCEL),
        [
            json!({ "originChatId": "chat-alpha-1", "taskId": "task-1", "reason": "Done", "clientRequestId": "c1" }),
            json!({ "originChatId": "chat-alpha-1", "taskId": "task-2" }),
        ]
    );
    let long = on
        .call(
            "task_cancel",
            json!({ "taskId": "t", "reason": "r".repeat(2_001) }),
        )
        .await;
    assert_eq!(long.unwrap_err().code, crate::error::INVALID_ARGUMENTS);
}
