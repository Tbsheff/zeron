//! Approvals asked of the person. A run that is not in full access asks
//! before what its [`AccessMode`] does not cover; every driver asks the same
//! way — a yes/no question through [`crate::RunControls::request_input`] —
//! so an approval reads alike whichever agent raised it.
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use zeron_proto::{AccessMode, UserInputAnswer, UserInputQuestion};

pub const ALLOW: &str = "Yes";
pub const DENY: &str = "No";
const ASKED_KEPT: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub kind: &'static str,
    pub name: String,
    pub input: Value,
}

static ASKED: Mutex<VecDeque<(String, Asked)>> = Mutex::new(VecDeque::new());

fn keep(question: UserInputQuestion, asked: Asked) -> UserInputQuestion {
    let mut kept = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.len() >= ASKED_KEPT {
        kept.pop_front();
    }
    kept.push_back((question.id.clone(), asked));
    question
}

pub fn asked(question: &UserInputQuestion) -> Option<Asked> {
    let mut kept = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    let at = kept.iter().position(|(id, _)| *id == question.id)?;
    kept.remove(at).map(|(_, asked)| asked)
}

pub fn denial(reason: &str, question: &UserInputQuestion) -> Vec<UserInputAnswer> {
    vec![UserInputAnswer {
        question_id: question.id.clone(),
        labels: vec![DENY.into(), reason.into()],
    }]
}

pub fn approval(question: &UserInputQuestion) -> Vec<UserInputAnswer> {
    vec![UserInputAnswer {
        question_id: question.id.clone(),
        labels: vec![ALLOW.into()],
    }]
}

pub fn denied_because<'a>(
    question: &UserInputQuestion,
    answers: &'a [UserInputAnswer],
) -> Option<&'a str> {
    let answer = answers.iter().find(|a| a.question_id == question.id)?;
    match answer.labels.as_slice() {
        [no, reason] if no == DENY => Some(reason),
        _ => None,
    }
}

/// What an agent wants to do, for deciding whether its mode covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Write, edit or delete files.
    Edit,
    /// Anything else: a shell command, network, an MCP or unknown tool.
    Other,
}

/// Whether `access` lets the agent go ahead with `action` without asking.
pub fn allowed_without_asking(access: AccessMode, action: Action) -> bool {
    match access {
        AccessMode::FullAccess => true,
        AccessMode::AutoAcceptEdits => action == Action::Edit,
        AccessMode::Supervised => false,
    }
}

/// A yes/no approval question: `header` names the kind ("Approve command"),
/// `question` says what and ends asking ("… Allow it?").
pub fn question(header: impl Into<String>, question: impl Into<String>) -> UserInputQuestion {
    UserInputQuestion {
        id: uuid::Uuid::new_v4().to_string(),
        header: header.into(),
        question: question.into(),
        options: vec![ALLOW.into(), DENY.into()],
        multi_select: false,
        prefill: None,
        multiline: false,
    }
}

/// Whether `answers` approve `question`. Anything but an explicit yes —
/// a no, free text, or the questions closed unanswered — is a refusal, so
/// the agent is never allowed silently.
pub fn approved(question: &UserInputQuestion, answers: &[UserInputAnswer]) -> bool {
    answers.iter().any(|a| {
        a.question_id == question.id && a.labels.iter().any(|l| l.eq_ignore_ascii_case(ALLOW))
    })
}

/// "`agent` wants to run `command`. Allow it?"
pub fn command(agent: &str, command: &str) -> UserInputQuestion {
    let command = command.trim();
    let asked = Asked {
        kind: "command",
        name: command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned(),
        input: json!({ "command": command }),
    };
    let question = question(
        "Approve command",
        if command.is_empty() {
            format!("{agent} wants to run a command. Allow it?")
        } else {
            format!("{agent} wants to run `{command}`. Allow it?")
        },
    );
    keep(question, asked)
}

/// "`agent` wants to modify a, b. Allow it?"
pub fn edit(agent: &str, paths: &[&str]) -> UserInputQuestion {
    let asked = Asked {
        kind: "edit",
        name: "edit".into(),
        input: json!({ "paths": paths }),
    };
    let question = question(
        "Approve file change",
        if paths.is_empty() {
            format!("{agent} wants to modify files. Allow it?")
        } else {
            format!("{agent} wants to modify {}. Allow it?", paths.join(", "))
        },
    );
    keep(question, asked)
}

/// "`agent` wants to use `tool`. Allow it?", with what it says it is for.
pub fn tool(agent: &str, tool: &str, detail: Option<&str>) -> UserInputQuestion {
    let detail_said = detail
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    let asked = Asked {
        kind: "tool",
        name: tool.to_owned(),
        input: json!({ "detail": detail.map(str::trim).unwrap_or_default() }),
    };
    keep(
        question(
            "Approve tool",
            format!("{agent} wants to use {tool}{detail_said}. Allow it?"),
        ),
        asked,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_cover_what_they_say() {
        use AccessMode::*;
        assert!(allowed_without_asking(FullAccess, Action::Other));
        assert!(allowed_without_asking(AutoAcceptEdits, Action::Edit));
        assert!(!allowed_without_asking(AutoAcceptEdits, Action::Other));
        assert!(!allowed_without_asking(Supervised, Action::Edit));
    }

    #[test]
    fn only_an_explicit_yes_approves() {
        let q = command("Muse", "cargo test");
        assert_eq!(q.question, "Muse wants to run `cargo test`. Allow it?");
        let answer = |labels: &[&str]| {
            vec![UserInputAnswer {
                question_id: q.id.clone(),
                labels: labels.iter().map(|l| l.to_string()).collect(),
            }]
        };
        assert!(approved(&q, &answer(&["Yes"])));
        assert!(!approved(&q, &answer(&["No"])));
        assert!(!approved(&q, &answer(&["sure, go ahead"])));
        assert!(!approved(&q, &[]));
    }

    #[test]
    fn an_approval_keeps_what_it_asks_until_read_once() {
        let q = command("Codex", "git status --short");
        assert_eq!(
            asked(&q),
            Some(Asked {
                kind: "command",
                name: "git".into(),
                input: json!({ "command": "git status --short" })
            })
        );
        assert_eq!(asked(&q), None, "read once");
        let e = edit("Claude", &["a.rs"]);
        assert_eq!(
            asked(&e).map(|a| (a.kind, a.input)),
            Some(("edit", json!({ "paths": ["a.rs"] })))
        );
        let t = tool("Claude", "WebFetch", Some("docs"));
        assert_eq!(
            asked(&t).map(|a| (a.kind, a.name)),
            Some(("tool", "WebFetch".to_owned()))
        );
        assert_eq!(
            asked(&question("Approve plan", "Go?")),
            None,
            "only commands, edits and tools"
        );
    }

    #[test]
    fn a_denial_carries_its_reason() {
        let q = command("Claude", "rm -rf /");
        let no = denial("denied by Safe commands", &q);
        assert!(!approved(&q, &no));
        assert_eq!(denied_because(&q, &no), Some("denied by Safe commands"));
        assert!(approved(&q, &approval(&q)));
        assert_eq!(denied_because(&q, &approval(&q)), None);
    }
}
