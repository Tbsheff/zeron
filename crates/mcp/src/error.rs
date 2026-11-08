use serde_json::{Value, json};
use zeron_rpc::RpcError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

pub const INVALID_ARGUMENTS: &str = "invalid_arguments";
pub const NOT_FOUND: &str = "not_found";
pub const AMBIGUOUS: &str = "ambiguous";
pub const REJECTED: &str = "rejected";
pub const ENGINE_UNAVAILABLE: &str = "engine_unavailable";
pub const UNKNOWN_TOOL: &str = "unknown_tool";

impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_ARGUMENTS, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(NOT_FOUND, message)
    }

    pub fn ambiguous(message: impl Into<String>) -> Self {
        Self::new(AMBIGUOUS, message)
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self::new(REJECTED, message)
    }

    pub fn with_example(mut self, tool: &str, example: &str) -> Self {
        if !example.is_empty() {
            self.message = format!("{} Valid call: {tool} {example}", self.message);
        }
        self
    }

    pub fn from_anyhow(error: &anyhow::Error) -> Self {
        let message = format!("{error:#}");
        if let Some(typed) = error.downcast_ref::<ToolError>() {
            return Self::new(typed.code, message);
        }
        if let Some(RpcError::Transport(_)) = error.downcast_ref::<RpcError>() {
            return Self::new(ENGINE_UNAVAILABLE, unavailable(&message));
        }
        if error.downcast_ref::<serde_json::Error>().is_some() {
            return Self::invalid(format!("invalid arguments: {message}"));
        }
        Self::rejected(message)
    }

    pub fn from_rpc(error: &RpcError, unknown_method: &str) -> Self {
        match error {
            RpcError::Failed(message) => Self::rejected(message.clone()),
            RpcError::BadParams(message) => Self::invalid(message.clone()),
            RpcError::UnknownMethod(_) => Self::rejected(unknown_method),
            RpcError::Transport(message) => Self::new(ENGINE_UNAVAILABLE, unavailable(message)),
            other => Self::rejected(other.to_string()),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

fn unavailable(message: &str) -> String {
    format!("Zeron's engine is not reachable ({message}). Is Zeron running? Retry once it is.")
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

#[cfg(test)]
impl ToolError {
    pub fn contains(&self, text: &str) -> bool {
        self.message.contains(text)
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(error: anyhow::Error) -> Self {
        Self::from_anyhow(&error)
    }
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb)).min(row[j] + 1).min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

pub fn suggestions<'a>(key: &str, names: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<&'a str> {
    let key = key.trim().to_lowercase();
    let mut scored: Vec<(usize, &str)> = names
        .filter_map(|(label, shown)| {
            let label = label.trim().to_lowercase();
            if label.is_empty() {
                return None;
            }
            let distance = edit_distance(&key, &label);
            let close = distance <= (key.chars().count() / 3).max(2)
                || label.contains(&key)
                || (key.len() >= 3 && key.contains(&label));
            close.then_some((distance, shown))
        })
        .collect();
    scored.sort_by_key(|(distance, _)| *distance);
    scored.dedup_by_key(|(_, shown)| *shown);
    scored.into_iter().take(3).map(|(_, shown)| shown).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_errors_keep_their_code_through_context() {
        let error = anyhow::Error::new(ToolError::not_found("no chat")).context("reading");
        assert_eq!(ToolError::from_anyhow(&error).code, NOT_FOUND);
        assert_eq!(ToolError::from_anyhow(&anyhow::anyhow!("nope")).code, REJECTED);
        let transport = anyhow::Error::new(RpcError::Transport("refused".into()));
        assert_eq!(ToolError::from_anyhow(&transport).code, ENGINE_UNAVAILABLE);
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("alpah", "alpha"), 2);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn suggestions_find_near_titles() {
        let names = [("Windows lane", "a"), ("Linux lane", "b"), ("Docs", "c")];
        assert_eq!(suggestions("windows lan", names.iter().copied()), ["a"]);
        let lane = suggestions("lane", names.iter().copied());
        assert!(lane.contains(&"a") && lane.contains(&"b") && lane.len() == 2);
        assert!(suggestions("zzzzzz", names.iter().copied()).is_empty());
    }
}
