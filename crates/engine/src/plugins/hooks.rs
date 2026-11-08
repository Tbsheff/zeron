use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as Json, json};
use zeron_harness::approval;
use zeron_proto::{UserInputAnswer, UserInputQuestion};

use super::service::Plan;
use super::{Plugins, install, lock};

pub const DISPATCH: &str = "message.dispatch";
pub const APPROVE: &str = "tool.approve";

#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    Proceed,
    Wait {
        plugin: String,
        reason: String,
        until: Option<i64>,
    },
    Reject {
        plugin: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Approval {
    Ask,
    Allow { plugin: String },
    Deny { plugin: String },
}

pub fn in_install_order(plugins_dir: &Path, plans: &[Plan]) -> Vec<Plan> {
    let mut hooked: Vec<(i64, Plan)> = plans
        .iter()
        .filter(|p| !p.hooks.is_empty())
        .map(|p| {
            let installed =
                install::Source::load(&plugins_dir.join(&p.plugin)).map_or(0, |s| s.installed_at);
            (installed, p.clone())
        })
        .collect();
    hooked.sort_by(|(a, x), (b, y)| (a, &x.plugin).cmp(&(b, &y.plugin)));
    hooked.into_iter().map(|(_, plan)| plan).collect()
}

fn deciding(plans: &[Plan], hook: &str) -> Vec<Plan> {
    plans
        .iter()
        .filter(|p| p.hooks.iter().any(|h| h == hook) && p.allows(permission(hook)))
        .cloned()
        .collect()
}

fn permission(hook: &str) -> &'static str {
    super::manifest::KNOWN_HOOKS
        .iter()
        .find(|(name, _)| *name == hook)
        .map_or("", |(_, permission)| permission)
}

fn shown(ctx: &Json, plan: &Plan) -> Json {
    let mut shown = ctx.clone();
    if let Some(fields) = shown.as_object_mut() {
        if !plan.allows("messages.read") {
            fields.remove("text");
            fields.remove("input");
        }
        if !plan.allows("files.read") {
            fields.remove("cwd");
        }
    }
    shown
}

fn dispatch_answer(plan: &Plan, value: &Json) -> Option<Dispatch> {
    let action = match value {
        Json::Null => "proceed",
        Json::Object(fields) => fields.get("action").and_then(Json::as_str)?,
        _ => return None,
    };
    let said = |key: &str| value[key].as_str().unwrap_or_default().trim().to_owned();
    Some(match action {
        "proceed" => Dispatch::Proceed,
        "wait" => Dispatch::Wait {
            plugin: plan.name.clone(),
            reason: said("reason"),
            until: value["until"].as_f64().map(|s| s as i64),
        },
        "reject" => Dispatch::Reject {
            plugin: plan.name.clone(),
            message: said("message"),
        },
        _ => return None,
    })
}

fn approval_answer(value: &Json) -> Option<&str> {
    match value {
        Json::Null => Some("ask"),
        Json::String(word) if matches!(word.as_str(), "allow" | "deny" | "ask") => Some(word),
        _ => None,
    }
}

impl Plugins {
    pub fn set_rechecker(&self, recheck: impl Fn() + Send + Sync + 'static) {
        *lock(&self.inner.rechecker) = Some(Arc::new(recheck));
    }

    pub fn hooks(&self, hook: &str) -> bool {
        !self.hooked(hook).is_empty()
    }

    fn hooked(&self, hook: &str) -> Vec<Plan> {
        deciding(&lock(&self.inner.hooked), hook)
    }

    fn unclear(&self, plan: &Plan, hook: &str, value: &Json) {
        self.inner.services.note(
            &plan.plugin,
            &format!("zeron.hook(\"{hook}\") answered {value}, which isn't a decision, so Zeron went on as if it said nothing"),
        );
    }

    pub async fn decide_dispatch(&self, ctx: Json) -> Dispatch {
        for plan in self.hooked(DISPATCH) {
            let settled = |value: &Json| {
                !matches!(
                    dispatch_answer(&plan, value),
                    Some(Dispatch::Proceed) | None
                )
            };
            let answers = self
                .inner
                .services
                .decide(&plan, DISPATCH, shown(&ctx, &plan), &settled)
                .await;
            for value in answers.into_iter().flatten() {
                match dispatch_answer(&plan, &value) {
                    Some(Dispatch::Proceed) => {}
                    Some(decided) => return decided,
                    None => self.unclear(&plan, DISPATCH, &value),
                }
            }
        }
        Dispatch::Proceed
    }

    pub async fn decide_approval(&self, ctx: Json) -> Approval {
        let what = format!(
            "{} {} in chat {}",
            ctx["kind"].as_str().unwrap_or_default(),
            ctx["name"].as_str().unwrap_or_default(),
            ctx["session"].as_str().unwrap_or_default()
        );
        let mut allowed = None;
        for plan in self.hooked(APPROVE) {
            let settled = |value: &Json| approval_answer(value) == Some("deny");
            let answers = self
                .inner
                .services
                .decide(&plan, APPROVE, shown(&ctx, &plan), &settled)
                .await;
            let words: Vec<&str> = answers
                .iter()
                .flatten()
                .filter_map(|value| {
                    approval_answer(value).or_else(|| {
                        self.unclear(&plan, APPROVE, value);
                        None
                    })
                })
                .collect();
            if words.contains(&"deny") {
                self.inner
                    .services
                    .note(&plan.plugin, &format!("tool.approve: denied {what}"));
                return Approval::Deny { plugin: plan.name };
            }
            if words.contains(&"allow") {
                self.inner
                    .services
                    .note(&plan.plugin, &format!("tool.approve: allowed {what}"));
                allowed.get_or_insert(plan.name);
            }
        }
        allowed.map_or(Approval::Ask, |plugin| Approval::Allow { plugin })
    }
}

pub fn approval_ctx(questions: &[UserInputQuestion], session: &str, cwd: &str) -> Option<Json> {
    let [question] = questions else {
        return None;
    };
    let asked = approval::asked(question)?;
    Some(
        json!({ "session": session, "kind": asked.kind, "name": asked.name, "input": asked.input, "cwd": cwd }),
    )
}

pub fn approval_answers(
    decided: &Approval,
    question: &UserInputQuestion,
) -> Option<Vec<UserInputAnswer>> {
    match decided {
        Approval::Ask => None,
        Approval::Allow { .. } => Some(approval::approval(question)),
        Approval::Deny { plugin } => {
            Some(approval::denial(&format!("denied by {plugin}"), question))
        }
    }
}

pub fn dispatch_ctx(session: &str, source: &str, plugin: Option<&str>, text: &str) -> Json {
    let mut ctx = json!({ "session": session, "source": source, "text": text });
    if let Some(plugin) = plugin {
        ctx["plugin"] = json!(plugin);
    }
    ctx
}

#[cfg(test)]
mod tests;
