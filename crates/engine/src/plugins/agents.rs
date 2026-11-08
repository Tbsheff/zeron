use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value as Json, json};
use zeron_proto::{HarnessId, McpServer};

use super::manifest::{Manifest, skill_name};
use super::scan::Entry;
use super::{Plugins, hooks, install, lock};

pub const PERMISSION: &str = "agents.instruct";
pub const MAX_INSTRUCTIONS: usize = 8 * 1024;
pub const MAX_SKILL: usize = 32 * 1024;
pub const MAX_TOTAL: usize = 24 * 1024;

const HEADER: &str = "## Plugins\n\nThe person turned on these Zeron plugins. Follow what they say where it applies.\n";
const AGENTS_DIR: &str = ".agents";

pub type ProjectOfFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn files_problem(
    manifest: &Manifest,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> Option<String> {
    if let Some(path) = &manifest.instructions
        && let Err(problem) = text_file(&read, path, MAX_INSTRUCTIONS, "instructions")
    {
        return Some(problem);
    }
    for path in &manifest.skills {
        let text = match text_file(&read, path, MAX_SKILL, "skill") {
            Ok(text) => text,
            Err(problem) => return Some(problem),
        };
        let name = skill_name(path).unwrap_or_default();
        match frontmatter(&text) {
            Some((said, description)) if said == name && !description.is_empty() => {}
            _ => {
                return Some(format!(
                    "Its skill {path} must start with front matter giving \"name: {name}\" and a description"
                ));
            }
        }
    }
    None
}

fn text_file(
    read: &impl Fn(&str) -> Option<Vec<u8>>,
    path: &str,
    cap: usize,
    what: &str,
) -> Result<String, String> {
    let bytes = read(path).ok_or_else(|| format!("Its {what} file {path} is missing"))?;
    if bytes.len() > cap {
        return Err(format!("Its {what} file {path} is over {} KB", cap / 1024));
    }
    String::from_utf8(bytes).map_err(|_| format!("Its {what} file {path} isn't UTF-8 text"))
}

pub fn frontmatter(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let block = &rest[..rest.find("\n---")?];
    let field = |key: &str| {
        block.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim().trim_matches(['"', '\'']).to_owned())
        })
    };
    Some((field("name")?, field("description").unwrap_or_default()))
}

struct Source {
    plugin: String,
    name: String,
    hash: String,
    snapshot: PathBuf,
    instructions: Option<String>,
    skills: Vec<String>,
}

impl Source {
    fn of(entry: &Entry) -> Option<Self> {
        let manifest = entry.inspected.manifest.as_ref()?;
        let gives = manifest.instructions.is_some()
            || !manifest.skills.is_empty()
            || manifest.service.is_some();
        if !entry.allows(PERMISSION) || !gives {
            return None;
        }
        Some(Self {
            plugin: entry.item.id.clone(),
            name: entry.item.name.clone(),
            hash: entry.item.hash.clone(),
            snapshot: PathBuf::from(entry.item.snapshot.as_ref()?),
            instructions: manifest.instructions.clone(),
            skills: manifest.skills.clone(),
        })
    }

    fn read(&self, path: &str, cap: usize) -> Option<String> {
        let file = path
            .split('/')
            .fold(self.snapshot.clone(), |dir, part| dir.join(part));
        std::fs::read_to_string(file)
            .ok()
            .filter(|text| text.len() <= cap)
    }

    fn skills_folder(&self) -> String {
        let short: String = self
            .hash
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect();
        format!("{}-{short}", self.plugin)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Delivery {
    pub instructions: Option<PathBuf>,
    pub skills: Vec<PathBuf>,
}

impl Delivery {
    pub fn stamp(&self, mcp: &mut McpServer) {
        *mcp = mcp.without_plugin_context();
        if let Some(path) = &self.instructions {
            mcp.env.insert(
                McpServer::PLUGIN_INSTRUCTIONS_ENV.into(),
                path.display().to_string(),
            );
        }
        if let Ok(joined) = std::env::join_paths(&self.skills)
            && !self.skills.is_empty()
        {
            mcp.env.insert(
                McpServer::PLUGIN_SKILLS_ENV.into(),
                joined.to_string_lossy().into_owned(),
            );
        }
    }
}

impl Plugins {
    pub fn set_project_of(
        &self,
        project_of: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) {
        *lock(&self.inner.project_of) = Some(Arc::new(project_of));
    }

    fn instructing(&self) -> Vec<Source> {
        let mut sources: Vec<(i64, Source)> = lock(&self.inner.entries)
            .iter()
            .filter_map(Source::of)
            .map(|source| {
                let dir = self.inner.plugins_dir.join(&source.plugin);
                (
                    install::Source::load(&dir).map_or(0, |s| s.installed_at),
                    source,
                )
            })
            .collect();
        sources.sort_by(|(a, x), (b, y)| (a, &x.plugin).cmp(&(b, &y.plugin)));
        sources.into_iter().map(|(_, source)| source).collect()
    }

    pub async fn agent_context(&self, chat: &str, harness: HarnessId) -> Delivery {
        let sources = self.instructing();
        let root = self.inner.plugins_dir.join(AGENTS_DIR);
        let chat_file = root.join("chats").join(format!("{}.md", safe_name(chat)));
        let skills_root = root.join("skills");
        if sources.is_empty() {
            prune(&root.join("skills"), &[]);
            let _ = std::fs::remove_file(&chat_file);
            prune(&skills_root, &[]);
            return Delivery::default();
        }
        let project = lock(&self.inner.project_of).clone().and_then(|of| of(chat));
        let mut ctx = json!({ "session": chat, "harness": harness.as_str() });
        if let Some(project) = project {
            ctx["project"] = json!(project);
        }
        let mut dynamic = self.instructed(&ctx).await;
        let native_skills = harness == HarnessId::ClaudeCode;
        let mut text = String::from(HEADER);
        let mut skills = Vec::new();
        let mut kept = Vec::new();
        for source in &sources {
            let skills_dir = (!source.skills.is_empty())
                .then(|| build_skills(&skills_root, source))
                .flatten();
            if let Some(dir) = skills_dir.as_ref() {
                kept.push(source.skills_folder());
                if native_skills {
                    skills.push(dir.clone());
                }
            }
            let listed = skills_dir.filter(|_| !native_skills);
            let block = self.block(source, dynamic.remove(&source.plugin), listed.as_deref());
            if block.is_empty() {
                continue;
            }
            if text.len() + block.len() > MAX_TOTAL {
                let why = format!(
                    "its agent instructions were left out of chat {chat}: all plugins' together would be over {} KB",
                    MAX_TOTAL / 1024
                );
                tracing::warn!(plugin = %source.plugin, "plugins: {why}");
                self.inner.services.note(&source.plugin, &why);
                continue;
            }
            text.push_str(&block);
        }
        prune(&skills_root, &kept);
        let instructions = (text.len() > HEADER.len())
            .then(|| write_atomic(&chat_file, &text).ok())
            .flatten();
        if instructions.is_none() {
            let _ = std::fs::remove_file(&chat_file);
        }
        Delivery {
            instructions,
            skills,
        }
    }

    fn block(&self, source: &Source, dynamic: Option<String>, skills_dir: Option<&Path>) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(text) = source
            .instructions
            .as_deref()
            .and_then(|path| source.read(path, MAX_INSTRUCTIONS))
            .filter(|t| !t.trim().is_empty())
        {
            parts.push(text.trim().to_owned());
        }
        if let Some(text) = dynamic {
            parts.push(text);
        }
        if let Some(dir) = skills_dir {
            parts.push(skills_listing(source, dir));
        }
        if parts.is_empty() {
            return String::new();
        }
        format!("\n### {}\n\n{}\n", source.name, parts.join("\n\n"))
    }

    async fn instructed(&self, ctx: &Json) -> HashMap<String, String> {
        let plans = self.hooked(hooks::INSTRUCT);
        let asked = plans.iter().map(|plan| async move {
            let answers = self
                .inner
                .services
                .decide(plan, hooks::INSTRUCT, ctx.clone(), &|_| false)
                .await;
            let texts: Vec<String> = answers
                .into_iter()
                .filter_map(|answer| match answer {
                    Ok(Json::String(text)) => Some(text.trim().to_owned()),
                    Ok(Json::Null) | Err(_) => None,
                    Ok(other) => {
                        self.inner.services.note(
                            &plan.plugin,
                            &format!("zeron.instruct answered {other}, which isn't text, so Zeron left it out"),
                        );
                        None
                    }
                })
                .filter(|text| !text.is_empty())
                .collect();
            let text = texts.join("\n\n");
            if text.len() > MAX_INSTRUCTIONS {
                self.inner.services.note(
                    &plan.plugin,
                    &format!("zeron.instruct gave over {} KB, so Zeron left it out", MAX_INSTRUCTIONS / 1024),
                );
                return None;
            }
            (!text.is_empty()).then(|| (plan.plugin.clone(), text))
        });
        futures::future::join_all(asked)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

fn skills_listing(source: &Source, dir: &Path) -> String {
    let mut lines = vec!["Its skills (read a skill's file before you follow it):".to_owned()];
    for path in &source.skills {
        let Some(name) = skill_name(path) else {
            continue;
        };
        let file = dir.join("skills").join(name).join("SKILL.md");
        let description = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| frontmatter(&text))
            .map(|(_, description)| description)
            .unwrap_or_default();
        lines.push(format!("- `{name}`: {description} ({})", file.display()));
    }
    lines.join("\n")
}

fn build_skills(root: &Path, source: &Source) -> Option<PathBuf> {
    let dir = root.join(source.skills_folder());
    if dir.is_dir() {
        return Some(dir);
    }
    let staging = root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let built = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(staging.join(".claude-plugin"))?;
        let manifest = json!({
            "name": source.plugin,
            "description": format!("Skills from the {} Zeron plugin", source.name),
        });
        std::fs::write(
            staging.join(".claude-plugin").join("plugin.json"),
            manifest.to_string(),
        )?;
        for path in &source.skills {
            let (Some(name), Some(text)) = (skill_name(path), source.read(path, MAX_SKILL)) else {
                continue;
            };
            let folder = staging.join("skills").join(name);
            std::fs::create_dir_all(&folder)?;
            std::fs::write(folder.join("SKILL.md"), text)?;
        }
        std::fs::rename(&staging, &dir)
    })();
    if built.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    dir.is_dir().then_some(dir)
}

fn prune(root: &Path, kept: &[String]) {
    let Ok(children) = std::fs::read_dir(root) else {
        return;
    };
    for child in children.flatten() {
        let name = child.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && !kept.contains(&name) {
            let _ = std::fs::remove_dir_all(child.path());
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let staging = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&staging);
    })?;
    Ok(path.to_path_buf())
}

fn safe_name(chat: &str) -> String {
    chat.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
