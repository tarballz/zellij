//! Turns a snapshot of a session into the text sent to the model provider.

use super::redact::{last_lines, redact, truncate_bytes};

pub const INSTRUCTIONS: &str = "Describe in one short line (under 60 characters, no quotes, no \
trailing period) what is being worked on in this terminal multiplexer session, based on the \
details below.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub title: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub screen_text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabSnapshot {
    pub name: String,
    pub panes: Vec<PaneSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session_name: String,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptOptions {
    pub include_screen_text: bool,
    pub screen_lines: usize,
    pub max_bytes: usize,
}

/// Builds the full prompt for a session. Everything that came from the session is redacted,
/// and the whole prompt is at most `options.max_bytes` long.
pub fn build_prompt(snapshot: &SessionSnapshot, options: &PromptOptions) -> String {
    let mut data = format!("Session: {}\n", redact(&snapshot.session_name));
    for tab in &snapshot.tabs {
        data.push_str(&format!("Tab: {}\n", redact(&tab.name)));
        for pane in &tab.panes {
            data.push_str(&format!("  Pane: {}\n", redact(&pane.title)));
            if let Some(command) = &pane.command {
                data.push_str(&format!("    command: {}\n", redact(command)));
            }
            if let Some(cwd) = &pane.cwd {
                data.push_str(&format!("    cwd: {}\n", redact(cwd)));
            }
            if let (true, Some(screen_text)) = (options.include_screen_text, &pane.screen_text) {
                let recent = redact(&last_lines(screen_text, options.screen_lines));
                if !recent.is_empty() {
                    data.push_str("    recent output:\n");
                    for line in recent.lines() {
                        data.push_str(&format!("    {}\n", line));
                    }
                }
            }
        }
    }
    let budget = options.max_bytes.saturating_sub(INSTRUCTIONS.len() + 2);
    format!("{}\n\n{}", INSTRUCTIONS, truncate_bytes(&data, budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> PromptOptions {
        PromptOptions {
            include_screen_text: true,
            screen_lines: 3,
            max_bytes: 4000,
        }
    }

    fn snapshot_with_screen(screen_text: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_name: "rusty-owl".to_owned(),
            tabs: vec![TabSnapshot {
                name: "build".to_owned(),
                panes: vec![PaneSnapshot {
                    title: "cargo watch".to_owned(),
                    command: Some("cargo watch -x test".to_owned()),
                    cwd: Some("/home/me/zellij".to_owned()),
                    screen_text: Some(screen_text.to_owned()),
                }],
            }],
        }
    }

    #[test]
    fn the_prompt_describes_the_tabs_and_panes() {
        let prompt = build_prompt(&snapshot_with_screen("ok"), &options());
        assert!(prompt.starts_with(INSTRUCTIONS));
        for expected in [
            "rusty-owl",
            "build",
            "cargo watch",
            "cargo watch -x test",
            "/home/me/zellij",
        ] {
            assert!(
                prompt.contains(expected),
                "missing {:?} in {}",
                expected,
                prompt
            );
        }
    }

    #[test]
    fn screen_text_is_left_out_when_disabled() {
        let mut opts = options();
        opts.include_screen_text = false;
        let prompt = build_prompt(&snapshot_with_screen("UNIQUE-SCREEN-LINE"), &opts);
        assert!(!prompt.contains("UNIQUE-SCREEN-LINE"));
        assert!(prompt.contains("cargo watch -x test"));
    }

    #[test]
    fn screen_text_keeps_only_the_last_lines() {
        let prompt = build_prompt(
            &snapshot_with_screen("one\ntwo\nthree\nfour\nfive"),
            &options(),
        );
        for kept in ["three", "four", "five"] {
            assert!(prompt.contains(kept), "missing {:?} in {}", kept, prompt);
        }
        // "one" can't be checked: the instructions contain it
        assert!(!prompt.contains("two"));
    }

    #[test]
    fn screen_text_is_redacted() {
        let prompt = build_prompt(
            &snapshot_with_screen("export MY_API_KEY=hunter2hunter2"),
            &options(),
        );
        assert!(prompt.contains("MY_API_KEY=[REDACTED]"));
        assert!(!prompt.contains("hunter2"));
    }

    #[test]
    fn commands_and_titles_are_redacted_too() {
        let mut snapshot = snapshot_with_screen("ok");
        snapshot.tabs[0].panes[0].command = Some("deploy DB_PASSWORD=swordfish".to_owned());
        snapshot.tabs[0].panes[0].title = "AUTH_TOKEN=abc123".to_owned();
        let prompt = build_prompt(&snapshot, &options());
        assert!(!prompt.contains("swordfish"));
        assert!(!prompt.contains("abc123"));
    }

    #[test]
    fn the_prompt_never_exceeds_the_byte_limit_and_keeps_the_instructions() {
        let long_screen = "line of output\n".repeat(500);
        let mut opts = options();
        opts.screen_lines = 500;
        opts.max_bytes = 600;
        let prompt = build_prompt(&snapshot_with_screen(&long_screen), &opts);
        assert!(prompt.len() <= 600, "len was {}", prompt.len());
        assert!(prompt.starts_with(INSTRUCTIONS));
    }
}
