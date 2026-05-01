//! Core [`EmbeddingProvider`] trait — the contract every backend must satisfy.
//!
//! New providers implement this trait; [`build_provider`](crate::factory::build_provider)
//! selects the right implementation at runtime based on [`EmbeddingsConfig`](crate::EmbeddingsConfig).
use crate::error::EmbedError;

/// Converts text into a fixed-dimension `Vec<f32>` embedding vector.
///
/// Implementations must be `Send + Sync` so they can be stored in a shared
/// context and called from multiple threads.
///
/// The three introspection methods — [`provider_name`], [`model_name`], and
/// [`dimensions`] — are recorded in the database alongside every embedding row
/// so that `vestige-store` can detect provider/model mismatches when the config
/// changes and prompt the user to re-embed.
///
/// [`provider_name`]: EmbeddingProvider::provider_name
/// [`model_name`]: EmbeddingProvider::model_name
/// [`dimensions`]: EmbeddingProvider::dimensions
pub trait EmbeddingProvider: Send + Sync {
    /// Short, stable identifier for the provider backend (e.g. `"fake"`, `"fastembed"`, `"ollama"`).
    ///
    /// This value is stored in the `embeddings` table and compared at startup to detect
    /// when the configured provider has changed since embeddings were last generated.
    fn provider_name(&self) -> &'static str;

    /// Name of the specific model in use (e.g. `"deterministic-sha256"`, `"nomic-embed-text"`).
    ///
    /// Stored alongside `provider_name` in the database. A model name change
    /// triggers the same mismatch warning as a provider change, because vectors
    /// from different models are not comparable.
    fn model_name(&self) -> &str;

    /// Number of dimensions in every output vector produced by this provider.
    ///
    /// All calls to [`embed`] and [`embed_batch`] on this instance must return
    /// vectors of exactly this length. Stored in the database; a dimension change
    /// is treated as a hard mismatch that requires a full re-embed.
    ///
    /// [`embed`]: EmbeddingProvider::embed
    /// [`embed_batch`]: EmbeddingProvider::embed_batch
    fn dimensions(&self) -> usize;

    /// Embed a single non-empty text string into a `Vec<f32>` of length [`dimensions`].
    ///
    /// # Errors
    ///
    /// - [`EmbedError::EmptyInput`] — `input` is an empty string.
    /// - [`EmbedError::ModelNotAvailable`] — the backend model could not be loaded.
    /// - [`EmbedError::Network`] — a network call to a local daemon failed (Ollama).
    /// - [`EmbedError::Backend`] — the backend returned an unexpected response.
    ///
    /// [`dimensions`]: EmbeddingProvider::dimensions
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbedError>;

    /// Embed a batch of non-empty strings.
    ///
    /// Returns a `Vec` of the same length as `inputs`, where each element is the
    /// embedding vector for the corresponding input string.
    ///
    /// The default implementation calls [`embed`] sequentially. Providers that
    /// support native batching (e.g. `fastembed`) override this for efficiency.
    ///
    /// # Errors
    ///
    /// Same variants as [`embed`]. If any individual input is empty the whole
    /// batch is rejected with [`EmbedError::EmptyInput`].
    ///
    /// [`embed`]: EmbeddingProvider::embed
    fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        inputs.iter().map(|s| self.embed(s)).collect()
    }
}
