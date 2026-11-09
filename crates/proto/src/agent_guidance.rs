use crate::McpServer;

pub const MCP_INSTRUCTIONS: &str = "Zeron runs coding agents in chats on the person's devices: delegate to parallel chats, message them, collect replies, show visuals, build plugins. Tools load on demand; search for zeron tools by name.";

pub const CODEX_ENTRY_MAX: usize = 3_900;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Features {
    pub visuals: bool,
    pub waits: bool,
    pub side_chats_only: bool,
    pub delegation: bool,
}

impl Features {
    pub fn all() -> Self {
        Self {
            visuals: true,
            waits: true,
            side_chats_only: true,
            delegation: false,
        }
    }
}

pub fn orchestrator_on(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value.trim(), "1" | "true" | "on"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub key: &'static str,
    pub text: &'static str,
}

pub const CORE: Section = Section {
    key: "core",
    text: "\
## Zeron

Ordinary coding tasks need no Zeron tools. Zeron runs agent chats on the person's devices; its tools (server `zeron`) load on demand, so search for them by name.

- Parallel work: `create_chats` (1-32 chats, a prompt each, all start at once).
- Message other Zeron chats that exist: `message_chats`. Collect a reply: `wait_for_turn`.
- Status or transcript: `read_chat`. Find chats: `list_chats`. Devices, projects, harnesses, models: `catalog`.
- Chat asks a question: `respond_to_input`. Stop it: `interrupt_chat`. Tidy up: `archive_chat`.
- Zeron plugins, templates, themes: `plugins`, `templates`, `themes`.

Rules:
1. Start all independent chats in one `create_chats` call, then wait. Never wait on one before you start the next.
2. A chat you create is your subagent. A subagent cannot create chats.
3. Any `connected` harness in `catalog` takes work, whatever provider you are.
4. Name a chat by id, id prefix, or exact title. A chat cannot message itself.
5. Results are JSON with stable ids. When a result has `next`, do what it says.",
};

pub const WAITS: Section = Section {
    key: "waits",
    text: "\
## Background work

When you start background work you will come back to once it finishes (a test run, a build, a long check) and you end your turn before it finishes, call the Zeron tool `wait_on_background` with its task id first, so the person sees this chat waiting rather than finished. Never for work you leave running on purpose (a dev server, a watcher).",
};

pub const DELEGATION: Section = Section {
    key: "delegation",
    text: "\
## Delegated tasks

- Parallel or isolated work: `delegate_task`, one child per call. The child sees only `task`. Keep its `taskId`.
- Default mode `async`: results arrive as one task-results message in a later turn, so end your turn. Do not poll `task_status`.
- Mode `wait` only when this turn needs the answer.
- Limits: depth 2, 8 unsettled children. Stop a task: `task_cancel`.",
};

pub const SIDE_CHATS_ONLY: Section = Section {
    key: "side_chats",
    text: "\
## Delegation

Your own subagent tools are off in Zeron's settings. Where you would start a subagent, delegate with Zeron's `create_chats` instead.",
};

pub const VISUALS: Section = Section {
    key: "visuals",
    text: "\
## Visuals

When a chart, table, diagram, or 3D view says more than prose, show it with `show_visual` before your final reply; show an image file with `show_image`. Do not restate what it shows. Full API: `visual_reference` or the `zeron:visuals` skill.",
};

pub fn sections(features: Features) -> Vec<Section> {
    let mut out = vec![CORE];
    if features.delegation {
        out.push(DELEGATION);
    }
    if features.waits {
        out.push(WAITS);
    }
    if features.side_chats_only {
        out.push(SIDE_CHATS_ONLY);
    }
    if features.visuals {
        out.push(VISUALS);
    }
    out
}

pub fn text(features: Features) -> String {
    sections(features)
        .iter()
        .map(|section| section.text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn with_plugins(features: Features, plugins: Option<&str>) -> String {
    let mut out = text(features);
    if let Some(plugins) = plugins.map(str::trim).filter(|text| !text.is_empty()) {
        out.push_str("\n\n");
        out.push_str(plugins);
    }
    out
}

pub fn codex_entries(features: Features, plugins: Option<&str>) -> Vec<(String, String)> {
    let mut values: Vec<String> = sections(features)
        .iter()
        .map(|section| section.text.to_owned())
        .collect();
    if let Some(plugins) = plugins.map(str::trim).filter(|text| !text.is_empty()) {
        values.extend(split_entries(plugins, CODEX_ENTRY_MAX));
    }
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| (format!("zeron_{:02}", index + 1), value))
        .collect()
}

pub fn split_entries(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for block in text.split("\n\n") {
        for piece in split_hard(block, max) {
            let joined = current.len() + 2 + piece.len();
            if !current.is_empty() && joined > max {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&piece);
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

fn split_hard(block: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = block;
    while rest.len() > max {
        let mut cut = max;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        let at = rest[..cut].rfind('\n').filter(|at| *at > 0).unwrap_or(cut);
        out.push(rest[..at].to_owned());
        rest = rest[at..].trim_start_matches('\n');
    }
    if !rest.is_empty() {
        out.push(rest.to_owned());
    }
    out
}

impl McpServer {
    pub const GUIDANCE_ENV: &'static str = "ZERON_MCP_GUIDANCE";
    pub const ORCHESTRATOR_ENV: &'static str = "ZERON_ORCH_V2";

    pub fn guidance_features(&self) -> Features {
        let in_chat = self
            .env
            .get(Self::CHAT_ID_ENV)
            .is_some_and(|id| !id.is_empty());
        Features {
            visuals: in_chat,
            waits: self.waits_on_background(),
            side_chats_only: self.side_chats_only(),
            delegation: in_chat
                && orchestrator_on(self.env.get(Self::ORCHESTRATOR_ENV).map(String::as_str)),
        }
    }

    pub fn guidance_in_prompt(&self) -> bool {
        self.env
            .get(Self::GUIDANCE_ENV)
            .is_some_and(|value| value == "prompt")
    }

    pub fn with_guidance_in_prompt(&self) -> Self {
        let mut server = self.clone();
        server
            .env
            .insert(Self::GUIDANCE_ENV.to_owned(), "prompt".to_owned());
        server
    }

    pub fn read_plugin_instructions(&self) -> Option<String> {
        std::fs::read_to_string(self.plugin_instructions()?)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }

    pub fn system_guidance(&self) -> String {
        with_plugins(
            self.guidance_features(),
            self.read_plugin_instructions().as_deref(),
        )
    }

    pub fn codex_guidance(&self) -> Vec<(String, String)> {
        codex_entries(
            self.guidance_features(),
            self.read_plugin_instructions().as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_instructions_fit_the_codex_one_liner() {
        assert!(MCP_INSTRUCTIONS.chars().count() <= 230, "{}", MCP_INSTRUCTIONS.len());
        assert!(MCP_INSTRUCTIONS.contains("search for zeron tools"));
    }

    #[test]
    fn core_guidance_opens_with_the_no_tools_rule_and_stays_small() {
        let body = CORE.text.split_once("\n\n").unwrap().1;
        assert!(body.starts_with("Ordinary coding tasks need no Zeron tools."));
        assert!(CORE.text.len() <= 2048, "{}", CORE.text.len());
    }

    #[test]
    fn sections_follow_the_features() {
        let keys = |features| sections(features).iter().map(|s| s.key).collect::<Vec<_>>();
        assert_eq!(keys(Features::default()), ["core"]);
        assert_eq!(keys(Features::all()), ["core", "waits", "side_chats", "visuals"]);
        assert!(!text(Features::default()).contains("wait_on_background"));
        assert!(text(Features { waits: true, ..Default::default() }).contains("wait_on_background"));
    }

    #[test]
    fn codex_entries_are_ordered_and_each_under_the_cap() {
        let plugins = format!("## Plugins\n\n{}\n\n### Two\n\n{}", "a".repeat(5_000), "b ".repeat(3_000));
        let entries = codex_entries(Features::all(), Some(&plugins));
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert!(entries.len() > 5);
        for (key, value) in &entries {
            assert!(value.len() < 4_000, "{key}: {}", value.len());
            assert!(!value.trim().is_empty());
        }
        let rejoined: String = entries[4..].iter().map(|(_, v)| v.replace('\n', "")).collect();
        assert_eq!(rejoined.matches('a').count(), 5_000);
    }

    #[test]
    fn a_server_reports_its_features_and_where_the_guidance_went() {
        let mut server = McpServer::default();
        assert_eq!(server.guidance_features(), Features::default());
        server.env.insert(McpServer::CHAT_ID_ENV.into(), "c".into());
        server.env.insert(McpServer::WAITS_ON_BACKGROUND_ENV.into(), "1".into());
        server.env.insert(McpServer::SIDE_CHATS_ONLY_ENV.into(), "1".into());
        assert_eq!(server.guidance_features(), Features::all());
        assert!(!server.guidance_in_prompt());
        assert!(server.with_guidance_in_prompt().guidance_in_prompt());
    }

    #[test]
    fn delegation_guidance_follows_the_orchestrator_flag_in_a_chat() {
        let mut server = McpServer::default();
        server
            .env
            .insert(McpServer::ORCHESTRATOR_ENV.into(), "1".into());
        assert!(!server.guidance_features().delegation);
        server.env.insert(McpServer::CHAT_ID_ENV.into(), "c".into());
        assert!(server.guidance_features().delegation);
        for (value, on) in [("true", true), (" on ", true), ("0", false), ("yes", false)] {
            server
                .env
                .insert(McpServer::ORCHESTRATOR_ENV.into(), value.into());
            assert_eq!(server.guidance_features().delegation, on, "{value}");
        }
        let on = Features {
            delegation: true,
            ..Features::all()
        };
        let keys: Vec<&str> = sections(on).iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
            ["core", "delegation", "waits", "side_chats", "visuals"]
        );
        assert!(!text(Features::all()).contains("delegate_task"));
        for (key, value) in codex_entries(on, None) {
            assert!(value.len() < 4_000, "{key}: {}", value.len());
        }
    }
}
