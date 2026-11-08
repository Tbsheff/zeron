use serde_json::{Value, json};
use zeron_rpc::{RpcError, methods};

use super::{Hints, ToolDef, Tools};
use crate::error::ToolError;

pub(super) const THEMES: &str = "themes";
pub(super) const NAMES: [&str; 1] = [THEMES];
const ACTIONS: [&str; 6] = ["list", "get", "create", "update", "delete", "use"];

const THEMES_DESCRIPTION: &str = "\
Themes Zeron's window wears. action list (editable: yours). get {id}: palettes in the shape create \
and update take. create {name, light?, dark? | vscode}. update {id, name?, light?, dark?, remove?}: \
yours only; copy a built-in one with create. delete {id}: yours. use {id, slot?}: wear it now. A \
palette: \"#rrggbb\" background, raised, text, muted, faint, accent, danger, warning, success, syntax \
(12); optional shell, card, terminal, ansi (16).";

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![ToolDef {
        name: THEMES,
        title: "Zeron themes",
        description: THEMES_DESCRIPTION,
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ACTIONS },
                "id": { "type": "string" },
                "name": { "type": "string" },
                "light": { "type": "object" },
                "dark": { "type": "object" },
                "vscode": { "type": ["object", "string"], "description": "A VS Code colour theme's JSON." },
                "remove": { "type": "array", "items": { "type": "string", "enum": ["light", "dark"] } },
                "slot": { "type": "string", "enum": ["light", "dark", "both"] }
            },
            "required": ["action"]
        }),
        output_schema: None,
        hints: Hints::WRITE.destructive(),
        always_load: false,
        example: r#"{"action":"use","id":"tokyo-night","slot":"dark"}"#,
    }]
}

fn plain(error: RpcError) -> ToolError {
    ToolError::from_rpc(&error, "This Zeron doesn't have theme tools yet; update Zeron to use them.")
}

fn action(args: &Value, allowed: &[&str]) -> Result<String, ToolError> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or_default();
    if allowed.contains(&action) {
        Ok(action.to_owned())
    } else {
        Err(ToolError::invalid(format!(
            "action must be one of: {}",
            allowed.join(", ")
        )))
    }
}

impl Tools {
    pub(super) async fn themes(&self, tool: &str, args: Value) -> Result<Value, ToolError> {
        let Value::Object(mut fields) = args else {
            return Err(ToolError::invalid("invalid arguments: expected an object"));
        };
        let _ = tool;
        let action = action(&Value::Object(fields.clone()), &ACTIONS)?;
        fields.remove("action");
        if let Some(Value::Array(schemes)) = fields.remove("remove") {
            if action != "update" {
                return Err(ToolError::invalid("remove works only with action update"));
            }
            for scheme in schemes.iter().filter_map(Value::as_str) {
                fields.insert(scheme.to_owned(), Value::Null);
            }
        }
        let method = match action.as_str() {
            "list" => methods::LIST_THEMES,
            "get" => methods::GET_THEME,
            "create" => methods::CREATE_THEME,
            "update" => methods::UPDATE_THEME,
            "delete" => methods::DELETE_THEME,
            _ => methods::USE_THEME,
        };
        let mut reply = self
            .zeron
            .call_rpc(method, Value::Object(fields))
            .await
            .map_err(plain)?;
        if method == methods::CREATE_THEME {
            reply["next"] = json!("To wear it, call themes {action: \"use\", id}.");
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeron::{Origin, Zeron};
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Arc;
    use zeron_rpc::{RpcReply, RpcService, memory_client};

    /// The engine's theme half, on a data dir of its own: the catalogue it
    /// answers from, as the engine does.
    struct Engine {
        dir: PathBuf,
    }

    #[async_trait]
    impl RpcService for Engine {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            use zeron_theme::catalog;
            let dir = &self.dir;
            let said = |e: anyhow::Error| RpcError::Failed(e.to_string());
            let text = |k: &str| params.get(k).and_then(Value::as_str);
            let need = || text("id").ok_or_else(|| RpcError::Failed("Say which theme".into()));
            let reply = match method {
                methods::LIST_THEMES => catalog::list(dir),
                methods::GET_THEME => catalog::get(dir, need()?).map_err(said)?,
                methods::CREATE_THEME => match params.get("vscode") {
                    Some(v) => catalog::create_from_vscode(dir, text("name"), v),
                    None => catalog::create(dir, text("name").unwrap_or(""), params.get("light"), params.get("dark")),
                }
                .map_err(said)?
                .to_json(),
                methods::UPDATE_THEME => catalog::update(dir, need()?, text("name"), params.get("light"), params.get("dark")).map_err(said)?.to_json(),
                methods::DELETE_THEME => {
                    catalog::delete(dir, need()?).map_err(said)?;
                    json!({ "deleted": need()? })
                }
                methods::USE_THEME => catalog::use_theme(dir, need()?, text("slot").unwrap_or("both")).map_err(said)?,
                other => return Err(RpcError::UnknownMethod(other.into())),
            };
            Ok(RpcReply::Value(reply))
        }
    }

    fn tools(dir: &std::path::Path) -> Tools {
        let engine = Arc::new(Engine { dir: dir.to_path_buf() });
        Tools::new(Arc::new(Zeron::with_client(memory_client(engine), Origin::default())))
    }

    fn palette(seed: &str) -> Value {
        let v = zeron_theme::builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == seed).unwrap();
        zeron_theme::catalog::Palette::of(v).to_json(None)
    }

    #[tokio::test]
    async fn a_theme_is_made_read_changed_worn_and_deleted() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let made = tools.call("themes", json!({ "action": "create", "name": "Ink", "light": palette("github-light"), "dark": palette("tokyo-night") })).await.unwrap();
        assert_eq!(made["id"], "custom-ink");
        assert!(made["next"].as_str().unwrap().contains("action: \"use\""));
        let got = tools.call("themes", json!({ "action": "get", "id": "custom-ink" })).await.unwrap();
        assert_eq!(got["dark"], palette("tokyo-night"), "as given");
        assert_eq!(got["editable"], true);
        // Read, changed, saved back.
        let mut dark = got["dark"].clone();
        dark["accent"] = json!("#ff3e9a");
        let changed = tools.call("themes", json!({ "action": "update", "id": "custom-ink", "name": "Ink Pink", "dark": dark })).await.unwrap();
        assert_eq!(changed["variants"]["dark"], "custom-ink-dark");
        let again = tools.call("themes", json!({ "action": "get", "id": "custom-ink" })).await.unwrap();
        assert_eq!((again["name"].as_str(), again["dark"]["accent"].as_str()), (Some("Ink Pink"), Some("#ff3e9a")));
        // Worn: list_themes says so.
        let used = tools.call("themes", json!({ "action": "use", "id": "custom-ink" })).await.unwrap();
        assert_eq!(used["chosen"], json!({ "light": "custom-ink-light", "dark": "custom-ink-dark" }));
        let listed = tools.call("themes", json!({ "action": "list" })).await.unwrap();
        let ink = listed["themes"].as_array().unwrap().iter().find(|t| t["id"] == "custom-ink").unwrap();
        assert_eq!(ink["in_use"], json!(["light", "dark"]));
        assert_eq!(listed["wearing"]["dark"]["variant"], "custom-ink-dark");
        // Gone, and its schemes back to the default.
        assert_eq!(tools.call("themes", json!({ "action": "delete", "id": "custom-ink" })).await.unwrap(), json!({ "deleted": "custom-ink" }));
        let listed = tools.call("themes", json!({ "action": "list" })).await.unwrap();
        assert_eq!(listed["wearing"]["dark"]["variant"], "zeron-dark");
    }

    #[tokio::test]
    async fn built_in_and_template_themes_are_read_only() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let gruvbox = tools.call("themes", json!({ "action": "get", "id": "gruvbox" })).await.unwrap();
        assert_eq!(gruvbox["editable"], false);
        assert!(gruvbox["light"].is_object() && gruvbox["dark"].is_object());
        for args in [
            json!({ "action": "update", "id": "gruvbox", "name": "Mine" }),
            json!({ "action": "delete", "id": "gruvbox" }),
        ] {
            let err = tools.call(THEMES, args.clone()).await.unwrap_err();
            assert!(err.message.contains("built in"), "{args}: {err}");
        }
        // A copy is yours.
        let copy = tools.call("themes", json!({ "action": "create", "name": "My Gruvbox", "light": gruvbox["light"], "dark": gruvbox["dark"] })).await.unwrap();
        assert_eq!(tools.call("themes", json!({ "action": "get", "id": copy["id"] })).await.unwrap()["editable"], true);
        let err = tools.call("themes", json!({ "action": "use", "id": "dracula", "slot": "light" })).await.unwrap_err();
        assert!(err.message.contains("no light version"), "{err}");
    }

    #[tokio::test]
    async fn a_bad_palette_says_what_to_fix() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let mut bad = palette("nord");
        bad["accent"] = json!("purple");
        bad["syntax"] = json!(["#ffffff", "#000000"]);
        let err = tools.call("themes", json!({ "action": "create", "name": "Bad", "dark": bad })).await.unwrap_err();
        assert!(err.message.contains("`dark.accent`: \"purple\" isn't a colour"), "{err}");
        assert!(err.message.contains("`dark.syntax` needs 12 colours"), "{err}");
        let err = tools.call("themes", json!({ "action": "create", "name": "Empty" })).await.unwrap_err();
        assert!(err.message.contains("`light`, `dark` or both"), "{err}");
        assert!(tools.call("themes", json!({ "action": "get", "id": "nope" })).await.unwrap_err().message.contains("no theme"));
        assert!(tools.call(THEMES, json!("x")).await.is_err());
        let removed = tools.call(THEMES, json!({ "action": "use", "id": "nord", "remove": ["light"] })).await.unwrap_err();
        assert!(removed.message.contains("only with action update"), "{removed}");
    }

    #[tokio::test]
    async fn a_vs_code_theme_comes_in_through_the_importer() {
        let data = tempfile::tempdir().unwrap();
        let tools = tools(data.path());
        let theme = json!({
            "name": "Paper Mill", "type": "light",
            "colors": { "editor.background": "#f7f3ea", "editor.foreground": "#2b2a27", "focusBorder": "#b5651d" },
            "tokenColors": [{ "scope": "keyword", "settings": { "foreground": "#8a3b12" } }],
        });
        let made = tools.call("themes", json!({ "action": "create", "vscode": theme })).await.unwrap();
        assert_eq!(made["name"], "Paper Mill");
        let got = tools.call("themes", json!({ "action": "get", "id": made["id"] })).await.unwrap();
        assert_eq!(got["light"]["background"], "#f7f3ea");
        assert_eq!(got["light"]["syntax"][1], "#8a3b12", "keywords from tokenColors");
    }

    #[test]
    fn the_theme_tool_says_what_a_palette_holds() {
        let defs = catalog();
        assert_eq!(defs.iter().map(|d| d.name).collect::<Vec<_>>(), NAMES);
        for field in ["background", "raised", "accent", "syntax (12)", "ansi (16)"] {
            assert!(defs[0].description.contains(field), "{field}");
        }
    }
}
