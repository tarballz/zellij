//! The real HTTP transport for `provider::summarize`.

use super::provider::ProviderRequest;
use isahc::prelude::*;
use std::time::Duration;

pub const REQUEST_TIMEOUT_SECS: u64 = 20;

/// Sends `request` and returns the response body. Errors never include the request headers or
/// body, so they are safe to log.
pub fn send_request(request: &ProviderRequest) -> Result<String, String> {
    let mut builder =
        isahc::Request::post(&request.url).timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS));
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let http_request = builder
        .body(request.body.clone())
        .map_err(|e| e.to_string())?;
    let mut response = http_request.send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.text().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::prompt::{
        build_prompt, PaneSnapshot, PromptOptions, SessionSnapshot, TabSnapshot,
    };
    use super::super::provider::summarize;
    use super::*;
    use zellij_utils::input::ai_summary::{AiProvider, AiSummaryConfig};

    /// Makes one real API call with a synthetic session. Run it explicitly:
    /// `cargo test -p zellij-server --lib live_claude_call -- --ignored --nocapture`
    #[test]
    #[ignore = "calls the real Anthropic API and needs ANTHROPIC_API_KEY"]
    fn live_claude_call_returns_a_blurb() {
        let config = AiSummaryConfig {
            provider: Some(AiProvider::Claude),
            ..Default::default()
        };
        let api_key = std::env::var(config.effective_api_key_env().unwrap())
            .expect("ANTHROPIC_API_KEY must be set");
        let snapshot = SessionSnapshot {
            session_name: "demo".to_owned(),
            tabs: vec![TabSnapshot {
                name: "editor".to_owned(),
                panes: vec![PaneSnapshot {
                    title: "nvim".to_owned(),
                    command: Some("nvim src/main.rs".to_owned()),
                    cwd: Some("/home/me/projects/web-scraper".to_owned()),
                    screen_text: Some("fn fetch_page(url: &str) -> Result<String> {".to_owned()),
                }],
            }],
        };
        let prompt = build_prompt(
            &snapshot,
            &PromptOptions {
                include_screen_text: true,
                screen_lines: 20,
                max_bytes: 4000,
            },
        );
        let blurb = summarize(&config, &api_key, &prompt, send_request);
        println!("live blurb: {:?}", blurb);
        assert!(blurb.is_some());
    }
}
