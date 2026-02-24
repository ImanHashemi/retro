use crate::config::AiConfig;
use crate::errors::CoreError;
use crate::models::ClaudeCliOutput;
use std::io::Write;
use std::process::Command;
use super::backend::{AnalysisBackend, BackendResponse};

/// AI backend that spawns `claude -p` in non-interactive mode.
pub struct ClaudeCliBackend {
    model: String,
}

impl ClaudeCliBackend {
    pub fn new(config: &AiConfig) -> Self {
        Self {
            model: config.model.clone(),
        }
    }

    /// Check if the claude CLI is available on PATH.
    pub fn is_available() -> bool {
        Command::new("claude")
            .arg("--version")
            .env_remove("CLAUDECODE")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl AnalysisBackend for ClaudeCliBackend {
    fn execute(&self, prompt: &str, json_schema: Option<&str>) -> Result<BackendResponse, CoreError> {
        // Pipe prompt via stdin to avoid ARG_MAX limits on large prompts.
        // --tools "" disables all tool use — we only need a plain JSON response,
        // and agent-mode tool planning can consume output tokens causing truncation.
        // When --json-schema is used, the CLI needs at least 2 turns
        // (internally uses a tool call), and --tools "" must be omitted
        // to avoid conflicting with constrained decoding.
        let max_turns = if json_schema.is_some() { "2" } else { "1" };
        let mut args = vec![
            "-p",
            "-",
            "--output-format",
            "json",
            "--model",
            &self.model,
            "--max-turns",
            max_turns,
        ];
        if let Some(schema) = json_schema {
            args.push("--json-schema");
            args.push(schema);
        } else {
            // Only disable tools when not using --json-schema
            args.push("--tools");
            args.push("");
        }
        let mut child = Command::new("claude")
            .args(&args)
            // Clear CLAUDECODE to avoid nested-session rejection when retro
            // is invoked from a post-commit hook inside a Claude Code session.
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                CoreError::Analysis(format!(
                    "failed to spawn claude CLI: {e}. Is claude installed and on PATH?"
                ))
            })?;

        // Write prompt to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).map_err(|e| {
                CoreError::Analysis(format!("failed to write prompt to claude stdin: {e}"))
            })?;
            // stdin is dropped here, closing the pipe
        }

        let output = child
            .wait_with_output()
            .map_err(|e| CoreError::Analysis(format!("claude CLI execution failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::Analysis(format!(
                "claude CLI exited with {}: {}",
                output.status, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse the JSON wrapper
        let cli_output: ClaudeCliOutput = serde_json::from_str(&stdout).map_err(|e| {
            CoreError::Analysis(format!(
                "failed to parse claude CLI output: {e}\nraw output: {}",
                truncate_for_error(&stdout)
            ))
        })?;

        if cli_output.is_error {
            let error_text = cli_output.result.unwrap_or_else(|| "unknown error".to_string());
            return Err(CoreError::Analysis(format!(
                "claude CLI returned error: {}",
                error_text
            )));
        }

        let input_tokens = cli_output.total_input_tokens();
        let output_tokens = cli_output.total_output_tokens();

        // When --json-schema is used, the structured JSON appears in
        // `structured_output` (as a parsed JSON value) rather than `result`.
        // Serialize it back to a string for downstream parsing.
        let result_text = cli_output
            .structured_output
            .map(|v| serde_json::to_string(&v).unwrap_or_default())
            .filter(|s| !s.is_empty())
            .or_else(|| cli_output.result.filter(|s| !s.is_empty()))
            .ok_or_else(|| {
                CoreError::Analysis(format!(
                    "claude CLI returned empty result (is_error={}, duration_ms={}, tokens_in={}, tokens_out={})",
                    cli_output.is_error,
                    cli_output.duration_ms,
                    input_tokens,
                    output_tokens,
                ))
            })?;

        Ok(BackendResponse {
            text: result_text,
            input_tokens,
            output_tokens,
        })
    }
}

fn truncate_for_error(s: &str) -> &str {
    if s.len() <= 500 {
        s
    } else {
        let mut i = 500;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        &s[..i]
    }
}
