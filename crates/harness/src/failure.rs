use zeron_proto::{AgentEvent, FailureClass, HarnessId};

type Rules = &'static [(FailureClass, &'static [&'static str])];

const CLAUDE: Rules = &[
    (
        FailureClass::UsageLimit { reset_at: None },
        &["limit reached", "rate_limit_error"],
    ),
    (
        FailureClass::PermissionError,
        &[
            "authentication failed",
            "invalid api key",
            "please run /login",
            "not logged in",
            "oauth token has expired",
            "isn't allowed to use claude",
            "billing error",
            "isn't signed in",
        ],
    ),
    (
        FailureClass::ValidationError,
        &[
            "rejected as invalid",
            "model isn't available",
            "context window",
            "prompt is too long",
            "could not be processed",
        ],
    ),
    (
        FailureClass::ProviderError,
        &[
            "overloaded",
            "server error",
            "repeated api errors",
            "api request failed",
            "model returned an error",
        ],
    ),
    (FailureClass::TransportError, &["timed out"]),
];

const CODEX: Rules = &[
    (
        FailureClass::UsageLimit { reset_at: None },
        &[
            "codex usage limit reached",
            "codex rate limit reached",
            "you've hit your usage limit",
            "you have hit your usage limit",
        ],
    ),
    (
        FailureClass::PermissionError,
        &[
            "credentials were rejected",
            "run codex login",
            "isn't signed in",
        ],
    ),
    (FailureClass::ValidationError, &["bad request"]),
    (
        FailureClass::ProviderError,
        &["internal error", "server error", "upstream 502"],
    ),
    (
        FailureClass::TransportError,
        &[
            "stream failed",
            "stream disconnected",
            "app-server exited before responding",
            "app-server stdin closed",
            "broken pipe",
            "timed out",
        ],
    ),
];

pub fn classify_failure(harness: HarnessId, message: &str) -> FailureClass {
    classify_at(harness, message, now_ms())
}

fn classify_at(harness: HarnessId, message: &str, now: i64) -> FailureClass {
    let rules = match harness {
        HarnessId::ClaudeCode => CLAUDE,
        HarnessId::Codex => CODEX,
        _ => return FailureClass::Unknown,
    };
    let text = message.to_ascii_lowercase();
    let class = rules
        .iter()
        .find(|(_, needles)| needles.iter().any(|needle| text.contains(needle)))
        .map_or(FailureClass::Unknown, |(class, _)| *class);
    match class {
        FailureClass::UsageLimit { .. } => FailureClass::UsageLimit {
            reset_at: wait_ms(&text).map(|ms| now.saturating_add(ms)),
        },
        other => other,
    }
}

fn wait_ms(text: &str) -> Option<i64> {
    let (_, rest) = text.split_once("resets in ")?;
    let phrase = rest.split(['.', ',', ';', ')', '\n']).next()?.trim();
    if phrase.starts_with("under a minute") {
        return Some(60_000);
    }
    let parts: Vec<i64> = phrase.split_whitespace().map_while(token_ms).collect();
    (!parts.is_empty()).then(|| parts.into_iter().fold(0, i64::saturating_add))
}

fn token_ms(token: &str) -> Option<i64> {
    let digits = token.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit_ms = match &token[digits.len()..] {
        "d" => 86_400_000,
        "h" => 3_600_000,
        "m" => 60_000,
        _ => return None,
    };
    digits
        .parse::<i64>()
        .ok()
        .map(|n| n.saturating_mul(unit_ms))
}

pub(crate) fn describe_wait(secs: i64) -> String {
    let minutes = (secs.max(0) + 59) / 60;
    let (days, hours, minutes) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => "under a minute".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

pub(crate) fn now_secs() -> i64 {
    now_ms() / 1000
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub fn failure_of(harness: HarnessId, error: Option<&str>) -> Option<FailureClass> {
    error.map(|message| classify_failure(harness, message))
}

pub fn with_failure(harness: HarnessId, mut event: AgentEvent) -> AgentEvent {
    fill_failure(harness, &mut event);
    event
}

fn fill_failure(harness: HarnessId, event: &mut AgentEvent) {
    match event {
        AgentEvent::Done { error, failure, .. } if failure.is_none() => {
            *failure = failure_of(harness, error.as_deref());
        }
        AgentEvent::Subagent { event, .. } => fill_failure(harness, event),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(message: &str) -> FailureClass {
        classify_failure(HarnessId::ClaudeCode, message)
    }

    fn codex(message: &str) -> FailureClass {
        classify_failure(HarnessId::Codex, message)
    }

    #[test]
    fn claude_usage_limit_from_assistant_code_rate_event_and_429_result() {
        for message in [
            "Claude usage limit reached — try again after the limit resets.",
            "Claude 5-hour limit reached — the turn was blocked. Try again after it resets.",
            "Claude rate limit reached — try again after it resets. API Error: 429 rate_limit_error (resets 3pm)",
        ] {
            assert_eq!(claude(message), UNTIMED, "{message}");
        }
    }

    const UNTIMED: FailureClass = FailureClass::UsageLimit { reset_at: None };

    fn reset_at(harness: HarnessId, message: &str) -> Option<i64> {
        match classify_at(harness, message, 1_000) {
            FailureClass::UsageLimit { reset_at } => reset_at,
            other => panic!("{message}: {other:?}"),
        }
    }

    #[test]
    fn usage_limit_reset_comes_from_every_resets_in_wording() {
        const M: i64 = 60_000;
        const H: i64 = 60 * M;
        const D: i64 = 24 * H;
        for (wait, ms) in [
            ("under a minute", M),
            ("7m", 7 * M),
            ("3h", 3 * H),
            ("2h 14m", 2 * H + 14 * M),
            ("2d", 2 * D),
            ("3d 5h", 3 * D + 5 * H),
        ] {
            let codex = format!("Codex usage limit reached — it resets in {wait}.");
            assert_eq!(
                reset_at(HarnessId::Codex, &codex),
                Some(1_000 + ms),
                "{codex}"
            );
            let claude =
                format!("Claude 5-hour limit reached — the turn was blocked. It resets in {wait}.");
            assert_eq!(
                reset_at(HarnessId::ClaudeCode, &claude),
                Some(1_000 + ms),
                "{claude}"
            );
        }
    }

    #[test]
    fn usage_limit_without_a_parsable_wait_has_no_reset() {
        for message in [
            "Claude rate limit reached — try again after it resets. API Error: 429 rate_limit_error (resets 3pm)",
            "Claude usage limit reached — try again after the limit resets.",
            "Claude 5-hour limit reached — it resets in a while.",
            "Claude 5-hour limit reached — it resets in 3pm.",
        ] {
            assert_eq!(reset_at(HarnessId::ClaudeCode, message), None, "{message}");
        }
        assert_eq!(
            classify_at(HarnessId::Codex, "upstream 502, resets in 5m", 1_000),
            FailureClass::ProviderError
        );
    }

    #[test]
    fn describe_wait_round_trips_through_the_parser() {
        for secs in [0, 59, 61, 3599, 3600, 3660, 86_399] {
            let message = format!(
                "Codex usage limit reached — it resets in {}.",
                describe_wait(secs)
            );
            let late = reset_at(HarnessId::Codex, &message).expect("parsed") - 1_000 - secs * 1000;
            assert!((0..=60_000).contains(&late), "{message}: {late}");
        }
        assert_eq!(describe_wait(-5), "under a minute");
        assert_eq!(describe_wait(3660), "1h 1m");
    }

    #[test]
    fn claude_permission_from_auth_login_org_and_billing_text() {
        for message in [
            "Authentication failed — sign in to Claude again.",
            "Invalid API key · Fix external API key",
            "Invalid API key · Please run /login",
            "Not logged in · Please run /login",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
            "This organization isn't allowed to use Claude here.",
            "Billing error — check your Claude plan or payment method.",
            "Claude Code isn't signed in on Cloud. Sign in with Claude, pick an account or add an Anthropic API key in Settings → Providers → Cloud, then send again.",
        ] {
            assert_eq!(claude(message), FailureClass::PermissionError, "{message}");
        }
    }

    #[test]
    fn claude_validation_from_invalid_request_model_and_context_text() {
        for message in [
            "The request was rejected as invalid.",
            "The selected model isn't available.",
            "The conversation no longer fits the model's context window. Run /compact or start a new chat.",
            "Claude stopped: an image in the conversation could not be processed.",
        ] {
            assert_eq!(claude(message), FailureClass::ValidationError, "{message}");
        }
    }

    #[test]
    fn claude_provider_from_overloaded_server_and_api_error_text() {
        for message in [
            "Claude is overloaded right now — try again shortly.",
            "Claude had a server error — try again.",
            "Claude gave up after repeated API errors.",
            "Claude's API request failed.",
            "Claude stopped: the model returned an error.",
        ] {
            assert_eq!(claude(message), FailureClass::ProviderError, "{message}");
        }
    }

    #[test]
    fn claude_transport_from_initialize_timeout() {
        assert_eq!(
            claude("Claude initialize timed out after 60s"),
            FailureClass::TransportError
        );
    }

    #[test]
    fn claude_unknown_for_run_limits_and_crashes() {
        for message in [
            "The run hit the maximum number of turns.",
            "Claude's result for this turn could not be read.",
            "claude exited unexpectedly (exit status: 1)",
        ] {
            assert_eq!(claude(message), FailureClass::Unknown, "{message}");
        }
    }

    #[test]
    fn codex_usage_limit_from_error_info_text_and_raw_message() {
        for message in [
            "Codex usage limit reached — it resets in 2h 14m.",
            "Codex rate limit reached. Slow down",
            "You've hit your usage limit.",
            "You have hit your usage limit.",
        ] {
            assert!(
                matches!(codex(message), FailureClass::UsageLimit { .. }),
                "{message}"
            );
        }
    }

    const SIGNED_OUT: &str = "Codex's credentials were rejected. Run codex login (or check the API key for your model provider), then send again.";

    #[test]
    fn codex_permission_from_signed_out_message() {
        assert_eq!(codex(SIGNED_OUT), FailureClass::PermissionError);
    }

    #[test]
    fn codex_permission_wins_over_a_crash_report_after_it() {
        let error = format!(
            "{}\n\ncodex app-server exited unexpectedly (exit status: 3): thread main panicked at session.rs: lost the model client",
            SIGNED_OUT
        );
        assert_eq!(codex(&error), FailureClass::PermissionError);
    }

    #[test]
    fn codex_validation_from_bad_request() {
        assert_eq!(
            codex("thread/start: bad request"),
            FailureClass::ValidationError
        );
    }

    #[test]
    fn codex_provider_from_upstream_5xx_and_internal_error() {
        assert_eq!(
            codex("upstream 502 from the model"),
            FailureClass::ProviderError
        );
        assert_eq!(
            codex("internal error near rate limit middleware"),
            FailureClass::ProviderError
        );
    }

    #[test]
    fn codex_transport_from_stream_and_app_server_loss() {
        for message in [
            "stream failed",
            "thread/start: app-server exited before responding",
            "thread/resume: app-server stdin closed (Broken pipe (os error 32))",
        ] {
            assert_eq!(codex(message), FailureClass::TransportError, "{message}");
        }
    }

    #[test]
    fn other_harnesses_are_unknown() {
        assert_eq!(
            classify_failure(HarnessId::Cursor, "Claude usage limit reached"),
            FailureClass::Unknown
        );
    }

    #[test]
    fn no_error_is_no_failure() {
        assert_eq!(failure_of(HarnessId::ClaudeCode, None), None);
        assert_eq!(failure_of(HarnessId::Codex, None), None);
        assert_eq!(failure_of(HarnessId::Pi, None), None);
        assert_eq!(
            failure_of(HarnessId::Pi, Some("boom")),
            Some(FailureClass::Unknown)
        );
    }

    #[test]
    fn serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(UNTIMED).unwrap(),
            serde_json::json!("usage_limit")
        );
        assert_eq!(
            serde_json::to_value(FailureClass::PermissionError).unwrap(),
            serde_json::json!("permission_error")
        );
    }
}
