//! Core `EmbeddingProvider` trait — the contract every backend must satisfy.
use crate::error::EmbedError;

/// Converts text into a fixed-dimension `Vec<f32>` embedding vector.
///
/// Implementations must be `Send + Sync` so they can be shared across threads
/// inside a Tokio runtime.
pub trait EmbeddingProvider: Send + Sync {
    /// Short identifier for the provider (e.g. `"fake"`, `"ollama"`).
    fn provider_name(&self) -> &'static str;

    /// Name of the model in use (e.g. `"deterministic-sha256"`, `"nomic-embed-text"`).
    fn model_name(&self) -> &str;

    /// Number of dimensions in every output vector.
    fn dimensions(&self) -> usize;

    /// Embed a single text string.
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbedError>;

    /// Embed a batch of strings. Default: sequential calls to [`Self::embed`].
    fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        inputs.iter().map(|s| self.embed(s)).collect()
    }
}
