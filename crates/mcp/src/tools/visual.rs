use anyhow::Context;
use serde::Deserialize;
use serde_json::{Value, json};
use zeron_proto::visual_reference;

use super::{Hints, ToolDef, Tools};
use crate::error::ToolError;

pub(super) const SHOW_VISUAL: &str = "show_visual";
pub(super) const SHOW_IMAGE: &str = "show_image";
pub(super) const WAIT_ON_BACKGROUND: &str = "wait_on_background";
pub(super) const VISUAL_REFERENCE: &str = "visual_reference";

pub(super) const SHOW_VISUAL_DESCRIPTION: &str = "\
Show an interactive visual (chart, table, dashboard, diagram, 3D view) above your final reply, when \
it says more than prose. Do not restate it after. For an image file, use show_image.\n\
\n\
A visual is a Luau script, sandboxed (no I/O; seeded math.random), drawn with Zeron's native UI. Set up \
data and state at the top level, then return a view function; Zeron calls it again after each handler:\n\
\n\
local s = ui.state { metric = \"p95\" }\n\
local data = { { region = \"us-east\", p95 = 182, p99 = 240 }, { region = \"eu-west\", p95 = 140, p99 = 210 } }\n\
return function()\n\
  return ui.column { gap = 16,\n\
    ui.text { \"Latency by region\", size = \"lg\", weight = 600 },\n\
    ui.tabs { options = { \"p95\", \"p99\" }, value = s.metric, on_change = function(v) s.metric = v end },\n\
    ui.chart { type = \"bar\", height = 220, unit = \"ms\",\n\
      x = ui.map(data, function(d) return d.region end),\n\
      series = { { name = s.metric, values = ui.map(data, function(d) return d[s.metric] end) } } },\n\
  }\n\
end\n\
\n\
Zeron runs the script first and returns the first error with its line: fix it and call again.\n\
\n\
Props per node (common ones; unknown props are errors):\n\
- any node: width = px, grow = true (in a row; a boolean, not a number), key\n\
- column, row: gap, align; row also justify, wrap. grid: columns, gap. rule {}\n\
- text { \"str\", size = xs|sm|body|lg|xl|2xl, weight = 400-700, color, mono, italic, align, truncate }\n\
- stat { value, label, delta, tone }. table { columns, rows }\n\
- chart { type = bar|line|area|scatter, height, x, series = { { name, values, color } }, stacked, \
min, max, unit, prefix, decimals }\n\
- tabs { options, value, on_change }, toggle { label, value, on_change }, slider { label, value, \
min, max, step, unit, on_change }, button { label, on_click, primary }\n\
- canvas (2D), scene (3D), shader (GLSL): read visual_reference first.\n\
To ask the person, set the pick's state and call ui.answer(choice, { label = \"Pick\" }) in one \
handler: their draft keeps one \"Pick: <choice>\" line per label, the latest pick; nothing sends.\n\
\n\
Colors are theme tokens, never hex (except 3D materials): text, muted, faint, accent, on-accent, \
success, warning, danger, border, chart1, chart2, chart3, chart4, chart5, chart6.\n\
\n\
Style, enforced: no fill, border, shadow, padding, or font props; no cards, boxes, banners, or \
backgrounds. Stats: a row of plain ui.stat. A chart: a plain heading, then the chart. Split sections \
with gap or one ui.rule. The column is 360-720px wide: use grow, wrap, grid; charts and scenes are \
160-420px tall.";

const SHOW_IMAGE_DESCRIPTION: &str = "\
Show an image file (screenshot, photo, rendered chart) above your final reply. Call it before the \
reply, once per image, in order. PNG, JPEG, WebP, or GIF; max 10 MB and 4096 px a side. Zeron copies \
the file. Do not describe it after. For a chart you build, use show_visual.";

const WAIT_ON_BACKGROUND_DESCRIPTION: &str = "\
Mark this chat as waiting on background work you started (a test run, a build), so it reads as \
waiting, not finished, after your turn; you are woken when it ends. Call it before you end the turn, \
with the task ids your tools reported. Not for work you leave running (a dev server, a watcher). \
Background agents are waited on already. waiting false takes a mark back. It does not stop the work.";

const VISUAL_REFERENCE_DESCRIPTION: &str = "\
Full show_visual API, one topic per call. Read-only. Use it when the show_visual summary or a check \
error is not enough. No topic: the topic list.";

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: SHOW_VISUAL,
            title: "Show visual",
            description: SHOW_VISUAL_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "script": { "type": "string" },
                    "assets": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Name the script uses -> file path. glb, PNG, JPEG, WebP, hdr; max 16 files, 10 MB."
                    }
                },
                "required": ["title", "script"]
            }),
            output_schema: None,
            hints: Hints::WRITE,
            always_load: false,
            example: r#"{"title":"Sales","script":"return function() return ui.column { ui.stat { value = \"42\", label = \"Deals\" } } end"}"#,
        },
        ToolDef {
            name: SHOW_IMAGE,
            title: "Show image",
            description: SHOW_IMAGE_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
            output_schema: None,
            hints: Hints::WRITE,
            always_load: false,
            example: r#"{"path":"/tmp/screenshot.png"}"#,
        },
        ToolDef {
            name: VISUAL_REFERENCE,
            title: "Visual reference",
            description: VISUAL_REFERENCE_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "enum": visual_reference::topics() }
                }
            }),
            output_schema: None,
            hints: Hints::READ,
            always_load: false,
            example: r#"{"topic":"data"}"#,
        },
        ToolDef {
            name: WAIT_ON_BACKGROUND,
            title: "Wait on background work",
            description: WAIT_ON_BACKGROUND_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_ids": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                    "waiting": { "type": "boolean" }
                },
                "required": ["task_ids"]
            }),
            output_schema: None,
            hints: Hints::WRITE.idempotent(),
            always_load: true,
            example: r#"{"task_ids":["bash_1"]}"#,
        },
    ]
}

#[derive(Deserialize)]
pub(super) struct ShowImageArgs {
    path: String,
}

#[derive(Deserialize)]
pub(super) struct WaitOnBackgroundArgs {
    task_ids: Vec<String>,
    #[serde(default)]
    waiting: Option<bool>,
}

#[derive(Deserialize)]
pub(super) struct ShowVisualArgs {
    script: String,
    title: String,
    #[serde(default)]
    assets: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
pub(super) struct VisualReferenceArgs {
    topic: Option<String>,
}

pub(super) fn reference(args: VisualReferenceArgs) -> Result<Value, ToolError> {
    let topics = visual_reference::topics();
    let Some(topic) = args.topic.filter(|t| !t.trim().is_empty()) else {
        return Ok(json!({ "topics": topics, "next": "Call visual_reference {topic} for one topic." }));
    };
    match visual_reference::section(&topic) {
        Some(body) => Ok(json!({ "topic": topic.trim().to_ascii_lowercase(), "reference": body })),
        None => Err(ToolError::invalid(format!(
            "unknown topic {topic:?}; topics: {}",
            topics.join(", ")
        ))),
    }
}

fn visual_hint(error: ToolError) -> ToolError {
    if !error.message.contains("visual script failed") {
        return error;
    }
    let message = visual_reference::check_error_with_hint(&error.message);
    ToolError::new(error.code, message)
}

impl Tools {
    fn in_chat(&self, tool: &str) -> anyhow::Result<String> {
        self.zeron.origin().chat_id.clone().with_context(|| {
            format!("{tool} only works from an agent Zeron launched in a chat")
        })
    }

    pub(super) async fn show_visual(&self, args: ShowVisualArgs) -> Result<Value, ToolError> {
        let shown = async {
            let chat_id = self.in_chat(SHOW_VISUAL)?;
            let assets = read_visual_assets(&args.assets).await?;
            self.zeron
                .call(
                    zeron_rpc::methods::PUBLISH_VISUAL,
                    json!({ "chatId": chat_id, "title": args.title, "script": args.script, "assets": assets }),
                )
                .await
        }
        .await
        .map_err(|e| visual_hint(ToolError::from_anyhow(&e)))?;
        Ok(json!({
            "visual": shown,
            "next": "Shown above your reply. Do not mention or describe it; reply with only what it does not say.",
        }))
    }

    pub(super) async fn show_image(&self, args: ShowImageArgs) -> anyhow::Result<Value> {
        use base64::Engine as _;
        let chat_id = self.in_chat(SHOW_IMAGE)?;
        let bytes = read_image(&args.path).await?;
        let name = std::path::Path::new(&args.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        let shown = self
            .zeron
            .call(
                zeron_rpc::methods::PUBLISH_IMAGE,
                json!({
                    "chatId": chat_id,
                    "name": name,
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
            )
            .await?;
        Ok(json!({
            "image": shown,
            "next": "Shown above your reply. Do not mention or describe it; reply with only what it does not show.",
        }))
    }

    pub(super) async fn wait_on_background(&self, args: WaitOnBackgroundArgs) -> anyhow::Result<Value> {
        let chat_id = self.in_chat(WAIT_ON_BACKGROUND)?;
        let waiting = args.waiting.unwrap_or(true);
        let marked = self
            .zeron
            .call(
                zeron_rpc::methods::AWAIT_BACKGROUND,
                json!({ "chatId": chat_id, "taskIds": args.task_ids, "waiting": waiting }),
            )
            .await?;
        let none_matched = marked["matched"].as_array().is_none_or(Vec::is_empty);
        let message = if none_matched {
            "None of those are running background tasks of yours now (they may have ended): nothing is waited on."
        } else if waiting {
            "This chat reads as waiting on that work after your turn, until it ends and you're woken."
        } else {
            "No longer waited on."
        };
        Ok(json!({
            "matched": marked["matched"],
            "unmatched": marked["unmatched"],
            "message": message,
        }))
    }
}

async fn read_visual_assets(
    assets: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<serde_json::Map<String, Value>> {
    use base64::Engine as _;
    use zeron_proto::{
        VISUAL_MAX_ASSET_BYTES as MAX_ASSET_BYTES, VISUAL_MAX_ASSETS as MAX_ASSETS,
        VISUAL_MAX_TOTAL_ASSET_BYTES as MAX_TOTAL_ASSET_BYTES,
        valid_visual_asset_name as valid_name,
    };
    anyhow::ensure!(
        assets.len() <= MAX_ASSETS,
        "a visual takes at most {MAX_ASSETS} assets"
    );
    let mut out = serde_json::Map::new();
    let mut total = 0;
    let mut seen = std::collections::HashSet::new();
    for (name, path) in assets {
        anyhow::ensure!(
            valid_name(name),
            "asset name '{name}' must be a plain file name (letters, digits, '.', '-', '_')"
        );
        anyhow::ensure!(
            seen.insert(name.to_ascii_lowercase()),
            "asset names must differ by more than case ('{name}')"
        );
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("asset '{name}': can't read {path}"))?;
        anyhow::ensure!(metadata.is_file(), "asset '{name}': {path} is not a file");
        anyhow::ensure!(
            metadata.len() <= MAX_ASSET_BYTES as u64,
            "asset '{name}' is over {} MB",
            MAX_ASSET_BYTES / (1024 * 1024)
        );
        let mut bytes = Vec::new();
        {
            use tokio::io::AsyncReadExt as _;
            tokio::fs::File::open(path)
                .await
                .with_context(|| format!("asset '{name}': can't read {path}"))?
                .take(MAX_ASSET_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .with_context(|| format!("asset '{name}': can't read {path}"))?;
        }
        anyhow::ensure!(
            bytes.len() <= MAX_ASSET_BYTES,
            "asset '{name}' is over {} MB",
            MAX_ASSET_BYTES / (1024 * 1024)
        );
        total += bytes.len();
        anyhow::ensure!(
            total <= MAX_TOTAL_ASSET_BYTES,
            "a visual's assets total at most {} MB",
            MAX_TOTAL_ASSET_BYTES / (1024 * 1024)
        );
        out.insert(
            name.clone(),
            Value::String(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
    }
    Ok(out)
}

async fn read_image(path: &str) -> anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    use zeron_proto::IMAGE_MAX_BYTES;
    let too_large = || anyhow::anyhow!("{path} is over {} MB", IMAGE_MAX_BYTES / (1024 * 1024));
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("can't read {path}"))?;
    anyhow::ensure!(metadata.is_file(), "{path} is not a file");
    if metadata.len() > IMAGE_MAX_BYTES as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await
        .with_context(|| format!("can't read {path}"))?
        .take(IMAGE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("can't read {path}"))?;
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}
