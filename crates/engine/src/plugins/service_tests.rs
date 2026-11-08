//! Plugin services through the plugins module: the calls a window, a peer
//! and an agent make, and the reasons they're refused.

use super::*;
use serde_json::json;

const SERVICE: &str = r#"
local zeron = require("./zeron")("svc")
zeron.serve("hello", function(args, call)
	return { hi = args.name, from = call.from.id, device = zeron.device.id }
end, { safe = true })
zeron.serve("shout", function(args)
	zeron.emit("shouted", { word = args.word })
	return true
end)
zeron.tool { name = "count", run = function(args) return #args.text end }
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    plugins: Plugins,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let plugins = Plugins::new(dir.path(), "dev-1");
    std::fs::create_dir_all(plugins.plugins_dir()).unwrap();
    Fixture { _dir: dir, plugins }
}

async fn call(plugins: &Plugins, method: &str, params: serde_json::Value) -> serde_json::Value {
    // Allowing and signing as Zeron's consent window does: with a review.
    let params = super::authority_tests::as_consent_window(plugins, method, params).await;
    match plugins.dispatch(method, params).await {
        Ok(RpcReply::Value(value)) => value,
        Ok(RpcReply::Stream(_)) => panic!("{method} streamed"),
        Err(error) => json!({ "rpc_error": error.to_string() }),
    }
}

/// Plugin `svc`'s folder at `version`, signed as yours.
async fn write(plugins: &Plugins, version: &str, manifest: serde_json::Value, service: Option<&str>) {
    let dir = plugins.plugins_dir().join("svc");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("init.luau"), "return nil").unwrap();
    std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
    if let Some(service) = service {
        std::fs::write(dir.join("service.luau"), service).unwrap();
    }
    let mut full = json!({ "id": "svc", "name": "Svc", "version": version, "entry": "init.luau" });
    for (k, v) in manifest.as_object().unwrap() {
        full[k] = v.clone();
    }
    std::fs::write(dir.join(MANIFEST_FILE), signing::pretty(&full)).unwrap();
    let signed = call(plugins, methods::SIGN_PLUGIN, json!({ "id": "svc" })).await;
    assert!(signed.get("rpc_error").is_none(), "{signed}");
}

fn with_service(extra: &[&str]) -> serde_json::Value {
    let mut permissions = vec!["service", "tools"];
    permissions.extend_from_slice(extra);
    json!({
        "permissions": permissions,
        "service": "service.luau",
        "tools": [{ "name": "count", "description": "Count characters", "input": { "type": "object" } }],
    })
}

async fn allow(plugins: &Plugins) {
    let item = call(plugins, methods::LIST_PLUGINS, json!({})).await["plugins"][0].clone();
    let decided = call(plugins, methods::DECIDE_PLUGIN, json!({ "id": "svc", "allow": true, "hash": item["hash"] })).await;
    assert_eq!(decided["status"], "on", "{decided}");
}

async fn ask(plugins: &Plugins, method: &str, args: serde_json::Value) -> serde_json::Value {
    call(plugins, methods::CALL_PLUGIN_SERVICE, json!({ "plugin": "svc", "method": method, "args": args })).await
}

fn own_key(plugins: &Plugins) -> Option<String> {
    signing::load_key(&plugins.inner.data_dir).map(|k| k.public())
}

async fn serve(plugins: &Plugins, version: &str, method: &str) -> serde_json::Value {
    call(
        plugins,
        methods::SERVE_PLUGIN_CALL,
        // Signed by this device's key, as another device of yours would
        // carry it (an unsigned caller for a signed plugin is refused).
        json!({ "plugin": "svc", "version": version, "from": "dev-2", "method": method, "args": { "name": "Ada" }, "authorKey": own_key(plugins) }),
    )
    .await
}

#[test]
fn a_manifest_with_a_service_asks_for_it() {
    let parse = |v: serde_json::Value| manifest::Manifest::from_value(v).unwrap().problem().map(str::to_owned);
    let base = json!({ "id": "svc", "name": "Svc", "version": "1.0.0", "entry": "init.luau" });
    let with = |extra: serde_json::Value| {
        let mut m = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        m
    };
    assert_eq!(parse(with(json!({ "service": "service.luau", "permissions": ["service"] }))), None);
    assert!(parse(with(json!({ "service": "service.luau" }))).unwrap().contains("doesn't ask for \"service\""));
    assert!(parse(with(json!({ "permissions": ["service"] }))).unwrap().contains("has none"));
    for bad in ["../x.luau", "/abs.luau", "service.lua", ".hidden/s.luau", "a//b.luau"] {
        assert!(
            parse(with(json!({ "service": bad, "permissions": ["service"] }))).is_some(),
            "{bad}"
        );
    }
    assert!(parse(with(json!({ "service": 3, "permissions": ["service"] }))).is_some());
}

#[tokio::test]
async fn a_window_calls_its_service_here() {
    let f = fixture();
    write(&f.plugins, "1.0.0", with_service(&[]), Some(SERVICE)).await;
    // Yours, but asking for permissions: waiting until you allow it.
    let refused = ask(&f.plugins, "hello", json!({ "name": "Ada" })).await;
    assert_eq!(refused["reason"], reason::NOT_APPROVED, "{refused}");
    allow(&f.plugins).await;
    let answered = ask(&f.plugins, "hello", json!({ "name": "Ada" })).await;
    assert_eq!(answered, json!({ "ok": true, "value": { "hi": "Ada", "from": "dev-1", "device": "dev-1" } }));
    let status = call(&f.plugins, methods::PLUGIN_STATUS, json!({ "id": "svc" })).await;
    assert_eq!(status["service"]["running"], true, "{status}");
    assert_eq!(status["service"]["methods"]["hello"], true, "{status}");
}

#[tokio::test]
async fn an_agents_tool_is_answered_with_no_window_open() {
    let f = fixture();
    write(&f.plugins, "1.0.0", with_service(&[]), Some(SERVICE)).await;
    allow(&f.plugins).await;
    let tools = call(&f.plugins, methods::LIST_PLUGIN_TOOLS, json!({})).await;
    assert_eq!(tools["tools"][0]["open"], true, "{tools}");
    let counted = call(&f.plugins, methods::CALL_PLUGIN_TOOL, json!({ "plugin": "svc", "tool": "count", "args": { "text": "four" } })).await;
    assert_eq!(counted, json!({ "ok": true, "value": 4 }));
}

#[tokio::test]
async fn a_peer_is_refused_with_a_reason() {
    let f = fixture();
    // Not installed.
    let refused = serve(&f.plugins, "1.0.0", "hello").await;
    assert_eq!(refused["reason"], reason::NOT_INSTALLED, "{refused}");
    // Installed, not yet allowed here.
    write(&f.plugins, "1.0.0", with_service(&[]), Some(SERVICE)).await;
    assert_eq!(serve(&f.plugins, "1.0.0", "hello").await["reason"], reason::NOT_APPROVED);
    allow(&f.plugins).await;
    // Allowed: answered, and the caller's device is who called.
    let answered = serve(&f.plugins, "1.0.0", "hello").await;
    assert_eq!(answered["value"]["from"], "dev-2", "{answered}");
    // Another version.
    let refused = serve(&f.plugins, "2.0.0", "hello").await;
    assert_eq!(refused["reason"], reason::VERSION_MISMATCH, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("1.0.0"), "{refused}");
    // A method it doesn't serve, and one that's Zeron's.
    assert_eq!(serve(&f.plugins, "1.0.0", "nope").await["reason"], reason::UNKNOWN_METHOD);
    assert_eq!(serve(&f.plugins, "1.0.0", "@nope").await["reason"], reason::BAD_CALL);
    // Turned off here.
    call(&f.plugins, methods::SET_PLUGIN_OFF, json!({ "id": "svc", "off": true })).await;
    assert_eq!(serve(&f.plugins, "1.0.0", "hello").await["reason"], reason::OFF);
    call(&f.plugins, methods::SET_PLUGIN_OFF, json!({ "id": "svc", "off": false })).await;
    assert_eq!(serve(&f.plugins, "1.0.0", "hello").await["ok"], true);
}

#[tokio::test]
async fn a_version_without_a_service_says_so() {
    let f = fixture();
    write(&f.plugins, "1.0.0", json!({}), None).await;
    let refused = serve(&f.plugins, "1.0.0", "hello").await;
    assert_eq!(refused["reason"], reason::NO_SERVICE, "{refused}");
}

#[tokio::test]
async fn an_update_adding_a_service_asks_again() {
    let f = fixture();
    write(&f.plugins, "1.0.0", json!({ "permissions": ["tools"], "tools": [{ "name": "count" }] }), None).await;
    allow(&f.plugins).await;
    write(&f.plugins, "1.1.0", with_service(&[]), Some(SERVICE)).await;
    let item = call(&f.plugins, methods::LIST_PLUGINS, json!({})).await["plugins"][0].clone();
    assert_eq!(item["status"], "pending", "{item}");
    assert_eq!(item["ask"], json!(["service"]), "{item}");
    assert_eq!(item["service"], "service.luau");
    assert_eq!(ask(&f.plugins, "hello", json!({})).await["reason"], reason::NOT_APPROVED);
}

#[tokio::test]
async fn a_calling_device_must_be_one_of_yours() {
    let f = fixture();
    write(&f.plugins, "1.0.0", with_service(&[]), Some(SERVICE)).await;
    allow(&f.plugins).await;
    // No device links in this fixture: another device is unreachable.
    let refused = call(
        &f.plugins,
        methods::CALL_PLUGIN_SERVICE,
        json!({ "plugin": "svc", "method": "hello", "device": "dev-9" }),
    )
    .await;
    assert_eq!(refused["reason"], reason::OFFLINE, "{refused}");
    // And this device by its own id is a local call.
    let answered = call(
        &f.plugins,
        methods::CALL_PLUGIN_SERVICE,
        json!({ "plugin": "svc", "method": "hello", "device": "dev-1", "args": { "name": "me" } }),
    )
    .await;
    assert_eq!(answered["value"]["hi"], "me", "{answered}");
}

#[tokio::test]
async fn a_service_event_reaches_this_devices_windows() {
    let f = fixture();
    write(&f.plugins, "1.0.0", with_service(&[]), Some(SERVICE)).await;
    allow(&f.plugins).await;
    let mut channel = match f.plugins.dispatch(methods::WATCH_PLUGIN_CHANNEL, json!({})).await {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("no stream"),
    };
    assert_eq!(ask(&f.plugins, "shout", json!({ "word": "hey" })).await["ok"], true);
    let event = tokio::time::timeout(Duration::from_secs(5), channel.next()).await.unwrap().unwrap();
    assert_eq!(event["plugin"], "svc");
    assert_eq!(event["event"], "shouted");
    assert_eq!(event["here"], true);
    assert_eq!(event["data"], json!({ "word": "hey" }));
    // An event from a watched device arrives the same way (`PluginDeliver`).
    let delivered = call(
        &f.plugins,
        methods::PLUGIN_DELIVER,
        json!({ "plugin": "svc", "from": "dev-2", "event": "shouted", "data": { "word": "there" }, "author_key": own_key(&f.plugins) }),
    )
    .await;
    assert_eq!(delivered["outcome"], outcome::DELIVERED, "{delivered}");
    let event = tokio::time::timeout(Duration::from_secs(5), channel.next()).await.unwrap().unwrap();
    assert_eq!((event["event"].clone(), event["from"]["id"].clone()), (json!("shouted"), json!("dev-2")));
    // A peer's @watch is kept, so this device's events go there too.
    assert_eq!(serve(&f.plugins, "1.0.0", "@watch").await["ok"], true);
    assert!(lock(&f.plugins.inner.watchers)["svc"].contains_key("dev-2"));
}

/// plugins-dev/checkout-status, the guide's example: its service runs this
/// device's `git` for a window's call and for an agent's tool.
#[tokio::test]
#[cfg(unix)]
async fn the_checkout_status_example_answers_from_git() {
    let f = fixture();
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/zgx/plugins-dev/checkout-status");
    let dir = f.plugins.plugins_dir().join("checkout-status");
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["zeron-plugin.json", "init.luau", "service.luau"] {
        std::fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    std::fs::write(dir.join("zeron.luau"), scaffold::SDK).unwrap();
    let signed = call(&f.plugins, methods::SIGN_PLUGIN, json!({ "id": "checkout-status" })).await;
    let decided = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        json!({ "id": "checkout-status", "allow": true, "hash": signed["hash"] }),
    )
    .await;
    assert_eq!(decided["status"], "on", "{decided}");

    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ran = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
        assert!(ran.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&ran.stderr));
    };
    git(&["init", "-q", "-b", "trunk"]);
    std::fs::write(repo.path().join("a.txt"), "a").unwrap();
    std::fs::write(repo.path().join("b.txt"), "b").unwrap();
    let cwd = repo.path().display().to_string();

    let answered = call(
        &f.plugins,
        methods::CALL_PLUGIN_SERVICE,
        json!({ "plugin": "checkout-status", "method": "status", "args": { "cwd": cwd } }),
    )
    .await;
    assert_eq!(answered["value"]["branch"], "trunk", "{answered}");
    assert_eq!(answered["value"]["changes"], 2, "{answered}");
    let tool = call(
        &f.plugins,
        methods::CALL_PLUGIN_TOOL,
        json!({ "plugin": "checkout-status", "tool": "checkout_status", "args": { "path": cwd } }),
    )
    .await;
    assert_eq!(tool["value"]["changes"], 2, "{tool}");
    // Not a checkout: git's own words, as the plugin's failure.
    let refused = call(
        &f.plugins,
        methods::CALL_PLUGIN_SERVICE,
        json!({ "plugin": "checkout-status", "method": "status", "args": { "cwd": "/" } }),
    )
    .await;
    assert_eq!(refused["reason"], reason::FAILED, "{refused}");
    assert!(refused["message"].as_str().unwrap().starts_with("git:"), "{refused}");
}

async fn until(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never: {what}");
}

fn done(status: zeron_proto::DoneStatus, error: Option<&str>) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: error.map(str::to_owned),
        session_id: None,
    }
}

#[tokio::test]
async fn the_turn_notify_example_says_when_a_turn_ends() {
    let f = fixture();
    type Posted = Vec<(String, Option<String>)>;
    let posted: Arc<Mutex<Posted>> = Arc::default();
    let seen = posted.clone();
    f.plugins.set_notifier(move |_, title, _, session| {
        lock(&seen).push((title.to_owned(), session.map(str::to_owned)));
        "posted".into()
    });
    let example =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/zgx/plugins-dev/turn-notify");
    let dir = f.plugins.plugins_dir().join("turn-notify");
    std::fs::create_dir_all(&dir).unwrap();
    for file in [
        "zeron-plugin.json",
        "init.luau",
        "service.luau",
        "zeron.luau",
    ] {
        std::fs::copy(example.join(file), dir.join(file)).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("zeron.luau")).unwrap(),
        scaffold::SDK,
        "its zeron.luau is the SDK's"
    );
    let signed = call(
        &f.plugins,
        methods::SIGN_PLUGIN,
        json!({ "id": "turn-notify" }),
    )
    .await;
    let decided = call(
        &f.plugins,
        methods::DECIDE_PLUGIN,
        json!({ "id": "turn-notify", "allow": true, "hash": signed["hash"] }),
    )
    .await;
    assert_eq!(decided["status"], "on", "{decided}");
    until("listening", || {
        f.plugins.inner.listening.load(Ordering::SeqCst)
    })
    .await;

    f.plugins
        .hear_agent("c1", &done(zeron_proto::DoneStatus::Completed, None));
    f.plugins
        .hear_agent("c1", &done(zeron_proto::DoneStatus::Interrupted, None));
    f.plugins
        .hear_agent("c2", &done(zeron_proto::DoneStatus::Errored, Some("boom")));
    until("two notifications", || lock(&posted).len() == 2).await;
    assert_eq!(
        lock(&posted).clone(),
        vec![
            (
                "Turn notify: A chat finished".to_string(),
                Some("c1".to_string())
            ),
            (
                "Turn notify: A chat failed".to_string(),
                Some("c2".to_string())
            ),
        ]
    );
}

const RECORDS: &str = r#"
local zeron = require("./zeron")("svc")
local last = zeron.store("last", false)
zeron.on("turn.done", function(e) last:set(e) end)
zeron.serve("last", function() return last:get() end)
"#;

#[tokio::test]
async fn what_was_said_reaches_a_service_only_with_messages_read() {
    let f = fixture();
    let events = |extra: &[&str]| {
        let mut permissions = vec!["service", "sessions.read"];
        permissions.extend_from_slice(extra);
        json!({ "permissions": permissions, "service": "service.luau", "events": ["turn.done"] })
    };
    write(&f.plugins, "1.0.0", events(&[]), Some(RECORDS)).await;
    allow(&f.plugins).await;
    until("listening", || {
        f.plugins.inner.listening.load(Ordering::SeqCst)
    })
    .await;
    f.plugins.hear_agent(
        "c1",
        &done(zeron_proto::DoneStatus::Errored, Some("the secret plan")),
    );
    let mut tries = 0;
    let last = loop {
        tries += 1;
        assert!(tries < 200, "never heard");
        let last = ask(&f.plugins, "last", json!({})).await;
        if last["value"].is_object() {
            break last["value"].clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(last, json!({ "session": "c1", "status": "errored" }));

    write(
        &f.plugins,
        "1.0.1",
        events(&["messages.read"]),
        Some(RECORDS),
    )
    .await;
    allow(&f.plugins).await;
    let mut tries = 0;
    let said = loop {
        tries += 1;
        assert!(tries < 200, "never heard");
        f.plugins.hear_agent(
            "c2",
            &done(zeron_proto::DoneStatus::Errored, Some("the secret plan")),
        );
        let last = ask(&f.plugins, "last", json!({})).await;
        if last["value"]["session"] == "c2" {
            break last["value"].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        said,
        json!({ "session": "c2", "status": "errored", "error": "the secret plan" })
    );
}
