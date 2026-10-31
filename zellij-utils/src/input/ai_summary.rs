use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use serde::{Deserialize, Serialize};

use super::config::ConfigError;

pub const DEFAULT_CLAUDE_MODEL: &str = "claude-haiku-5-5";
pub const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-chat";
/// `fm`'s name for the on-device Apple Foundation Model.
pub const DEFAULT_APPLE_MODEL: &str = "system";
pub const DEFAULT_CLAUDE_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
pub const DEFAULT_DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
pub const DEFAULT_INTERVAL_SECS: u64 = 300;
pub const DEFAULT_SCREEN_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AiProvider {
    Claude,
    DeepSeek,
    /// The on-device Apple Foundation Model, run through the macOS `fm` command. Nothing leaves
    /// the machine, so no API key is involved.
    Apple,
}

impl AiProvider {
    fn as_str(&self) -> &'static str {
        match self {
            AiProvider::Claude => "claude",
            AiProvider::DeepSeek => "deepseek",
            AiProvider::Apple => "apple",
        }
    }
}

/// Settings for the optional AI-written session blurbs shown by `zellij attach` and
/// `zellij ls`. The feature is off until a `provider` is set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct AiSummaryConfig {
    pub provider: Option<AiProvider>,
    pub model: Option<String>,
    /// Name of the environment variable holding the API key. The key itself is never
    /// stored in the config.
    pub api_key_env: Option<String>,
    pub interval_secs: Option<u64>,
    pub include_screen_text: Option<bool>,
    pub screen_lines: Option<usize>,
}

fn kdl_error(message: String, node: &KdlNode) -> ConfigError {
    ConfigError::new_kdl_error(message, node.span().offset(), node.span().len())
}

fn first_value(node: &KdlNode) -> Option<&KdlValue> {
    node.entries()
        .iter()
        .find(|e| e.name().is_none())
        .map(|e| e.value())
}

fn string_arg(node: &KdlNode) -> Result<String, ConfigError> {
    first_value(node)
        .and_then(|v| v.as_string())
        .map(|s| s.to_owned())
        .ok_or_else(|| {
            kdl_error(
                format!("{} must have a string value", node.name().value()),
                node,
            )
        })
}

fn positive_int_arg(node: &KdlNode) -> Result<u64, ConfigError> {
    match first_value(node).and_then(|v| v.as_i64()) {
        Some(v) if v > 0 => Ok(v as u64),
        _ => Err(kdl_error(
            format!("{} must be a positive number", node.name().value()),
            node,
        )),
    }
}

impl AiSummaryConfig {
    pub fn is_enabled(&self) -> bool {
        self.provider.is_some()
    }

    pub fn effective_model(&self) -> Option<String> {
        let provider = self.provider?;
        Some(self.model.clone().unwrap_or_else(|| match provider {
            AiProvider::Claude => DEFAULT_CLAUDE_MODEL.to_owned(),
            AiProvider::DeepSeek => DEFAULT_DEEPSEEK_MODEL.to_owned(),
            AiProvider::Apple => DEFAULT_APPLE_MODEL.to_owned(),
        }))
    }

    /// Whether the provider is reached over the network with a key. False when disabled.
    pub fn needs_api_key(&self) -> bool {
        matches!(
            self.provider,
            Some(AiProvider::Claude) | Some(AiProvider::DeepSeek)
        )
    }

    /// The variable the API key is read from. `None` when disabled, or when the provider has
    /// no key (the on-device one).
    pub fn effective_api_key_env(&self) -> Option<String> {
        match self.provider? {
            AiProvider::Claude => Some(
                self.api_key_env
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CLAUDE_API_KEY_ENV.to_owned()),
            ),
            AiProvider::DeepSeek => Some(
                self.api_key_env
                    .clone()
                    .unwrap_or_else(|| DEFAULT_DEEPSEEK_API_KEY_ENV.to_owned()),
            ),
            AiProvider::Apple => None,
        }
    }

    pub fn effective_interval_secs(&self) -> u64 {
        self.interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS)
    }

    pub fn effective_include_screen_text(&self) -> bool {
        self.include_screen_text.unwrap_or(true)
    }

    pub fn effective_screen_lines(&self) -> usize {
        self.screen_lines.unwrap_or(DEFAULT_SCREEN_LINES)
    }

    pub fn from_kdl(kdl: &KdlNode) -> Result<Self, ConfigError> {
        let mut config = AiSummaryConfig::default();
        let Some(children) = kdl.children() else {
            return Ok(config);
        };
        for node in children.nodes() {
            match node.name().value() {
                "provider" => {
                    config.provider = Some(match string_arg(node)?.as_str() {
                        "claude" => AiProvider::Claude,
                        "deepseek" => AiProvider::DeepSeek,
                        "apple" => AiProvider::Apple,
                        _ => {
                            return Err(kdl_error(
                                "provider must be \"claude\", \"deepseek\" or \"apple\"".to_owned(),
                                node,
                            ))
                        },
                    });
                },
                "model" => config.model = Some(string_arg(node)?),
                "api_key_env" => config.api_key_env = Some(string_arg(node)?),
                "interval_secs" => config.interval_secs = Some(positive_int_arg(node)?),
                "screen_lines" => config.screen_lines = Some(positive_int_arg(node)? as usize),
                "include_screen_text" => {
                    config.include_screen_text =
                        Some(first_value(node).and_then(|v| v.as_bool()).ok_or_else(|| {
                            kdl_error("include_screen_text must be true or false".to_owned(), node)
                        })?);
                },
                other => {
                    return Err(kdl_error(
                        format!("Unknown ai_summary setting \"{}\"", other),
                        node,
                    ))
                },
            }
        }
        Ok(config)
    }

    pub fn to_kdl(&self) -> Option<KdlNode> {
        if self == &AiSummaryConfig::default() {
            return None;
        }
        let mut node = KdlNode::new("ai_summary");
        let mut children = KdlDocument::new();
        let mut push = |name: &str, value: KdlValue| {
            let mut child = KdlNode::new(name);
            child.push(KdlEntry::new(value));
            children.nodes_mut().push(child);
        };
        if let Some(provider) = self.provider {
            push("provider", KdlValue::from(provider.as_str().to_owned()));
        }
        if let Some(model) = &self.model {
            push("model", KdlValue::from(model.clone()));
        }
        if let Some(api_key_env) = &self.api_key_env {
            push("api_key_env", KdlValue::from(api_key_env.clone()));
        }
        if let Some(interval_secs) = self.interval_secs {
            push("interval_secs", KdlValue::from(interval_secs as i64));
        }
        if let Some(include_screen_text) = self.include_screen_text {
            push("include_screen_text", KdlValue::from(include_screen_text));
        }
        if let Some(screen_lines) = self.screen_lines {
            push("screen_lines", KdlValue::from(screen_lines as i64));
        }
        node.set_children(children);
        Some(node)
    }

    pub fn merge(&self, other: AiSummaryConfig) -> Self {
        AiSummaryConfig {
            provider: other.provider.or(self.provider),
            model: other.model.or_else(|| self.model.clone()),
            api_key_env: other.api_key_env.or_else(|| self.api_key_env.clone()),
            interval_secs: other.interval_secs.or(self.interval_secs),
            include_screen_text: other.include_screen_text.or(self.include_screen_text),
            screen_lines: other.screen_lines.or(self.screen_lines),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kdl::KdlDocument;

    fn parse(text: &str) -> Result<AiSummaryConfig, ConfigError> {
        let document: KdlDocument = text.parse().unwrap();
        AiSummaryConfig::from_kdl(document.get("ai_summary").unwrap())
    }

    #[test]
    fn full_block_is_parsed() {
        let config = parse(
            r#"
            ai_summary {
                provider "deepseek"
                model "deepseek-chat"
                api_key_env "MY_KEY"
                interval_secs 120
                include_screen_text false
                screen_lines 10
            }
            "#,
        )
        .unwrap();
        assert_eq!(config.provider, Some(AiProvider::DeepSeek));
        assert_eq!(config.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(config.api_key_env.as_deref(), Some("MY_KEY"));
        assert_eq!(config.interval_secs, Some(120));
        assert_eq!(config.include_screen_text, Some(false));
        assert_eq!(config.screen_lines, Some(10));
    }

    #[test]
    fn disabled_without_a_provider() {
        let config = parse("ai_summary {\n}").unwrap();
        assert!(!config.is_enabled());
        assert_eq!(config.effective_model(), None);
        assert_eq!(config.effective_api_key_env(), None);
    }

    #[test]
    fn defaults_follow_the_provider() {
        let claude = parse("ai_summary {\n provider \"claude\"\n}").unwrap();
        assert!(claude.is_enabled());
        assert_eq!(
            claude.effective_model().as_deref(),
            Some(DEFAULT_CLAUDE_MODEL)
        );
        assert_eq!(
            claude.effective_api_key_env().as_deref(),
            Some(DEFAULT_CLAUDE_API_KEY_ENV)
        );
        assert_eq!(claude.effective_interval_secs(), DEFAULT_INTERVAL_SECS);
        assert_eq!(claude.effective_screen_lines(), DEFAULT_SCREEN_LINES);
        assert!(claude.effective_include_screen_text());

        let deepseek = parse("ai_summary {\n provider \"deepseek\"\n}").unwrap();
        assert_eq!(
            deepseek.effective_model().as_deref(),
            Some(DEFAULT_DEEPSEEK_MODEL)
        );
        assert_eq!(
            deepseek.effective_api_key_env().as_deref(),
            Some(DEFAULT_DEEPSEEK_API_KEY_ENV)
        );
    }

    #[test]
    fn apple_runs_on_device_with_no_api_key() {
        let apple = parse("ai_summary {\n provider \"apple\"\n}").unwrap();
        assert_eq!(apple.provider, Some(AiProvider::Apple));
        assert!(apple.is_enabled());
        // `fm`'s name for the on-device model
        assert_eq!(
            apple.effective_model().as_deref(),
            Some(DEFAULT_APPLE_MODEL)
        );
        assert_eq!(DEFAULT_APPLE_MODEL, "system");
        // nothing leaves the machine, so there is no key to look up
        assert_eq!(apple.effective_api_key_env(), None);
        assert!(!apple.needs_api_key());
    }

    #[test]
    fn network_providers_need_an_api_key() {
        for provider in ["claude", "deepseek"] {
            let config = parse(&format!("ai_summary {{\n provider \"{}\"\n}}", provider)).unwrap();
            assert!(config.needs_api_key(), "{} needs a key", provider);
        }
        assert!(!parse("ai_summary {\n}").unwrap().needs_api_key());
    }

    #[test]
    fn apple_round_trips_through_kdl() {
        let config = parse("ai_summary {\n provider \"apple\"\n}").unwrap();
        let reparsed = AiSummaryConfig::from_kdl(&config.to_kdl().unwrap()).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn bad_values_are_rejected() {
        assert!(parse("ai_summary {\n provider \"gpt\"\n}").is_err());
        assert!(parse("ai_summary {\n interval_secs 0\n}").is_err());
        assert!(parse("ai_summary {\n screen_lines -3\n}").is_err());
        assert!(parse("ai_summary {\n include_screen_text \"yes\"\n}").is_err());
        assert!(parse("ai_summary {\n api_key \"sk-secret\"\n}").is_err());
    }

    #[test]
    fn to_kdl_round_trips() {
        let config = parse(
            r#"
            ai_summary {
                provider "claude"
                model "some-model"
                api_key_env "MY_KEY"
                interval_secs 60
                include_screen_text true
                screen_lines 5
            }
            "#,
        )
        .unwrap();
        let node = config.to_kdl().unwrap();
        let reparsed = AiSummaryConfig::from_kdl(&node).unwrap();
        assert_eq!(config, reparsed);
        assert_eq!(AiSummaryConfig::default().to_kdl(), None);
    }

    #[test]
    fn merge_prefers_the_other_config() {
        let base = parse("ai_summary {\n provider \"claude\"\n interval_secs 60\n}").unwrap();
        let other = parse("ai_summary {\n provider \"deepseek\"\n}").unwrap();
        let merged = base.merge(other);
        assert_eq!(merged.provider, Some(AiProvider::DeepSeek));
        assert_eq!(merged.interval_secs, Some(60));
    }
}
