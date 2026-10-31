//! Request building and response parsing for the supported model providers. Nothing in this
//! module touches the network.

use serde_json::{json, Value};
use zellij_utils::input::ai_summary::{AiProvider, AiSummaryConfig};

pub const MAX_OUTPUT_TOKENS: u32 = 60;
pub const MAX_BLURB_CHARS: usize = 80;

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

// the headers carry the API key, and the body carries session content: neither may end up in logs
impl std::fmt::Debug for ProviderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRequest")
            .field("url", &self.url)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("body_len", &self.body.len())
            .finish()
    }
}

pub fn build_request(
    provider: AiProvider,
    model: &str,
    api_key: &str,
    prompt: &str,
) -> ProviderRequest {
    // both providers accept the same minimal chat shape
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_OUTPUT_TOKENS,
        "messages": [{ "role": "user", "content": prompt }],
    });
    if provider == AiProvider::Claude {
        // Claude models think by default, which would use up the whole token budget
        body["thinking"] = json!({ "type": "disabled" });
    }
    let body = body.to_string();
    let content_type = ("content-type".to_owned(), "application/json".to_owned());
    match provider {
        AiProvider::Claude => ProviderRequest {
            url: "https://api.anthropic.com/v1/messages".to_owned(),
            headers: vec![
                ("x-api-key".to_owned(), api_key.to_owned()),
                ("anthropic-version".to_owned(), "2023-06-01".to_owned()),
                content_type,
            ],
            body,
        },
        AiProvider::DeepSeek => ProviderRequest {
            url: "https://api.deepseek.com/chat/completions".to_owned(),
            headers: vec![
                ("authorization".to_owned(), format!("Bearer {}", api_key)),
                content_type,
            ],
            body,
        },
    }
}

/// Extracts the generated text from a provider response body, `None` if the response is an
/// error or isn't shaped as expected.
pub fn parse_response(provider: AiProvider, body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let text = match provider {
        AiProvider::Claude => value["content"]
            .as_array()?
            .iter()
            .find(|block| block["type"] == "text")?["text"]
            .as_str()?,
        AiProvider::DeepSeek => value["choices"][0]["message"]["content"].as_str()?,
    };
    Some(text.to_owned())
}

/// Asks the configured provider for a blurb. `send` performs the HTTP request and returns the
/// response body. Any failure (disabled config, transport error, error response) is `None`:
/// a missing blurb must never get in the way of the session list.
pub fn summarize<F>(
    config: &AiSummaryConfig,
    api_key: &str,
    prompt: &str,
    send: F,
) -> Option<String>
where
    F: FnOnce(&ProviderRequest) -> Result<String, String>,
{
    let provider = config.provider?;
    let model = config.effective_model()?;
    let request = build_request(provider, &model, api_key, prompt);
    let response = send(&request).ok()?;
    clean_blurb(&parse_response(provider, &response)?)
}

/// Reduces model output to a single short line, `None` if nothing usable is left.
pub fn clean_blurb(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line = line
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim_end_matches('.')
        .trim();
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(MAX_BLURB_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn body_of(request: &ProviderRequest) -> Value {
        serde_json::from_str(&request.body).unwrap()
    }

    fn header<'a>(request: &'a ProviderRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn claude_requests_target_the_messages_api() {
        let request = build_request(AiProvider::Claude, "some-model", "KEY123", "describe this");
        assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(header(&request, "x-api-key"), Some("KEY123"));
        assert_eq!(header(&request, "anthropic-version"), Some("2023-06-01"));
        assert_eq!(header(&request, "content-type"), Some("application/json"));
        let body = body_of(&request);
        assert_eq!(body["model"], "some-model");
        assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "describe this");
    }

    #[test]
    fn claude_requests_turn_extended_thinking_off() {
        // Haiku 5.5 thinks by default, and a one-line summary has a tiny token budget: the
        // thinking would use all of it and leave no text in the reply
        let request = build_request(AiProvider::Claude, "m", "key", "p");
        assert_eq!(body_of(&request)["thinking"]["type"], "disabled");
    }

    #[test]
    fn deepseek_requests_have_no_thinking_setting() {
        let request = build_request(AiProvider::DeepSeek, "m", "key", "p");
        assert!(body_of(&request).get("thinking").is_none());
    }

    #[test]
    fn a_reply_that_starts_with_a_thinking_block_still_yields_the_text() {
        let body = r#"{"content":[{"type":"thinking","thinking":"","signature":"abc"},{"type":"text","text":"Fixing tests"}]}"#;
        assert_eq!(
            parse_response(AiProvider::Claude, body).as_deref(),
            Some("Fixing tests")
        );
    }

    #[test]
    fn deepseek_requests_target_the_chat_completions_api() {
        let request = build_request(
            AiProvider::DeepSeek,
            "deepseek-chat",
            "KEY123",
            "describe this",
        );
        assert_eq!(request.url, "https://api.deepseek.com/chat/completions");
        assert_eq!(header(&request, "authorization"), Some("Bearer KEY123"));
        assert_eq!(header(&request, "content-type"), Some("application/json"));
        let body = body_of(&request);
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["max_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "describe this");
    }

    #[test]
    fn debug_output_never_contains_the_api_key() {
        for provider in [AiProvider::Claude, AiProvider::DeepSeek] {
            let request = build_request(provider, "m", "SUPER-SECRET-KEY", "p");
            assert!(!format!("{:?}", request).contains("SUPER-SECRET-KEY"));
        }
    }

    #[test]
    fn claude_responses_are_parsed() {
        let body = r#"{"id":"msg_1","content":[{"type":"text","text":"Building zellij"}],"stop_reason":"end_turn"}"#;
        assert_eq!(
            parse_response(AiProvider::Claude, body).as_deref(),
            Some("Building zellij")
        );
    }

    #[test]
    fn deepseek_responses_are_parsed() {
        let body = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Editing keymaps"}}]}"#;
        assert_eq!(
            parse_response(AiProvider::DeepSeek, body).as_deref(),
            Some("Editing keymaps")
        );
    }

    #[test]
    fn error_and_malformed_responses_yield_nothing() {
        let claude_error =
            r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#;
        let deepseek_error =
            r#"{"error":{"message":"Authentication Fails","type":"authentication_error"}}"#;
        assert_eq!(parse_response(AiProvider::Claude, claude_error), None);
        assert_eq!(parse_response(AiProvider::DeepSeek, deepseek_error), None);
        assert_eq!(parse_response(AiProvider::Claude, "not json"), None);
        assert_eq!(parse_response(AiProvider::DeepSeek, ""), None);
        assert_eq!(
            parse_response(AiProvider::Claude, r#"{"content":[]}"#),
            None
        );
        assert_eq!(
            parse_response(AiProvider::DeepSeek, r#"{"choices":[]}"#),
            None
        );
    }

    #[test]
    fn blurbs_are_reduced_to_one_clean_line() {
        assert_eq!(
            clean_blurb("  Fixing the build \n"),
            Some("Fixing the build".to_owned())
        );
        assert_eq!(
            clean_blurb("\"Fixing the build.\""),
            Some("Fixing the build".to_owned())
        );
        assert_eq!(
            clean_blurb("Fixing the build\nand also other things"),
            Some("Fixing the build".to_owned())
        );
        assert_eq!(
            clean_blurb("\n\nSecond line wins"),
            Some("Second line wins".to_owned())
        );
    }

    #[test]
    fn blurbs_are_capped_in_length_without_splitting_characters() {
        let long = "é".repeat(MAX_BLURB_CHARS + 20);
        let cleaned = clean_blurb(&long).unwrap();
        assert_eq!(cleaned.chars().count(), MAX_BLURB_CHARS);
    }

    fn config_for(provider: AiProvider) -> AiSummaryConfig {
        AiSummaryConfig {
            provider: Some(provider),
            ..Default::default()
        }
    }

    const CLAUDE_REPLY: &str = r#"{"content":[{"type":"text","text":"\"Building zellij.\""}]}"#;

    #[test]
    fn summarize_returns_the_cleaned_blurb() {
        let blurb = summarize(&config_for(AiProvider::Claude), "key", "prompt", |_| {
            Ok(CLAUDE_REPLY.to_owned())
        });
        assert_eq!(blurb.as_deref(), Some("Building zellij"));
    }

    #[test]
    fn summarize_sends_the_provider_default_model_and_the_prompt() {
        let sent = std::cell::RefCell::new(None);
        summarize(
            &config_for(AiProvider::DeepSeek),
            "key",
            "my prompt",
            |request| {
                *sent.borrow_mut() = Some(request.clone());
                Err("stop here".to_owned())
            },
        );
        let request = sent.into_inner().unwrap();
        let body = body_of(&request);
        assert_eq!(request.url, "https://api.deepseek.com/chat/completions");
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["messages"][0]["content"], "my prompt");
    }

    #[test]
    fn summarize_honors_a_configured_model() {
        let mut config = config_for(AiProvider::Claude);
        config.model = Some("custom-model".to_owned());
        let sent = std::cell::RefCell::new(None);
        summarize(&config, "key", "p", |request| {
            *sent.borrow_mut() = Some(request.clone());
            Err("stop here".to_owned())
        });
        assert_eq!(
            body_of(&sent.into_inner().unwrap())["model"],
            "custom-model"
        );
    }

    #[test]
    fn summarize_yields_nothing_when_the_transport_fails() {
        let blurb = summarize(&config_for(AiProvider::Claude), "key", "p", |_| {
            Err("connection refused".to_owned())
        });
        assert_eq!(blurb, None);
    }

    #[test]
    fn summarize_yields_nothing_for_an_error_response() {
        let blurb = summarize(&config_for(AiProvider::Claude), "key", "p", |_| {
            Ok(r#"{"type":"error","error":{"message":"bad key"}}"#.to_owned())
        });
        assert_eq!(blurb, None);
    }

    #[test]
    fn summarize_does_nothing_when_disabled() {
        let called = std::cell::Cell::new(false);
        let blurb = summarize(&AiSummaryConfig::default(), "key", "p", |_| {
            called.set(true);
            Ok(CLAUDE_REPLY.to_owned())
        });
        assert_eq!(blurb, None);
        assert!(!called.get());
    }

    #[test]
    fn empty_blurbs_yield_nothing() {
        assert_eq!(clean_blurb(""), None);
        assert_eq!(clean_blurb("  \n \n"), None);
        assert_eq!(clean_blurb("\"\""), None);
    }
}
