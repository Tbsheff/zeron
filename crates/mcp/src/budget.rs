use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use zeron_proto::agent_guidance::{self, CORE, Features, MCP_INSTRUCTIONS};

use crate::tools::{ListedTool, Tools};
use crate::zeron::{Origin, Zeron};

const BUDGET_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/budget.json");
const MCP_INSTRUCTIONS_MAX_CHARS: usize = 230;
const CORE_MAX: usize = 2_048;
const CODEX_ENTRY_MAX: usize = 4_000;
const SHOW_VISUAL_MAX: usize = 3_072;
const DESCRIPTION_MAX: usize = 1_024;
const OPENCODE_UPFRONT_MAX: usize = 12_700;
const OPENCODE_UPFRONT_ORCH_MAX: usize = 14_600;
const DELEGATE_TOOLS: [&str; 3] = ["delegate_task", "task_status", "task_cancel"];

fn bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        other => other.to_string().len(),
    }
}

async fn listed(origin: Origin) -> Vec<ListedTool> {
    let client = zeron_rpc::memory_client(Arc::new(NoEngine));
    let tools = Tools::new(Arc::new(Zeron::with_client(client, origin)));
    tools.list().into_iter().map(ListedTool::from).collect()
}

struct NoEngine;

#[async_trait::async_trait]
impl zeron_rpc::RpcService for NoEngine {
    async fn handle(&self, method: &str, _params: Value) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        Err(zeron_rpc::RpcError::UnknownMethod(method.into()))
    }
}

fn tool_bytes(tool: &ListedTool) -> usize {
    tool.name.len() + tool.title.as_deref().map_or(0, str::len) + tool.description.len() + bytes(&tool.input_schema)
}

fn upfront_bytes(tool: &ListedTool) -> usize {
    tool.name.len() + tool.description.len() + bytes(&tool.input_schema)
}

fn in_chat(waits: bool, orchestrator: bool) -> Origin {
    Origin {
        chat_id: Some("budget-chat".into()),
        waits_on_background: waits,
        orchestrator,
        ..Default::default()
    }
}

async fn measure() -> Value {
    let mut out = measure_with(false).await;
    let on = measure_with(true).await;
    let only = |key: &str| -> BTreeMap<String, Value> {
        on[key].as_object().unwrap().iter().filter(|(name, _)| DELEGATE_TOOLS.contains(&name.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    out["orchestrator"] = json!({
        "tools": only("tools"),
        "descriptions": only("descriptions"),
        "totals": on["totals"],
        "instructions": on["instructions"],
        "opencodeUpfront": on["opencodeUpfront"],
    });
    out
}

async fn measure_with(orchestrator: bool) -> Value {
    let all = listed(in_chat(true, orchestrator)).await;
    let opencode_tools = listed(in_chat(false, orchestrator)).await;
    let opencode_features = Features {
        visuals: true,
        waits: false,
        side_chats_only: true,
        delegation: orchestrator,
    };
    let per_tool: BTreeMap<String, usize> = all.iter().map(|t| (t.name.clone(), tool_bytes(t))).collect();
    let descriptions: BTreeMap<String, usize> = all.iter().map(|t| (t.name.clone(), t.description.len())).collect();
    let claude_features = Features { delegation: orchestrator, ..Features::all() };
    let codex = agent_guidance::codex_entries(Features { waits: false, ..claude_features }, None);
    json!({
        "tools": per_tool,
        "descriptions": descriptions,
        "totals": {
            "toolCount": all.len(),
            "tools": all.iter().map(tool_bytes).sum::<usize>(),
            "descriptions": all.iter().map(|t| t.description.len()).sum::<usize>(),
            "inputSchemas": all.iter().map(|t| bytes(&t.input_schema)).sum::<usize>(),
            "outputSchemas": all.iter().filter_map(|t| t.output_schema.as_ref()).map(bytes).sum::<usize>(),
        },
        "instructions": {
            "mcpInstructionsChars": MCP_INSTRUCTIONS.chars().count(),
            "core": CORE.text.len(),
            "claudeSystemPrompt": agent_guidance::text(claude_features).len(),
            "codexLargestEntry": codex.iter().map(|(_, v)| v.len()).max().unwrap_or(0),
            "codexTotal": codex.iter().map(|(_, v)| v.len()).sum::<usize>(),
            "opencodeSystem": agent_guidance::text(opencode_features).len(),
            "mcpFallback": MCP_INSTRUCTIONS.len() + 2 + agent_guidance::text(claude_features).len(),
        },
        "opencodeUpfront": opencode_tools.iter().map(upfront_bytes).sum::<usize>()
            + agent_guidance::text(opencode_features).len(),
    })
}

fn grew(path: &str, now: &Value, budget: &Value, out: &mut Vec<String>) {
    match (now, budget) {
        (Value::Object(now), Value::Object(budget)) => {
            for (key, value) in now {
                let child = format!("{path}.{key}");
                match budget.get(key) {
                    Some(limit) => grew(&child, value, limit, out),
                    None => out.push(format!("{child}: new ({value}); add it to budget.json")),
                }
            }
        }
        (Value::Number(now), Value::Number(limit)) if now.as_u64() > limit.as_u64() => {
            out.push(format!("{path}: {now} > budget {limit}"));
        }
        _ => {}
    }
}

#[tokio::test]
async fn agent_context_stays_inside_its_budget() {
    let now = measure().await;
    if std::env::var_os("ZERON_MCP_BUDGET_UPDATE").is_some() {
        std::fs::write(BUDGET_FILE, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
    }
    let instructions = &now["instructions"];
    let orchestrator = &now["orchestrator"];
    let hard = [
        ("MCP instructions chars", instructions["mcpInstructionsChars"].as_u64(), MCP_INSTRUCTIONS_MAX_CHARS),
        ("core guidance", instructions["core"].as_u64(), CORE_MAX),
        ("largest Codex entry", instructions["codexLargestEntry"].as_u64(), CODEX_ENTRY_MAX - 1),
        ("OpenCode upfront", now["opencodeUpfront"].as_u64(), OPENCODE_UPFRONT_MAX),
        ("largest Codex entry, orchestrator on", orchestrator["instructions"]["codexLargestEntry"].as_u64(), CODEX_ENTRY_MAX - 1),
        ("OpenCode upfront, orchestrator on", orchestrator["opencodeUpfront"].as_u64(), OPENCODE_UPFRONT_ORCH_MAX),
    ];
    for (what, measured, cap) in hard {
        assert!(measured.unwrap() as usize <= cap, "{what}: {measured:?} > {cap}");
    }
    assert_eq!(orchestrator["descriptions"].as_object().unwrap().len(), DELEGATE_TOOLS.len());
    let descriptions = now["descriptions"].as_object().unwrap().iter().chain(orchestrator["descriptions"].as_object().unwrap());
    for (name, size) in descriptions {
        let cap = if name == "show_visual" { SHOW_VISUAL_MAX } else { DESCRIPTION_MAX };
        assert!(size.as_u64().unwrap() as usize <= cap, "{name} description: {size} > {cap}");
    }
    let budget: Value = serde_json::from_str(&std::fs::read_to_string(BUDGET_FILE).unwrap()).unwrap();
    let mut growth = Vec::new();
    grew("", &now, &budget, &mut growth);
    assert!(
        growth.is_empty(),
        "agent context grew past crates/mcp/budget.json:\n{}\nShrink it, or raise the budget on purpose (ZERON_MCP_BUDGET_UPDATE=1) with evals that show the bytes pay off.",
        growth.join("\n")
    );
}

#[test]
fn growth_is_reported_per_key_and_new_keys_need_a_budget() {
    let mut out = Vec::new();
    grew("", &json!({ "a": 5, "b": { "c": 3 }, "d": 1 }), &json!({ "a": 5, "b": { "c": 2 } }), &mut out);
    assert_eq!(out, [".b.c: 3 > budget 2", ".d: new (1); add it to budget.json"]);
}
