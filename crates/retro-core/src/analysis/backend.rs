use crate::errors::CoreError;

/// Response from an AI backend call.
pub struct BackendResponse {
    /// The AI's response text (inner result extracted from wrapper).
    pub text: String,
    /// Input tokens consumed.
    pub input_tokens: u64,
    /// Output tokens produced.
    pub output_tokens: u64,
}

/// Trait for AI analysis backends. Sync only — no async.
pub trait AnalysisBackend {
    /// Execute a prompt and return the response text and cost.
    /// When `json_schema` is provided, the backend passes it to `--json-schema`
    /// for constrained decoding (guaranteed valid JSON matching the schema).
    fn execute(&self, prompt: &str, json_schema: Option<&str>) -> Result<BackendResponse, CoreError>;
}

/// Scripted backend for tests: returns canned responses in order, recording
/// prompts. Lives in production code (not cfg(test)) so retro-cli integration
/// tests and runner_v3 tests can use it too.
#[derive(Default)]
pub struct MockBackend {
    pub responses: std::sync::Mutex<Vec<String>>,
    pub prompts_seen: std::sync::Mutex<Vec<String>>,
}

impl MockBackend {
    pub fn with_responses(responses: Vec<String>) -> Self {
        MockBackend {
            responses: std::sync::Mutex::new(responses),
            prompts_seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl AnalysisBackend for MockBackend {
    fn execute(
        &self,
        prompt: &str,
        _json_schema: Option<&str>,
    ) -> Result<BackendResponse, CoreError> {
        self.prompts_seen.lock().unwrap().push(prompt.to_string());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(CoreError::Analysis("MockBackend: no responses left".to_string()));
        }
        Ok(BackendResponse {
            text: responses.remove(0),
            input_tokens: 100,
            output_tokens: 50,
        })
    }
}
