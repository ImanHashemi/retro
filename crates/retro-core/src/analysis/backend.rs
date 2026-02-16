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
    fn execute(&self, prompt: &str) -> Result<BackendResponse, CoreError>;
}
