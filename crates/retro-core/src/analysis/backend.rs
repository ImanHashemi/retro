use crate::errors::CoreError;

/// Response from an AI backend call.
pub struct BackendResponse {
    /// The AI's response text (inner result extracted from wrapper).
    pub text: String,
    /// Cost of the API call in USD.
    pub cost_usd: f64,
}

/// Trait for AI analysis backends. Sync only — no async.
pub trait AnalysisBackend {
    /// Execute a prompt and return the response text and cost.
    fn execute(&self, prompt: &str) -> Result<BackendResponse, CoreError>;
}
