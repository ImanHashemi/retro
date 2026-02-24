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
