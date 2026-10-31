//! Scrubs likely secrets from terminal text before it is sent to a model provider.

use once_cell::sync::Lazy;
use regex::Regex;

pub const REDACTED: &str = "[REDACTED]";

// `FOO_API_KEY=value`, `DB_PASSWORD: value` (upper-case env style, so that ordinary words
// such as "keyboard" are left alone)
static ENV_STYLE_ASSIGNMENT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b([A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD)[A-Z0-9_]*)(\s*[=:]\s*)\S+")
        .unwrap()
});
// `password=value`, `token: value`
static WORD_ASSIGNMENT: Lazy<Regex> = Lazy::new(|| {
    // the flag is scoped to the keywords: turning Unicode off for the whole pattern would let
    // `\S` match invalid UTF-8, which `Regex` (as opposed to `bytes::Regex`) rejects
    Regex::new(r"\b((?i-u:password|passwd|secret|token|api[_-]?key))(\s*[=:]\s*)\S+").unwrap()
});
static AUTHORIZATION_HEADER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"((?i-u:authorization)\s*:\s*)[^'"\r\n]+"#).unwrap());
static KNOWN_TOKEN_FORMAT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|xox[abprs]-[A-Za-z0-9-]{8,})",
    )
    .unwrap()
});
static LONG_OPAQUE_STRING: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b[A-Za-z0-9_-]{32,}\b").unwrap());

/// Replaces likely secrets in `text` with `[REDACTED]`.
pub fn redact(text: &str) -> String {
    let text = ENV_STYLE_ASSIGNMENT.replace_all(text, format!("$1$2{}", REDACTED).as_str());
    let text = WORD_ASSIGNMENT.replace_all(&text, format!("$1$2{}", REDACTED).as_str());
    let text = AUTHORIZATION_HEADER.replace_all(&text, format!("$1{}", REDACTED).as_str());
    let text = KNOWN_TOKEN_FORMAT.replace_all(&text, REDACTED);
    // an opaque blob mixing letters and digits is more likely a credential than an identifier
    let text = LONG_OPAQUE_STRING.replace_all(&text, |caps: &regex::Captures| {
        let candidate = &caps[0];
        let has_letter = candidate.chars().any(|c| c.is_ascii_alphabetic());
        let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
        if has_letter && has_digit {
            REDACTED.to_owned()
        } else {
            candidate.to_owned()
        }
    });
    text.into_owned()
}

/// Keeps only the last `n` lines of `text`.
pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Cuts `text` down to at most `max_bytes`, never splitting a character.
pub fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_looking_assignments_lose_their_value() {
        assert_eq!(
            redact("export ANTHROPIC_API_KEY=sk-ant-abc123 && run"),
            "export ANTHROPIC_API_KEY=[REDACTED] && run"
        );
        assert_eq!(redact("DB_PASSWORD=hunter2"), "DB_PASSWORD=[REDACTED]");
        assert_eq!(redact("GITHUB_TOKEN: abc"), "GITHUB_TOKEN: [REDACTED]");
    }

    #[test]
    fn authorization_headers_lose_their_credentials() {
        assert_eq!(
            redact("curl -H 'Authorization: Bearer abc.def.ghi' https://x"),
            "curl -H 'Authorization: [REDACTED]' https://x"
        );
    }

    #[test]
    fn well_known_token_formats_are_redacted() {
        for token in [
            "sk-ant-api03-AbCdEf123456",
            "ghp_0123456789abcdefghij0123456789abcdef",
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-1234-5678-abcdef",
        ] {
            let out = redact(&format!("token is {} ok", token));
            assert_eq!(out, format!("token is {} ok", REDACTED), "for {}", token);
        }
    }

    #[test]
    fn long_opaque_strings_are_redacted() {
        let blob = "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8";
        assert_eq!(
            redact(&format!("key {}", blob)),
            format!("key {}", REDACTED)
        );
    }

    #[test]
    fn ordinary_terminal_text_is_left_alone() {
        for text in [
            "cargo build --release",
            "~/code/zellij/zellij-utils/src/input/config.rs",
            "commit bf517077c (HEAD -> main)",
            "error[E0599]: no variant named `Integer`",
            "",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    #[test]
    fn last_lines_keeps_the_tail() {
        assert_eq!(last_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(last_lines("a\nb", 5), "a\nb");
        assert_eq!(last_lines("a\nb", 0), "");
    }

    #[test]
    fn truncate_bytes_never_splits_a_character() {
        assert_eq!(truncate_bytes("hello", 10), "hello");
        assert_eq!(truncate_bytes("hello", 3), "hel");
        assert_eq!(truncate_bytes("héllo", 2), "h");
    }
}
