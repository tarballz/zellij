//! Decides whether a session's blurb needs regenerating, and regenerates it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use zellij_utils::input::ai_summary::AiSummaryConfig;

use super::prompt::{build_prompt, PromptOptions, SessionSnapshot};
use super::provider::{summarize, ProviderRequest};

/// Upper bound on the size of a prompt, in bytes.
pub const MAX_PROMPT_BYTES: usize = 6000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Nothing changed since the last blurb, so the provider wasn't called.
    Unchanged,
    /// The provider couldn't be reached or didn't produce a usable blurb.
    Failed,
    /// A new blurb. `fingerprint` identifies the input it was generated from.
    Updated { blurb: String, fingerprint: u64 },
}

pub fn prompt_options(config: &AiSummaryConfig) -> PromptOptions {
    PromptOptions {
        include_screen_text: config.effective_include_screen_text(),
        screen_lines: config.effective_screen_lines(),
        max_bytes: MAX_PROMPT_BYTES,
    }
}

/// Regenerates the blurb unless the prompt built from `snapshot` is the one that produced
/// `last_fingerprint`. The fingerprint is taken from the prompt itself, so anything that
/// doesn't reach the provider (screen text when it is disabled, say) never causes a call.
pub fn refresh_blurb<F>(
    config: &AiSummaryConfig,
    api_key: &str,
    snapshot: &SessionSnapshot,
    last_fingerprint: Option<u64>,
    send: F,
) -> RefreshOutcome
where
    F: FnOnce(&ProviderRequest) -> Result<String, String>,
{
    let prompt = build_prompt(snapshot, &prompt_options(config));
    let mut hasher = DefaultHasher::new();
    prompt.hash(&mut hasher);
    let fingerprint = hasher.finish();
    if last_fingerprint == Some(fingerprint) {
        return RefreshOutcome::Unchanged;
    }
    match summarize(config, api_key, &prompt, send) {
        Some(blurb) => RefreshOutcome::Updated { blurb, fingerprint },
        None => RefreshOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::super::prompt::{PaneSnapshot, TabSnapshot};
    use super::*;
    use std::cell::Cell;
    use zellij_utils::input::ai_summary::AiProvider;

    const REPLY: &str = r#"{"content":[{"type":"text","text":"Building zellij"}]}"#;

    fn config() -> AiSummaryConfig {
        AiSummaryConfig {
            provider: Some(AiProvider::Claude),
            ..Default::default()
        }
    }

    fn snapshot(title: &str, screen_text: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_name: "demo".to_owned(),
            tabs: vec![TabSnapshot {
                name: "main".to_owned(),
                panes: vec![PaneSnapshot {
                    title: title.to_owned(),
                    screen_text: Some(screen_text.to_owned()),
                    ..Default::default()
                }],
            }],
        }
    }

    fn fingerprint_of(outcome: RefreshOutcome) -> u64 {
        match outcome {
            RefreshOutcome::Updated { fingerprint, .. } => fingerprint,
            other => panic!("expected an update, got {:?}", other),
        }
    }

    #[test]
    fn the_first_refresh_calls_the_provider_and_returns_the_blurb() {
        let outcome = refresh_blurb(&config(), "key", &snapshot("shell", "ls"), None, |_| {
            Ok(REPLY.to_owned())
        });
        match outcome {
            RefreshOutcome::Updated { blurb, .. } => assert_eq!(blurb, "Building zellij"),
            other => panic!("expected an update, got {:?}", other),
        }
    }

    #[test]
    fn an_unchanged_snapshot_does_not_call_the_provider() {
        let fingerprint = fingerprint_of(refresh_blurb(
            &config(),
            "key",
            &snapshot("shell", "ls"),
            None,
            |_| Ok(REPLY.to_owned()),
        ));
        let called = Cell::new(false);
        let outcome = refresh_blurb(
            &config(),
            "key",
            &snapshot("shell", "ls"),
            Some(fingerprint),
            |_| {
                called.set(true);
                Ok(REPLY.to_owned())
            },
        );
        assert_eq!(outcome, RefreshOutcome::Unchanged);
        assert!(!called.get());
    }

    #[test]
    fn a_changed_snapshot_calls_the_provider_again() {
        let first = fingerprint_of(refresh_blurb(
            &config(),
            "key",
            &snapshot("shell", "ls"),
            None,
            |_| Ok(REPLY.to_owned()),
        ));
        let second = fingerprint_of(refresh_blurb(
            &config(),
            "key",
            &snapshot("nvim", "ls"),
            Some(first),
            |_| Ok(REPLY.to_owned()),
        ));
        assert_ne!(first, second);
    }

    #[test]
    fn screen_only_changes_are_ignored_when_screen_text_is_disabled() {
        let mut config = config();
        config.include_screen_text = Some(false);
        let first = fingerprint_of(refresh_blurb(
            &config,
            "key",
            &snapshot("shell", "first screen"),
            None,
            |_| Ok(REPLY.to_owned()),
        ));
        let outcome = refresh_blurb(
            &config,
            "key",
            &snapshot("shell", "completely different screen"),
            Some(first),
            |_| panic!("the provider must not be called"),
        );
        assert_eq!(outcome, RefreshOutcome::Unchanged);
    }

    #[test]
    fn a_failing_provider_is_a_failed_refresh() {
        let outcome = refresh_blurb(&config(), "key", &snapshot("shell", "ls"), None, |_| {
            Err("connection refused".to_owned())
        });
        assert_eq!(outcome, RefreshOutcome::Failed);
    }

    #[test]
    fn prompt_options_follow_the_config() {
        let mut config = config();
        config.include_screen_text = Some(false);
        config.screen_lines = Some(7);
        let options = prompt_options(&config);
        assert!(!options.include_screen_text);
        assert_eq!(options.screen_lines, 7);
        assert_eq!(options.max_bytes, MAX_PROMPT_BYTES);
    }
}
