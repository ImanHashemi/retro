use regex::Regex;
use std::sync::OnceLock;

static SCRUB_PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();

fn get_patterns() -> &'static Vec<(Regex, &'static str)> {
    SCRUB_PATTERNS.get_or_init(|| {
        vec![
            // AWS access key IDs
            (Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(), "[REDACTED_AWS_KEY]"),
            // GitHub tokens
            (
                Regex::new(r"gh[ps]_[A-Za-z0-9_]{36,}").unwrap(),
                "[REDACTED_GH_TOKEN]",
            ),
            // GitHub OAuth tokens
            (
                Regex::new(r"gho_[A-Za-z0-9_]{36,}").unwrap(),
                "[REDACTED_GH_OAUTH]",
            ),
            // Generic API keys (key=..., token=..., secret=..., password=...)
            (
                Regex::new(r#"(?i)(api[_-]?key|token|secret|password|passwd|authorization)\s*[=:]\s*['"]?([A-Za-z0-9_\-./+]{16,})['"]?"#).unwrap(),
                "$1=[REDACTED]",
            ),
            // Bearer tokens
            (
                Regex::new(r"(?i)Bearer\s+[A-Za-z0-9_\-./+]{20,}").unwrap(),
                "Bearer [REDACTED]",
            ),
            // Private keys
            (
                Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----").unwrap(),
                "[REDACTED_PRIVATE_KEY]",
            ),
            // Anthropic API keys
            (
                Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").unwrap(),
                "[REDACTED_ANTHROPIC_KEY]",
            ),
            // OpenAI API keys
            (
                Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
                "[REDACTED_OPENAI_KEY]",
            ),
        ]
    })
}

/// Scrub sensitive data from text using regex patterns.
pub fn scrub_secrets(text: &str) -> String {
    let mut result = text.to_string();
    for (pattern, replacement) in get_patterns() {
        result = pattern.replace_all(&result, *replacement).to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrub_aws_key() {
        let input = "aws_key = AKIAIOSFODNN7EXAMPLE";
        let result = scrub_secrets(input);
        assert!(result.contains("[REDACTED"));
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_scrub_github_token() {
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let result = scrub_secrets(input);
        assert!(result.contains("[REDACTED"));
    }

    #[test]
    fn test_scrub_bearer() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let result = scrub_secrets(input);
        assert!(result.contains("Bearer [REDACTED]"));
    }

    #[test]
    fn test_no_scrub_normal_text() {
        let input = "This is a normal message about coding";
        let result = scrub_secrets(input);
        assert_eq!(result, input);
    }
}
