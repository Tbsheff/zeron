use std::sync::Arc;

use serde_json::{Value, json};
use zeron_doc::{OrchBody, OrchMessage, SessionDoc};
use zeron_orch::{Config, DelEvent, Event, Mode, RunEvent, ThreadId};
use zeron_sync::DocsStore;

use super::{Driver, DriverError, FaultPoint};

fn fixture(device: &str) -> (tempfile::TempDir, Arc<DocsStore>, Driver) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let driver = Driver::open_with_capture(store.clone(), device, Config::default(), true).unwrap();
    (dir, store, driver)
}

fn rows(driver: &Driver) -> Vec<Value> {
    driver
        .mechanism_lines()
        .unwrap()
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn last(driver: &Driver) -> Value {
    rows(driver).pop().unwrap()
}

fn open(driver: &Driver, id: u32) {
    driver
        .apply(
            &format!("open:{id}"),
            Event::Del(DelEvent::OpenRoot {
                t: ThreadId(id),
                access: 2,
                reconfigure: false,
            }),
        )
        .unwrap();
}

fn queue(id: u32, msg: u64) -> Event {
    Event::Run(RunEvent::PersonSend {
        t: ThreadId(id),
        mode: Mode::Queue,
        msg,
    })
}

fn export(name: &str, drivers: &[&Driver]) {
    let Some(root) = std::env::var_os("ORCH_MECHANISM_OUT") else {
        return;
    };
    let dir =
        std::path::PathBuf::from(root).join(format!("mechanism-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files = Vec::new();
    let mut devices = Vec::new();
    for driver in drivers {
        let file = format!("{}.jsonl", driver.device());
        std::fs::write(
            dir.join(&file),
            driver.mechanism_lines().unwrap().join("\n") + "\n",
        )
        .unwrap();
        files.push(file);
        devices.push(driver.device());
    }
    std::fs::write(
        dir.join("capture.json"),
        serde_json::to_vec(&json!({
            "test_name":format!("orchestrator::mechanism_tests::{name}"),
            "devices":devices,"filenames":files,"capture":"present",
            "scope":"unit","synthetic":true,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn capture_commits_receipt_outbox_completion_and_command_as_separate_rows() {
    let (_dir, store, driver) = fixture("dev-a");
    open(&driver, 1);
    driver.apply("person", queue(1, 1)).unwrap();
    driver
        .apply_command(
            "start",
            Some("cmd:start"),
            Event::Run(RunEvent::StartFromQueue { t: ThreadId(1) }),
        )
        .unwrap();
    let applied = last(&driver);
    assert_eq!(applied["action"], "Apply");
    assert_eq!(
        applied["snapshot"]["state"],
        serde_json::to_value(driver.state().unwrap()).unwrap()
    );
    assert!(
        applied["snapshot"]["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"] == "start")
    );
    assert!(!store.is_processed("cmd:start").unwrap());
    assert_eq!(
        applied["snapshot"]["outbox"],
        serde_json::to_value(driver.outbox().unwrap()).unwrap()
    );
    let before = applied["snapshot"].clone();
    let result = driver
        .apply_command(
            "start",
            Some("cmd:start"),
            Event::Run(RunEvent::StartFromQueue { t: ThreadId(1) }),
        )
        .unwrap();
    assert!(result.duplicate);
    assert_eq!(last(&driver)["args"]["duplicate"], true);
    assert_eq!(last(&driver)["snapshot"], before);
    let effect = driver
        .pending()
        .unwrap()
        .into_iter()
        .find(|row| row.effect_id.starts_with("start#"))
        .unwrap();
    driver.complete(&effect.effect_id).unwrap();
    assert_eq!(last(&driver)["action"], "CompleteEffect");
    assert!(
        last(&driver)["snapshot"]["outbox"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["effect_id"] == effect.effect_id && row["done"] == true)
    );
    assert!(!store.is_processed("cmd:start").unwrap());
    driver.mark_command("cmd:start").unwrap();
    assert_eq!(last(&driver)["action"], "MarkCommand");
    assert!(store.is_processed("cmd:start").unwrap());
    assert!(
        last(&driver)["snapshot"]["processed"]
            .as_array()
            .unwrap()
            .contains(&json!("cmd:start"))
    );
    let all = rows(&driver);
    for (index, row) in all.iter().enumerate() {
        assert_eq!(row["seq"], index as u64);
    }
    export(
        "capture_commits_receipt_outbox_completion_and_command_as_separate_rows",
        &[&driver],
    );
}

#[test]
fn capture_rolls_back_fp3_and_preserves_doc_fact_across_fp1_fp2_fp4_and_reopen() {
    let (dir, store, driver) = fixture("dev-a");
    open(&driver, 1);
    let doc = SessionDoc::init("chat").unwrap();
    let notice = OrchMessage {
        id: "notice:chat:1".into(),
        sender_chat: "chat".into(),
        target_chat: "parent".into(),
        sender_device: "dev-a".into(),
        body: OrchBody::Notice {
            run: 1,
            outcome: "completed".into(),
            summary: String::new(),
        },
    };
    doc.append_orch_out(&notice).unwrap();
    store
        .save_snapshot("chat", &doc.export_snapshot().unwrap())
        .unwrap();
    driver.observe_mechanism("doc:notice:chat:1", "durable_doc_fixture", json!({"wire_id":notice.id,"sender":"chat","target":"parent","device":"dev-a","target_host":"dev-b","room":"chat","upward":true,"body":{"type":"notice","run":1,"outcome":"completed"},"already_present":null})).unwrap();
    let before = driver.mechanism_lines().unwrap();
    let state = driver.state().unwrap();
    driver.crash_at(FaultPoint::Fp3ReceivedNoReceipt, 0);
    assert!(matches!(
        driver.apply("person", queue(1, 1)),
        Err(DriverError::Crashed(FaultPoint::Fp3ReceivedNoReceipt))
    ));
    assert_eq!(driver.mechanism_lines().unwrap(), before);
    assert_eq!(
        serde_json::to_value(driver.state().unwrap()).unwrap(),
        serde_json::to_value(state).unwrap()
    );
    assert!(!driver.has_receipt("person").unwrap());
    driver.crash_at(FaultPoint::Fp1StepCommittedEffectNotRun, 0);
    assert!(matches!(
        driver.apply("person", queue(1, 1)),
        Err(DriverError::Crashed(
            FaultPoint::Fp1StepCommittedEffectNotRun
        ))
    ));
    assert!(driver.has_receipt("person").unwrap());
    assert_eq!(driver.mechanism_lines().unwrap().len(), before.len() + 1);
    assert!(driver.apply("person", queue(1, 1)).unwrap().duplicate);
    driver
        .apply(
            "start",
            Event::Run(RunEvent::StartFromQueue { t: ThreadId(1) }),
        )
        .unwrap();
    let effect = driver
        .pending()
        .unwrap()
        .into_iter()
        .find(|row| row.effect_id.starts_with("start#"))
        .unwrap();
    let before_complete = driver.mechanism_lines().unwrap();
    driver.crash_at(FaultPoint::Fp2EffectRunNotMarked, 0);
    assert!(driver.complete(&effect.effect_id).is_err());
    assert_eq!(driver.mechanism_lines().unwrap(), before_complete);
    assert!(
        driver
            .pending()
            .unwrap()
            .iter()
            .any(|row| row.effect_id == effect.effect_id)
    );
    driver.complete(&effect.effect_id).unwrap();
    let before_mark = driver.mechanism_lines().unwrap();
    driver.crash_at(FaultPoint::Fp4ReceiptNotMarked, 0);
    assert!(driver.mark_command("cmd:person").is_err());
    assert_eq!(driver.mechanism_lines().unwrap(), before_mark);
    assert!(!store.is_processed("cmd:person").unwrap());
    driver.mark_command("cmd:person").unwrap();
    let saved = driver.mechanism_lines().unwrap();
    let facts = last(&driver)["snapshot"]["facts"].clone();
    drop(driver);
    drop(store);
    let reopened_store = Arc::new(DocsStore::open(dir.path()).unwrap());
    let reopened =
        Driver::open_with_capture(reopened_store.clone(), "dev-a", Config::default(), false)
            .unwrap();
    assert!(reopened.mechanism_capture_enabled());
    assert_eq!(
        &reopened.mechanism_lines().unwrap()[..saved.len()],
        saved.as_slice()
    );
    let recovery_rows = rows(&reopened);
    assert_eq!(recovery_rows[saved.len()]["action"], "RecoveryOpen");
    assert_eq!(recovery_rows[saved.len()]["snapshot"]["facts"], facts);
    assert_eq!(recovery_rows.len(), saved.len() + 2);
    assert_eq!(last(&reopened)["action"], "ObserveCatalog");
    assert_eq!(last(&reopened)["args"]["kind"], "transfer_catalog");
    assert_eq!(last(&reopened)["args"]["source"], "durable_snapshots");
    let recovered_facts = last(&reopened)["snapshot"]["facts"].clone();
    for fact in facts.as_array().unwrap() {
        assert!(recovered_facts.as_array().unwrap().contains(fact));
    }
    let recovered_raw = loro::LoroDoc::new();
    recovered_raw
        .import(&reopened_store.load_snapshot("chat").unwrap().unwrap())
        .unwrap();
    let current_doc = SessionDoc::from_doc(recovered_raw);
    let expected_catalog = json!({
        "scope":"durable_snapshots",
        "chats":[super::transfer::durable_fact(&current_doc, "chat").unwrap()],
        "unknown":[]
    });
    let added: Vec<_> = recovered_facts.as_array().unwrap().iter()
        .filter(|fact| !facts.as_array().unwrap().contains(fact)).collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["kind"], "transfer_catalog");
    assert_eq!(added[0]["data"], expected_catalog);
    assert!(reopened.has_receipt("person").unwrap());
    assert!(reopened_store.is_processed("cmd:person").unwrap());
    assert!(
        reopened
            .outbox()
            .unwrap()
            .iter()
            .any(|row| row.effect_id == effect.effect_id && row.done)
    );
    let recovered_doc = loro::LoroDoc::new();
    recovered_doc
        .import(&reopened_store.load_snapshot("chat").unwrap().unwrap())
        .unwrap();
    assert_eq!(
        SessionDoc::from_doc(recovered_doc).read_orch_out().unwrap(),
        vec![notice]
    );
    let before_conflict = reopened.mechanism_lines().unwrap();
    assert!(
        reopened
            .observe_mechanism("doc:notice:chat:1", "durable_doc_fixture", json!({"conflict":true}))
            .is_err()
    );
    assert_eq!(reopened.mechanism_lines().unwrap(), before_conflict);
    assert_eq!(last(&reopened)["snapshot"]["facts"], recovered_facts);
    export(
        "capture_rolls_back_fp3_and_preserves_doc_fact_across_fp1_fp2_fp4_and_reopen",
        &[&reopened],
    );
}

#[test]
fn capture_maps_stable_chats_without_alias_and_keeps_unknown_host_unknown() {
    let (_a_dir, _a_store, a) = fixture("dev-a");
    let (_b_dir, _b_store, b) = fixture("dev-b");
    a.put_meta("thread:1", "shared").unwrap();
    a.put_meta("remote-host:shared", "dev-a").unwrap();
    b.put_meta("thread:1", "other").unwrap();
    b.put_meta("remote-host:other", "dev-b").unwrap();
    b.put_meta("thread:9", "shared").unwrap();
    b.put_meta("remote-host:shared", "dev-a").unwrap();
    b.put_meta("thread:2", "unknown").unwrap();
    open(&a, 1);
    open(&b, 1);
    let a_threads = last(&a)["snapshot"]["threads"].as_array().unwrap().clone();
    let b_threads = last(&b)["snapshot"]["threads"].as_array().unwrap().clone();
    assert!(a_threads.iter().any(|row| row["local_thread"] == 1
        && row["chat"] == "shared"
        && row["host_known"] == true));
    assert!(
        b_threads
            .iter()
            .any(|row| row["local_thread"] == 1 && row["chat"] == "other")
    );
    assert!(
        b_threads.iter().any(|row| row["local_thread"] == 9
            && row["chat"] == "shared"
            && row["shadow"] == true)
    );
    assert!(b_threads.iter().any(|row| row["chat"] == "unknown"
        && row["host_known"] == false
        && row["host"].is_null()
        && row["shadow"].is_null()));
    export(
        "capture_maps_stable_chats_without_alias_and_keeps_unknown_host_unknown",
        &[&a, &b],
    );
}
