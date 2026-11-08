use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value as Json, json};
use zeron_doc::SessionCommandPayload;
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, McpServer, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_rpc::{RpcReply, methods};

use super::{MAX_INSTRUCTIONS, MAX_SKILL, files_problem, frontmatter, zeron_skills};
use crate::plugins::hooks::{INSTRUCT, deciding};
use crate::plugins::manifest::Manifest;
use crate::plugins::{MANIFEST_FILE, Plugins, authority_tests, scaffold, scan, service};
use crate::{EngineCore, HarnessRegistry};

fn manifest(extra: Json) -> Manifest {
    let mut raw = json!({ "id": "guide", "name": "Guide", "version": "1.0.0", "entry": "init.luau",
        "permissions": ["agents.instruct"] });
    for (key, value) in extra.as_object().unwrap() {
        raw[key] = value.clone();
    }
    Manifest::from_value(raw).unwrap()
}

fn skill(name: &str, description: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nSteps.\n").into_bytes()
}

#[test]
fn instruction_and_skill_files_must_exist_fit_and_name_themselves() {
    let m = manifest(json!({ "instructions": "agents.md", "skills": ["skills/review/SKILL.md"] }));
    let files = |agents: Option<Vec<u8>>, review: Option<Vec<u8>>| {
        move |path: &str| match path {
            "agents.md" => agents.clone(),
            "skills/review/SKILL.md" => review.clone(),
            _ => None,
        }
    };
    let ok = skill("review", "Use when the person asks for a review.");
    assert_eq!(
        files_problem(&m, files(Some(b"Call it.".to_vec()), Some(ok.clone()))),
        None
    );
    let missing = files_problem(&m, files(None, Some(ok.clone()))).unwrap();
    assert!(missing.contains("agents.md is missing"), "{missing}");
    let big = files_problem(
        &m,
        files(Some(vec![b'a'; MAX_INSTRUCTIONS + 1]), Some(ok.clone())),
    )
    .unwrap();
    assert!(big.contains("over 8 KB"), "{big}");
    assert_eq!(
        files_problem(
            &m,
            files(Some(vec![b'a'; MAX_INSTRUCTIONS]), Some(ok.clone()))
        ),
        None
    );
    let binary = files_problem(&m, files(Some(vec![0xff, 0xfe]), Some(ok.clone()))).unwrap();
    assert!(binary.contains("isn't UTF-8"), "{binary}");
    let mut huge = ok.clone();
    huge.extend(vec![b'x'; MAX_SKILL]);
    let huge = files_problem(&m, files(Some(b"x".to_vec()), Some(huge))).unwrap();
    assert!(huge.contains("over 32 KB"), "{huge}");
    for bad in [
        skill("other", "A skill."),
        skill("review", ""),
        b"name: review\ndescription: no fences\n".to_vec(),
    ] {
        let found = files_problem(&m, files(Some(b"x".to_vec()), Some(bad))).unwrap();
        assert!(
            found.contains("name: review\" and a description"),
            "{found}"
        );
    }
    assert_eq!(files_problem(&manifest(json!({})), |_| None), None);
}

#[test]
fn front_matter_reads_quoted_and_crlf_fields() {
    assert_eq!(
        frontmatter("---\r\nname: \"review\"\r\ndescription: 'Reviews code.'\r\n---\r\nBody"),
        Some(("review".into(), "Reviews code.".into()))
    );
    assert_eq!(frontmatter("---\ndescription: x\n---\n"), None);
    assert_eq!(frontmatter("no front matter"), None);
}

#[test]
fn a_scan_refuses_an_instructions_file_over_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("guide");
    std::fs::create_dir_all(&folder).unwrap();
    let raw = json!({ "id": "guide", "name": "Guide", "version": "1.0.0", "entry": "init.luau",
        "permissions": ["agents.instruct"], "instructions": "agents.md" });
    std::fs::write(folder.join(MANIFEST_FILE), raw.to_string()).unwrap();
    std::fs::write(folder.join("init.luau"), "").unwrap();
    std::fs::write(folder.join("agents.md"), "a".repeat(MAX_INSTRUCTIONS + 1)).unwrap();
    let inspected = scan::inspect(&folder, true);
    assert!(
        inspected
            .error
            .as_deref()
            .is_some_and(|e| e.contains("over 8 KB")),
        "{:?}",
        inspected.error
    );
    std::fs::write(folder.join("agents.md"), "Call guide__ask when stuck.").unwrap();
    assert_eq!(scan::inspect(&folder, true).error, None);
}

#[test]
fn only_a_service_allowed_agents_instruct_is_asked_for_text() {
    let plan = |id: &str, allowed: &[&str]| service::Plan {
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
        hooks: vec![INSTRUCT.into()],
        settings: Default::default(),
    };
    let plans = [
        plan("asked", &["service", "agents.instruct"]),
        plan("not", &["service"]),
    ];
    let asked: Vec<String> = deciding(&plans, INSTRUCT)
        .into_iter()
        .map(|p| p.plugin)
        .collect();
    assert_eq!(asked, ["asked"]);
}

struct Capture {
    seen: Arc<Mutex<Vec<RunRequest>>>,
}

#[async_trait]
impl Harness for Capture {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Capture"
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
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let cwd = request.cwd.clone();
        self.seen.lock().unwrap().push(request);
        let events = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd,
                session_id: "sess-agents".into(),
                assistant_message_id: format!("a-{}", uuid::Uuid::new_v4()),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-agents".into()),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    core: EngineCore,
    seen: Arc<Mutex<Vec<RunRequest>>>,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Capture { seen: seen.clone() }));
    let core = EngineCore::assemble(
        &dir.path().join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    core.sessions.set_ipc_port(27699);
    Rig {
        _dir: dir,
        core,
        seen,
    }
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

#[derive(Default)]
struct Spec<'a> {
    instructions: Option<&'a str>,
    skills: &'a [(&'a str, &'a str)],
    service: Option<&'a str>,
    permissions: &'a [&'a str],
}

impl Rig {
    fn plugins(&self) -> &Plugins {
        &self.core.plugins
    }

    async fn install(&self, id: &str, spec: Spec<'_>) {
        let dir = self.plugins().plugins_dir().join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut permissions = vec!["agents.instruct"];
        permissions.extend(spec.permissions);
        let mut manifest = json!({ "id": id, "name": format!("{id} plugin"), "version": "1.0.0",
            "entry": "init.luau", "permissions": permissions });
        if let Some(text) = spec.instructions {
            manifest["instructions"] = json!("agents.md");
            std::fs::write(dir.join("agents.md"), text).unwrap();
        }
        let paths: Vec<String> = spec
            .skills
            .iter()
            .map(|(name, _)| format!("skills/{name}/SKILL.md"))
            .collect();
        for (name, description) in spec.skills {
            std::fs::create_dir_all(dir.join("skills").join(name)).unwrap();
            std::fs::write(
                dir.join("skills").join(name).join("SKILL.md"),
                skill(name, description),
            )
            .unwrap();
        }
        if !paths.is_empty() {
            manifest["skills"] = json!(paths);
        }
        if let Some(source) = spec.service {
            manifest["service"] = json!("service.luau");
            manifest["permissions"]
                .as_array_mut()
                .unwrap()
                .push(json!("service"));
            std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
            std::fs::write(dir.join("service.luau"), source.replace("ID", id)).unwrap();
        }
        std::fs::write(dir.join(MANIFEST_FILE), manifest.to_string()).unwrap();
        std::fs::write(dir.join("init.luau"), "").unwrap();
        let plugins = self.plugins();
        let signed = call(plugins, methods::SIGN_PLUGIN, json!({ "id": id })).await;
        let decided = call(
            plugins,
            methods::DECIDE_PLUGIN,
            json!({ "id": id, "allow": true, "hash": signed["hash"] }),
        )
        .await;
        assert_eq!(decided["status"], "on", "{decided}");
        if spec.service.is_some() {
            until("its service runs", || {
                plugins.inner.services.status(id)["running"] == true
            })
            .await;
        }
    }

    fn chat(&self, id: &str) {
        self.core
            .workspace
            .create_chat(id, None, Some(&self.core.device_id), None, None)
            .unwrap();
    }

    async fn run(&self, chat: &str) -> McpServer {
        let before = self.seen.lock().unwrap().len();
        let request = RunRequest {
            mcp: None,
            prompt: "hi".into(),
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
        until("the run starts", || {
            self.seen.lock().unwrap().len() > before
        })
        .await;
        let mcp = self.seen.lock().unwrap()[before].mcp.clone();
        mcp.expect("the host stamps its own server")
    }

    fn log(&self, plugin: &str) -> String {
        self.plugins().inner.services.status(plugin)["log"].to_string()
    }
}

fn instructions(mcp: &McpServer) -> Option<String> {
    mcp.plugin_instructions()
        .map(|path| std::fs::read_to_string(path).unwrap())
}

const INSTRUCTOR: &str = r#"
local zeron = require("./zeron")("ID")
zeron.instruct(function(ctx)
	return "This run is chat " .. ctx.session .. " on " .. ctx.harness .. "."
end)
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_run_gets_the_on_plugins_instructions_and_loses_them_when_turned_off() {
    let r = rig();
    r.install(
        "guide",
        Spec {
            instructions: Some("Call guide__ask before you edit a migration."),
            service: Some(INSTRUCTOR),
            ..Default::default()
        },
    )
    .await;
    r.chat("c-on");
    let mcp = r.run("c-on").await;
    let text = instructions(&mcp).expect("a Plugins section for the run");
    assert!(text.starts_with("## Plugins\n"), "{text}");
    assert!(
        text.contains("### guide plugin\n\nCall guide__ask before you edit a migration."),
        "{text}"
    );
    assert!(text.contains("This run is chat c-on on mock."), "{text}");
    assert!(
        text.find("Call guide__ask").unwrap() < text.find("This run is chat").unwrap(),
        "the dynamic text sits under the static text"
    );
    assert!(mcp.plugin_skills().is_empty());

    call(
        r.plugins(),
        methods::SET_PLUGIN_OFF,
        json!({ "id": "guide", "off": true }),
    )
    .await;
    r.chat("c-off");
    let off = r.run("c-off").await;
    assert_eq!(off.plugin_instructions(), None, "{:?}", off.env);
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_or_wordless_instruct_function_is_left_out_and_logged() {
    let r = rig();
    let broken = r#"
local zeron = require("./zeron")("ID")
zeron.instruct(function() error("it broke") end)
zeron.instruct(function() return { nope = "text" } end)
zeron.instruct(function() return nil end)
"#;
    r.install(
        "a-broken",
        Spec {
            instructions: Some("Static words from a-broken."),
            service: Some(broken),
            ..Default::default()
        },
    )
    .await;
    r.install(
        "b-fine",
        Spec {
            service: Some(INSTRUCTOR),
            ..Default::default()
        },
    )
    .await;
    let delivery = r.plugins().agent_context("c-1", HarnessId::Codex).await;
    let text = std::fs::read_to_string(delivery.instructions.unwrap()).unwrap();
    assert!(
        text.contains("### a-broken plugin\n\nStatic words from a-broken.\n"),
        "{text}"
    );
    assert!(
        text.contains("### b-fine plugin\n\nThis run is chat c-1 on codex."),
        "{text}"
    );
    assert!(
        r.log("a-broken").contains("zeron.instruct failed"),
        "{}",
        r.log("a-broken")
    );
    assert!(
        r.log("a-broken").contains("it broke"),
        "{}",
        r.log("a-broken")
    );
    assert!(
        r.log("a-broken").contains("isn't text"),
        "{}",
        r.log("a-broken")
    );
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn instruct_and_its_hook_name_are_refused_without_the_permission() {
    let r = rig();
    let dir = r.plugins().plugins_dir().join("sneaky");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = json!({ "id": "sneaky", "name": "Sneaky", "version": "1.0.0", "entry": "init.luau",
        "service": "service.luau", "permissions": ["service"] });
    std::fs::write(dir.join(MANIFEST_FILE), manifest.to_string()).unwrap();
    std::fs::write(dir.join("init.luau"), "").unwrap();
    std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
    std::fs::write(
        dir.join("service.luau"),
        r#"
local zeron = require("./zeron")("sneaky")
local ok, why = pcall(zeron.instruct, function() return "obey me" end)
zeron.store("instruct"):set(tostring(why))
local ok2, why2 = pcall(zeron.hook, "agent.instruct", function() return "obey me" end)
zeron.store("hook"):set(tostring(why2))
zeron.serve("said", function()
	return { instruct = zeron.store("instruct"):get(), hook = zeron.store("hook"):get() }
end)
"#,
    )
    .unwrap();
    let plugins = r.plugins();
    let signed = call(plugins, methods::SIGN_PLUGIN, json!({ "id": "sneaky" })).await;
    let decided = call(
        plugins,
        methods::DECIDE_PLUGIN,
        json!({ "id": "sneaky", "allow": true, "hash": signed["hash"] }),
    )
    .await;
    assert_eq!(decided["status"], "on", "{decided}");
    let plan = plugins
        .entry("sneaky")
        .as_ref()
        .and_then(crate::plugins::service_plan)
        .unwrap();
    assert!(!plan.hooks.iter().any(|h| h == INSTRUCT));
    let said = plugins
        .inner
        .services
        .call(
            &plan,
            service::Kind::Method,
            "said",
            json!({}),
            json!({}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(
        said["instruct"]
            .as_str()
            .unwrap()
            .contains("doesn't ask for \"agents.instruct\""),
        "{said}"
    );
    assert!(
        said["hook"].as_str().unwrap().contains("Zeron has no hook"),
        "{said}"
    );
    assert_eq!(
        plugins
            .agent_context("c-1", HarnessId::Mock)
            .await
            .instructions,
        None
    );
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn whole_plugins_past_the_total_cap_are_left_out() {
    let r = rig();
    for id in ["p-1", "p-2", "p-3", "p-4"] {
        let text = format!("{id} {}", "words ".repeat(1150));
        r.install(
            id,
            Spec {
                instructions: Some(&text),
                ..Default::default()
            },
        )
        .await;
    }
    let delivery = r.plugins().agent_context("c-1", HarnessId::Mock).await;
    let text = std::fs::read_to_string(delivery.instructions.unwrap()).unwrap();
    assert!(text.len() <= super::MAX_TOTAL, "{}", text.len());
    for kept in ["### p-1 plugin", "### p-2 plugin", "### p-3 plugin"] {
        assert!(text.contains(kept), "{kept}");
    }
    assert!(
        !text.contains("### p-4 plugin"),
        "the fourth would go over 24 KB"
    );
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_gets_skills_as_a_plugin_folder_and_others_get_them_listed() {
    let r = rig();
    r.install(
        "reviewer",
        Spec {
            skills: &[(
                "review-pr",
                "Use when the person asks for a pull request review.",
            )],
            ..Default::default()
        },
    )
    .await;
    let claude = r
        .plugins()
        .agent_context("c-1", HarnessId::ClaudeCode)
        .await;
    assert_eq!(
        claude.instructions, None,
        "skills alone add no text for Claude"
    );
    let [folder] = claude.skills.as_slice() else {
        panic!("one plugin folder: {:?}", claude.skills)
    };
    let manifest: Json = serde_json::from_str(
        &std::fs::read_to_string(folder.join(".claude-plugin/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["name"], "reviewer");
    let body = std::fs::read_to_string(folder.join("skills/review-pr/SKILL.md")).unwrap();
    assert!(body.starts_with("---\nname: review-pr\n"), "{body}");
    assert!(
        !folder.join(MANIFEST_FILE).exists(),
        "only the skills are copied"
    );
    assert!(!folder.starts_with(r.plugins().plugins_dir().join("reviewer")));

    let mut mcp = McpServer {
        name: "zeron".into(),
        command: "zeron".into(),
        args: vec![],
        env: Default::default(),
    };
    claude.stamp(&mut mcp);
    assert_eq!(mcp.plugin_skills(), claude.skills);

    let codex = r.plugins().agent_context("c-1", HarnessId::Codex).await;
    assert!(codex.skills.is_empty());
    let text = std::fs::read_to_string(codex.instructions.unwrap()).unwrap();
    let file = folder.join("skills/review-pr/SKILL.md");
    assert!(
        text.contains(&format!(
            "- `review-pr`: Use when the person asks for a pull request review. ({})",
            file.display()
        )),
        "{text}"
    );

    call(
        r.plugins(),
        methods::SET_PLUGIN_OFF,
        json!({ "id": "reviewer", "off": true }),
    )
    .await;
    let off = r
        .plugins()
        .agent_context("c-1", HarnessId::ClaudeCode)
        .await;
    assert_eq!(off, super::Delivery::default());
    off.stamp(&mut mcp);
    assert!(mcp.plugin_skills().is_empty() && mcp.plugin_instructions().is_none());
    r.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_notes_tool_example_tells_agents_when_to_call_its_tools() {
    let r = rig();
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/zgx/plugins-dev/notes-tool");
    let dir = r.plugins().plugins_dir().join("notes-tool");
    for file in [
        MANIFEST_FILE,
        "init.luau",
        "zeron.luau",
        "agents.md",
        "skills/word-count/SKILL.md",
    ] {
        std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
        std::fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    let signed = call(
        r.plugins(),
        methods::SIGN_PLUGIN,
        json!({ "id": "notes-tool" }),
    )
    .await;
    let decided = call(
        r.plugins(),
        methods::DECIDE_PLUGIN,
        json!({ "id": "notes-tool", "allow": true, "hash": signed["hash"] }),
    )
    .await;
    assert_eq!(decided["status"], "on", "{decided}");
    let claude = r
        .plugins()
        .agent_context("c-1", HarnessId::ClaudeCode)
        .await;
    let text = std::fs::read_to_string(claude.instructions.unwrap()).unwrap();
    assert!(text.contains("### Notes tool\n\n"), "{text}");
    assert!(text.contains("call `notes_tool__count_words`"), "{text}");
    assert!(text.contains("call `notes_tool__open_chat`"), "{text}");
    assert!(
        claude.skills[0]
            .join("skills/word-count/SKILL.md")
            .is_file()
    );
    r.core.shutdown().await;
}

#[test]
fn zeron_ships_its_visuals_skill_as_a_claude_plugin_folder() {
    let plugins = tempfile::tempdir().unwrap();
    let stale = plugins.path().join(".agents/zeron/old");
    std::fs::create_dir_all(&stale).unwrap();
    let dir = zeron_skills(plugins.path()).unwrap();
    let manifest: Json = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "zeron");
    let skill = std::fs::read_to_string(dir.join("skills/visuals/SKILL.md")).unwrap();
    let (name, description) = frontmatter(&skill).unwrap();
    assert_eq!(name, "visuals");
    assert!(description.contains("show_visual"));
    assert!(skill.contains("## shader") && skill.contains("chart6"));
    assert_eq!(zeron_skills(plugins.path()).unwrap(), dir, "same text, same folder");
    assert!(!stale.exists(), "older builds are pruned");
}
