use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value as Json, json};
use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload};
use zeron_harness::{Harness, HarnessError, RunControls, approval};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_rpc::{RpcReply, methods};

use super::{APPROVE, DISPATCH, deciding, in_install_order};
use crate::plugins::{MANIFEST_FILE, Plugins, authority_tests, scaffold, service, service_plan};
use crate::{EngineCore, HarnessRegistry};

struct Agent;

async fn reply(prompt: String, controls: RunControls) -> String {
    let Some(command) = prompt.strip_prefix("ask:") else {
        return format!("ack: {prompt}");
    };
    let question = approval::command("Agent", command);
    let answers = (controls.request_input)(vec![question.clone()])
        .await
        .unwrap_or_default();
    if approval::approved(&question, &answers) {
        format!("ran {command}")
    } else {
        format!(
            "refused: {}",
            approval::denied_because(&question, &answers).unwrap_or("no")
        )
    }
}

#[async_trait]
impl Harness for Agent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Agent"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let cwd = request.cwd.clone();
        let started = Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd,
            session_id: "sess-hook".into(),
            assistant_message_id: format!("a-{}", uuid::Uuid::new_v4()),
        });
        let rest = futures::stream::once(reply(request.prompt, controls)).flat_map(|text| {
            futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta { text }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("sess-hook".into()),
                    failure: None,
                }),
            ])
        });
        Ok(futures::stream::once(async { started }).chain(rest).boxed())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    core: EngineCore,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Agent));
    let core = EngineCore::assemble(
        &dir.path().join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    Rig { _dir: dir, core }
}

async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never: {what}");
}

async fn call(plugins: &Plugins, method: &str, params: Json) -> Json {
    let params = authority_tests::as_consent_window(plugins, method, params).await;
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => value,
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(error) => json!({ "rpc_error": error.to_string() }),
    }
}

impl Rig {
    fn plugins(&self) -> &Plugins {
        &self.core.plugins
    }

    async fn allow(&self, id: &str) {
        let plugins = self.plugins();
        let signed = call(plugins, methods::SIGN_PLUGIN, json!({ "id": id })).await;
        let decided = call(
            plugins,
            methods::DECIDE_PLUGIN,
            json!({ "id": id, "allow": true, "hash": signed["hash"] }),
        )
        .await;
        assert_eq!(decided["status"], "on", "{decided}");
        until("its service runs", || {
            plugins.inner.services.status(id)["running"] == true
        })
        .await;
    }

    async fn install(&self, id: &str, hooks: &[&str], permissions: &[&str], source: &str) {
        let dir = self.plugins().plugins_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = json!({ "id": id, "name": id.replace('-', " "), "version": "1.0.0", "entry": "init.luau",
            "service": "service.luau", "permissions": permissions, "hooks": hooks });
        std::fs::write(dir.join(MANIFEST_FILE), manifest.to_string()).unwrap();
        std::fs::write(dir.join("init.luau"), "").unwrap();
        std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
        std::fs::write(dir.join("service.luau"), source.replace("ID", id)).unwrap();
        self.allow(id).await;
    }

    async fn example(&self, id: &str) {
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/zgx/plugins-dev")
            .join(id);
        let dir = self.plugins().plugins_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        for file in [MANIFEST_FILE, "init.luau", "service.luau", "zeron.luau"] {
            std::fs::copy(example.join(file), dir.join(file)).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("zeron.luau")).unwrap(),
            scaffold::SDK,
            "its zeron.luau is the SDK's"
        );
        self.allow(id).await;
    }

    fn chat(&self, id: &str) {
        self.core
            .workspace
            .create_chat(id, None, Some(&self.core.device_id), None, None)
            .unwrap();
    }

    fn run(&self, chat: &str, prompt: &str) {
        let request = RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "~".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: Default::default(),
            plan: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
            extra_mcp: Vec::new(),
        };
        let message_id = uuid::Uuid::new_v4().to_string();
        self.core
            .doc_host
            .queue_command(
                chat,
                SessionCommandPayload::Run {
                    request,
                    message_id,
                },
            )
            .unwrap();
    }

    fn queue(&self, chat: &str, text: &str) {
        self.core
            .doc_host
            .queue_message(chat, text, Vec::new())
            .unwrap();
    }

    fn plugin_send(&self, chat: &str, text: &str) {
        self.core
            .doc_host
            .queue_plugin_message(chat, text, "sender")
            .unwrap();
    }

    fn queued(&self, chat: &str) -> usize {
        self.core
            .doc_host
            .open(chat)
            .unwrap()
            .doc()
            .read_queue()
            .unwrap()
            .len()
    }

    fn parts(&self, chat: &str, role: MessageRole) -> Vec<MessagePart> {
        let entries = self
            .core
            .doc_host
            .open(chat)
            .ok()
            .and_then(|h| h.doc().read_entries().ok())
            .unwrap_or_default();
        entries
            .into_iter()
            .filter(|e| e.role == role)
            .flat_map(|e| e.parts)
            .collect()
    }

    fn said(&self, chat: &str, role: MessageRole) -> Vec<String> {
        self.parts(chat, role)
            .into_iter()
            .filter_map(|p| match p {
                MessagePart::Text { text, .. } if !text.is_empty() => Some(text),
                _ => None,
            })
            .collect()
    }

    fn notices(&self, chat: &str) -> Vec<String> {
        self.parts(chat, MessageRole::Assistant)
            .into_iter()
            .filter_map(|p| match p {
                MessagePart::Error { message, .. } => Some(message),
                _ => None,
            })
            .collect()
    }

    fn asked_person(&self, chat: &str) -> bool {
        self.parts(chat, MessageRole::Assistant)
            .iter()
            .any(|p| matches!(p, MessagePart::Input { .. }))
    }

    fn replied(&self, chat: &str, text: &str) -> bool {
        self.said(chat, MessageRole::Assistant)
            .iter()
            .any(|t| t == text)
    }

    fn kept(&self, plugin: &str, key: &str) -> Json {
        let path = self
            .plugins()
            .plugins_dir()
            .join(".storage")
            .join(format!("{plugin}.service.json"));
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Json>(&b).ok())
            .map_or(Json::Null, |all| all[key].clone())
    }

    fn log(&self, plugin: &str) -> String {
        self.plugins().inner.services.status(plugin)["log"].to_string()
    }

    async fn serve(&self, plugin: &str, method: &str) -> Json {
        let plan = self
            .plugins()
            .entry(plugin)
            .as_ref()
            .and_then(service_plan)
            .unwrap();
        let services = &self.plugins().inner.services;
        services
            .call(
                &plan,
                service::Kind::Method,
                method,
                json!({}),
                json!({}),
                Duration::from_secs(5),
            )
            .await
            .unwrap()
    }
}

const GATE: &str = r#"
local zeron = require("./zeron")("ID")
local seen = zeron.store("seen", {})
local open = zeron.store("open", false)
zeron.hook("message.dispatch", function(ctx)
	local all = seen:get()
	table.insert(all, ctx)
	seen:set(all)
	if ctx.source == "plugin" and not open:get() then
		return { action = "wait", reason = "not yet" }
	elseif ctx.source == "queue" then
		return { action = "reject", message = "no queue today" }
	end
	return nil
end)
zeron.serve("open", function()
	open:set(true)
	return zeron.hooks.recheck("message.dispatch"):await()
end)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_proceeds_waits_until_rechecked_and_rejects_with_a_notice() {
    let r = rig();
    r.install("gate", &[DISPATCH], &["service", "hooks.dispatch"], GATE)
        .await;
    assert!(r.plugins().hooks(DISPATCH) && !r.plugins().hooks(APPROVE));
    r.chat("c1");
    r.run("c1", "hello");
    until("the person's message goes", || {
        r.replied("c1", "ack: hello")
    })
    .await;

    r.plugin_send("c1", "from a plugin");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(r.queued("c1"), 1, "it waits in the queue");
    assert!(!r.replied("c1", "ack: from a plugin"));
    assert_eq!(r.serve("gate", "open").await, json!(true));
    until("the recheck lets it go", || {
        r.replied("c1", "ack: from a plugin")
    })
    .await;

    r.queue("c1", "queued");
    until("refused", || {
        r.notices("c1")
            .iter()
            .any(|n| n.contains("gate") && n.contains("no queue today"))
    })
    .await;
    assert_eq!(r.queued("c1"), 0);
    assert!(
        r.said("c1", MessageRole::User)
            .contains(&"queued".to_owned()),
        "it lands unsent"
    );
    assert!(!r.replied("c1", "ack: queued"));

    let seen = r.kept("gate", "seen");
    let sources: Vec<&str> = seen
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["source"].as_str())
        .collect();
    for source in ["person", "plugin", "queue"] {
        assert!(sources.contains(&source), "{seen}");
    }
    for ctx in seen.as_array().unwrap() {
        assert_eq!(ctx["session"], "c1");
        assert!(
            ctx.get("text").is_none(),
            "no text without messages.read: {ctx}"
        );
        assert_eq!(
            ctx.get("plugin").is_some(),
            ctx["source"] == "plugin",
            "{ctx}"
        );
    }
    assert!(
        seen.as_array()
            .unwrap()
            .iter()
            .any(|c| c["plugin"] == "sender")
    );
    r.core.shutdown().await;
}

const FIRST: &str = r#"
local zeron = require("./zeron")("ID")
local deadline = zeron.store("deadline", 0)
zeron.hook("message.dispatch", function(ctx)
	if ctx.text == "boom" then
		error("it broke")
	elseif ctx.text == "slow" then
		zeron.sleep(5):await()
		return { action = "reject", message = "too late" }
	elseif ctx.text == "both" then
		return { action = "wait", reason = "first" }
	elseif ctx.text == "later" then
		if deadline:get() == 0 then
			deadline:set(os.time() + 2)
		end
		if os.time() < deadline:get() then
			return { action = "wait", reason = "soon", ["until"] = deadline:get() }
		end
	end
	return nil
end)
"#;

const SECOND: &str = r#"
local zeron = require("./zeron")("ID")
local seen = zeron.store("seen", {})
zeron.hook("message.dispatch", function(ctx)
	local all = seen:get()
	table.insert(all, ctx.text)
	seen:set(all)
	if ctx.text == "both" then
		return { action = "reject", message = "second" }
	end
	return nil
end)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_runs_in_order_wakes_at_until_and_fails_open() {
    let r = rig();
    let permissions = ["service", "hooks.dispatch", "messages.read"];
    r.install("a-first", &[DISPATCH], &permissions, FIRST).await;
    r.install("b-second", &[DISPATCH], &permissions, SECOND)
        .await;
    for chat in ["c-until", "c-order", "c-boom", "c-slow"] {
        r.chat(chat);
    }

    let asked = Instant::now();
    r.queue("c-until", "later");
    until("woken at until", || r.replied("c-until", "ack: later")).await;
    assert!(
        asked.elapsed() >= Duration::from_millis(1000),
        "{:?}",
        asked.elapsed()
    );

    r.queue("c-order", "both");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(r.queued("c-order"), 1, "the first plugin's wait wins");
    assert!(r.notices("c-order").is_empty());
    let second_saw = r.kept("b-second", "seen");
    assert!(
        !second_saw
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| t == "both"),
        "the second isn't asked: {second_saw}"
    );

    r.queue("c-boom", "boom");
    until("an error proceeds", || r.replied("c-boom", "ack: boom")).await;
    assert!(
        r.log("a-first").contains("it broke"),
        "{}",
        r.log("a-first")
    );

    r.queue("c-slow", "slow");
    until("a timeout proceeds", || r.replied("c-slow", "ack: slow")).await;
    assert!(r.notices("c-slow").is_empty(), "the late reject is ignored");
    assert!(
        r.log("a-first").contains("within 2 s"),
        "{}",
        r.log("a-first")
    );
    r.core.shutdown().await;
}

const APPROVER: &str = r#"
local zeron = require("./zeron")("ID")
zeron.hook("tool.approve", function(ctx)
	local command = ctx.input.command
	if command == "boom" then
		error("it broke")
	elseif ctx.name == "ls" or ctx.name == "git" then
		return "allow"
	elseif ctx.name == "rm" then
		return "deny"
	end
	return "ask"
end)
"#;

const GIT_DENIER: &str = r#"
local zeron = require("./zeron")("ID")
local seen = zeron.store("seen", {})
zeron.hook("tool.approve", function(ctx)
	local all = seen:get()
	table.insert(all, ctx)
	seen:set(all)
	return if ctx.name == "git" then "deny" else nil
end)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn approve_allows_denies_asks_and_a_deny_beats_an_allow() {
    let r = rig();
    r.install(
        "a-approver",
        &[APPROVE],
        &["service", "hooks.approve", "messages.read"],
        APPROVER,
    )
    .await;
    r.install(
        "b-git-denier",
        &[APPROVE],
        &["service", "hooks.approve"],
        GIT_DENIER,
    )
    .await;
    for chat in ["c-ls", "c-rm", "c-git", "c-boom", "c-make"] {
        r.chat(chat);
    }
    r.run("c-ls", "ask:ls -la");
    r.run("c-rm", "ask:rm -rf /");
    r.run("c-git", "ask:git push");
    r.run("c-boom", "ask:boom");
    r.run("c-make", "ask:make");

    until("allowed", || r.replied("c-ls", "ran ls -la")).await;
    assert!(
        !r.asked_person("c-ls"),
        "an allowed request never reaches the person"
    );
    until("denied", || {
        r.replied("c-rm", "refused: denied by a approver")
    })
    .await;
    until("a deny beats an allow", || {
        r.replied("c-git", "refused: denied by b git denier")
    })
    .await;
    assert!(!r.asked_person("c-rm") && !r.asked_person("c-git"));
    until("an error asks", || r.asked_person("c-boom")).await;
    until("ask asks", || r.asked_person("c-make")).await;
    assert!(
        r.said("c-make", MessageRole::Assistant).is_empty(),
        "still waiting on the person"
    );

    let log = r.log("a-approver");
    assert!(log.contains("allowed command ls in chat c-ls"), "{log}");
    assert!(log.contains("denied command rm in chat c-rm"), "{log}");
    assert!(log.contains("it broke"), "{log}");
    assert!(
        r.log("b-git-denier")
            .contains("denied command git in chat c-git")
    );
    for ctx in r.kept("b-git-denier", "seen").as_array().unwrap() {
        assert!(
            ctx.get("input").is_none() && ctx.get("cwd").is_none(),
            "no input or cwd without their permissions: {ctx}"
        );
        assert_eq!(ctx["kind"], "command");
    }
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_quiet_hours_example_holds_plugin_sends_and_lets_the_person_through() {
    let r = rig();
    let hour = chrono::Timelike::hour(&chrono::Local::now()) as i64;
    let settings = r
        .plugins()
        .plugins_dir()
        .join(".storage")
        .join("quiet-hours.settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        json!({ "start": hour, "end": (hour + 2) % 24 }).to_string(),
    )
    .unwrap();
    r.example("quiet-hours").await;
    r.chat("c-night");
    r.plugin_send("c-night", "theirs");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(r.queued("c-night"), 1, "a plugin's send waits");
    std::fs::write(&settings, json!({ "start": 0, "end": 0 }).to_string()).unwrap();
    r.core.doc_host.kick_drains();
    until("morning", || r.replied("c-night", "ack: theirs")).await;

    std::fs::write(
        &settings,
        json!({ "start": hour, "end": (hour + 2) % 24 }).to_string(),
    )
    .unwrap();
    r.queue("c-night", "queued");
    r.run("c-night", "mine");
    until("the person's own send goes", || {
        r.replied("c-night", "ack: mine")
    })
    .await;
    assert_eq!(r.queued("c-night"), 1, "the queued row still waits");
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_safe_commands_example_allows_reads_refuses_wipes_and_asks_the_rest() {
    let r = rig();
    r.example("safe-commands").await;
    for chat in ["c-read", "c-wipe", "c-chain", "c-line", "c-pre"] {
        r.chat(chat);
    }
    r.run("c-read", "ask:git status");
    r.run("c-wipe", "ask:rm -rf /");
    r.run("c-chain", "ask:git status; rm -r src");
    r.run("c-line", "ask:ls x\nrm -r src");
    r.run("c-pre", "ask:rg --pre=sh pattern .");
    until("a read is allowed", || {
        r.replied("c-read", "ran git status")
    })
    .await;
    until("a wipe is refused", || {
        r.replied("c-wipe", "refused: denied by Safe commands")
    })
    .await;
    until("the rest asks", || r.asked_person("c-chain")).await;
    until("a second line asks", || r.asked_person("c-line")).await;
    until("a flag that runs a program asks", || r.asked_person("c-pre")).await;
    r.core.shutdown().await;
}

fn plan(id: &str, hooks: &[&str], allowed: &[&str]) -> service::Plan {
    service::Plan {
        plugin: id.into(),
        name: id.into(),
        version: "1.0.0".into(),
        hash: format!("hash-{id}"),
        dir: Default::default(),
        entry: "service.luau".into(),
        allowed: allowed.iter().map(|s| s.to_string()).collect(),
        commands: Vec::new(),
        hosts: Vec::new(),
        events: Vec::new(),
        hooks: hooks.iter().map(|s| s.to_string()).collect(),
        settings: Default::default(),
    }
}

#[test]
fn only_a_hook_its_permission_allowed_decides_in_install_order() {
    let dir = tempfile::tempdir().unwrap();
    let source = |installed_at: i64| {
        json!({ "kind": "file", "version": "1.0.0", "installed_at": installed_at }).to_string()
    };
    for (id, at) in [("z-early", 100), ("a-late", 200)] {
        std::fs::create_dir_all(dir.path().join(id)).unwrap();
        std::fs::write(
            dir.path()
                .join(id)
                .join(crate::plugins::install::SOURCE_FILE),
            source(at),
        )
        .unwrap();
    }
    let plans = [
        plan("a-late", &[DISPATCH], &["service", "hooks.dispatch"]),
        plan("m-by-hand", &[DISPATCH], &["service", "hooks.dispatch"]),
        plan(
            "z-early",
            &[DISPATCH, APPROVE],
            &["service", "hooks.dispatch"],
        ),
        plan("no-hooks", &[], &["service", "hooks.dispatch"]),
    ];
    let ordered = in_install_order(dir.path(), &plans);
    let ids: Vec<&str> = ordered.iter().map(|p| p.plugin.as_str()).collect();
    assert_eq!(ids, ["m-by-hand", "z-early", "a-late"]);
    let dispatching: Vec<String> = deciding(&ordered, DISPATCH)
        .into_iter()
        .map(|p| p.plugin)
        .collect();
    assert_eq!(dispatching, ["m-by-hand", "z-early", "a-late"]);
    assert!(
        deciding(&ordered, APPROVE).is_empty(),
        "z-early lists tool.approve but wasn't allowed hooks.approve"
    );
}
