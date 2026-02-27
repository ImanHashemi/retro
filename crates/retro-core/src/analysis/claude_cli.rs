use crate::config::AiConfig;
use crate::errors::CoreError;
use crate::models::ClaudeCliOutput;
use std::io::{Read, Write};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};
use super::backend::{AnalysisBackend, BackendResponse};

/// Maximum time to wait for a single `claude -p` call before killing it.
const EXECUTE_TIMEOUT_SECS: u64 = 300; // 5 minutes

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

    /// Pre-flight auth check: sends a minimal prompt WITHOUT --json-schema
    /// (which returns immediately on auth failure) and checks is_error.
    /// This prevents the infinite StructuredOutput retry loop that occurs
    /// when --json-schema is used with an expired/missing auth token.
    pub fn check_auth() -> Result<(), CoreError> {
        let output = Command::new("claude")
            .args(["-p", "ping", "--output-format", "json", "--max-turns", "1", "--tools", ""])
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| CoreError::Analysis(format!("auth check failed to spawn: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(cli_output) = serde_json::from_str::<ClaudeCliOutput>(&stdout) {
            if cli_output.is_error {
                let msg = cli_output.result.unwrap_or_default();
                return Err(CoreError::Analysis(format!(
                    "claude CLI auth failed: {msg}"
                )));
            }
        } else if !output.status.success() {
            // Couldn't parse JSON — fall back to checking stderr/stdout for auth errors
            let all_output = format!("{}{}", stdout, String::from_utf8_lossy(&output.stderr));
            if all_output.contains("Not logged in") || all_output.contains("/login") {
                return Err(CoreError::Analysis(
                    "claude CLI is not authenticated. Run `claude /login` first.".to_string()
                ));
            }
            return Err(CoreError::Analysis(format!(
                "claude CLI auth check failed with exit code {}: {}",
                output.status, all_output.trim()
            )));
        }

        Ok(())
    }
}

/// Maximum time to wait for an agentic `claude -p` call (codebase exploration).
const AGENTIC_TIMEOUT_SECS: u64 = 600; // 10 minutes

impl ClaudeCliBackend {
    /// Execute an agentic prompt: unlimited turns, full tool access, raw markdown output.
    ///
    /// Key differences from `execute()`:
    /// - No `--max-turns` (unlimited turns for codebase exploration)
    /// - No `--tools ""` (model needs tool access)
    /// - No `--json-schema` (output is raw markdown)
    /// - Longer timeout: 600 seconds (10 minutes)
    /// - Result comes from `result` field (not `structured_output`)
    /// - Optional `cwd` to set the working directory for tool calls
    pub fn execute_agentic(&self, prompt: &str, cwd: Option<&str>) -> Result<BackendResponse, CoreError> {
        let args = vec![
            "-p",
            "-",
            "--output-format",
            "json",
            "--model",
            &self.model,
        ];

        let mut cmd = Command::new("claude");
        cmd.args(&args)
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| {
            CoreError::Analysis(format!(
                "failed to spawn claude CLI (agentic): {e}. Is claude installed and on PATH?"
            ))
        })?;

        // Write prompt to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).map_err(|e| {
                CoreError::Analysis(format!("failed to write prompt to claude stdin: {e}"))
            })?;
        }

        // Read stdout/stderr in background threads to prevent pipe deadlock
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let stderr_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let timeout = Duration::from_secs(AGENTIC_TIMEOUT_SECS);
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(CoreError::Analysis(format!(
                            "claude CLI agentic call timed out after {}s — killed process.",
                            AGENTIC_TIMEOUT_SECS
                        )));
                    }
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => {
                    return Err(CoreError::Analysis(format!(
                        "error waiting for claude CLI (agentic): {e}"
                    )));
                }
            }
        };

        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            return Err(CoreError::Analysis(format!(
                "claude CLI (agentic) exited with {}: {}",
                status, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes);

        // Parse the JSON wrapper
        let cli_output: ClaudeCliOutput = serde_json::from_str(&stdout).map_err(|e| {
            CoreError::Analysis(format!(
                "failed to parse claude CLI agentic output: {e}\nraw output: {}",
                truncate_for_error(&stdout)
            ))
        })?;

        if cli_output.is_error {
            let error_text = cli_output.result.unwrap_or_else(|| "unknown error".to_string());
            return Err(CoreError::Analysis(format!(
                "claude CLI (agentic) returned error: {}",
                error_text
            )));
        }

        let input_tokens = cli_output.total_input_tokens();
        let output_tokens = cli_output.total_output_tokens();

        // Agentic calls: result comes from `result` field (no --json-schema used)
        let result_text = cli_output
            .result
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CoreError::Analysis(format!(
                    "claude CLI (agentic) returned empty result (is_error={}, num_turns={}, duration_ms={}, tokens_in={}, tokens_out={})",
                    cli_output.is_error,
                    cli_output.num_turns,
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

impl AnalysisBackend for ClaudeCliBackend {
    fn execute(&self, prompt: &str, json_schema: Option<&str>) -> Result<BackendResponse, CoreError> {
        // Pipe prompt via stdin to avoid ARG_MAX limits on large prompts.
        //
        // When --json-schema is used:
        //   - --tools "" is omitted because it conflicts with the internal
        //     constrained-decoding tool call on large prompts.
        //   - --max-turns 5 gives the model room for tool calls (which it
        //     sometimes makes when tools aren't disabled) plus the final
        //     structured output turn. With --max-turns 2, the model
        //     intermittently exhausts turns on tool calls before producing
        //     structured_output, leaving both result and structured_output empty.
        //
        // When --json-schema is NOT used:
        //   - --tools "" disables all tool use (we only need a plain response).
        //   - --max-turns 1 is sufficient since there are no tool calls.
        let max_turns = if json_schema.is_some() { "5" } else { "1" };
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

        // Read stdout/stderr in background threads to prevent pipe deadlock,
        // then poll the child with a timeout to kill runaway processes
        // (e.g., the CLI's internal StructuredOutput retry loop).
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });
        let stderr_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let timeout = Duration::from_secs(EXECUTE_TIMEOUT_SECS);
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(CoreError::Analysis(format!(
                            "claude CLI timed out after {}s — killed process. \
                             This may indicate a StructuredOutput retry loop in the CLI.",
                            EXECUTE_TIMEOUT_SECS
                        )));
                    }
                    thread::sleep(Duration::from_millis(500));
                }
                Err(e) => {
                    return Err(CoreError::Analysis(format!(
                        "error waiting for claude CLI: {e}"
                    )));
                }
            }
        };

        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            return Err(CoreError::Analysis(format!(
                "claude CLI exited with {}: {}",
                status, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes);

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
                    "claude CLI returned empty result (is_error={}, num_turns={}, duration_ms={}, tokens_in={}, tokens_out={})",
                    cli_output.is_error,
                    cli_output.num_turns,
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
