use zeron_proto::{AgentEvent, FailureClass, HarnessId};

type Rules = &'static [(FailureClass, &'static [&'static str])];

const CLAUDE: Rules = &[
    (
        FailureClass::UsageLimit,
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
        FailureClass::UsageLimit,
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
    let rules = match harness {
        HarnessId::ClaudeCode => CLAUDE,
        HarnessId::Codex => CODEX,
        _ => return FailureClass::Unknown,
    };
    let text = message.to_ascii_lowercase();
    rules
        .iter()
        .find(|(_, needles)| needles.iter().any(|needle| text.contains(needle)))
        .map_or(FailureClass::Unknown, |(class, _)| *class)
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
            assert_eq!(claude(message), FailureClass::UsageLimit, "{message}");
        }
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
            assert_eq!(codex(message), FailureClass::UsageLimit, "{message}");
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
            serde_json::to_value(FailureClass::UsageLimit).unwrap(),
            serde_json::json!("usage_limit")
        );
        assert_eq!(
            serde_json::to_value(FailureClass::PermissionError).unwrap(),
            serde_json::json!("permission_error")
        );
    }
}
