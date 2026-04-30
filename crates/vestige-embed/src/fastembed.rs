//! FastEmbed embedding provider — local ONNX models, no network after first download.
//!
//! Gated behind the `fastembed` cargo feature. On first `embed()` call the model
//! is loaded (and downloaded ~60MB if not already cached) into a `OnceLock` so
//! subsequent calls are cheap. The constructor is always instantaneous.

use std::path::PathBuf;
use std::sync::OnceLock;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::error::EmbedError;
use crate::provider::EmbeddingProvider;

// === TYPES ===

/// Maps a model short-name (as passed via config) to a fastembed `EmbeddingModel`
/// and its output dimension count.
struct ModelSpec {
    model: EmbeddingModel,
    dimensions: usize,
}

fn resolve_model_spec(model_name: &str) -> Option<ModelSpec> {
    match model_name {
        "bge-small-en-v1.5" => Some(ModelSpec {
            model: EmbeddingModel::BGESmallENV15,
            dimensions: 384,
        }),
        "bge-base-en-v1.5" => Some(ModelSpec {
            model: EmbeddingModel::BGEBaseENV15,
            dimensions: 768,
        }),
        "bge-large-en-v1.5" => Some(ModelSpec {
            model: EmbeddingModel::BGELargeENV15,
            dimensions: 1024,
        }),
        _ => None,
    }
}

/// FastEmbed provider using local ONNX models via the `fastembed` crate.
///
/// Lazy: the model is not loaded (and not downloaded) until the first
/// call to [`embed`](FastembedProvider::embed).
pub struct FastembedProvider {
    model_name: String,
    dimensions: usize,
    embedding_model: EmbeddingModel,
    cache_dir: PathBuf,
    inner: OnceLock<TextEmbedding>,
}

// === PUBLIC API ===

impl FastembedProvider {
    /// Create a new provider for the given model short-name.
    ///
    /// Valid names: `"bge-small-en-v1.5"` (default, 384 dims),
    /// `"bge-base-en-v1.5"` (768 dims), `"bge-large-en-v1.5"` (1024 dims).
    ///
    /// The model is NOT loaded here — loading happens on the first `embed()` call.
    pub fn new(model_name: &str) -> Result<Self, EmbedError> {
        let spec = resolve_model_spec(model_name).ok_or_else(|| EmbedError::ModelNotAvailable {
            model: model_name.to_string(),
            reason:
                "unknown model name (valid: bge-small-en-v1.5, bge-base-en-v1.5, bge-large-en-v1.5)"
                    .to_string(),
        })?;

        let cache_dir = resolve_cache_dir(model_name);

        Ok(Self {
            model_name: model_name.to_string(),
            dimensions: spec.dimensions,
            embedding_model: spec.model,
            cache_dir,
            inner: OnceLock::new(),
        })
    }

    /// Return a reference to the loaded `TextEmbedding`, initialising it on first call.
    fn get_or_init_model(&self) -> Result<&TextEmbedding, EmbedError> {
        if self.inner.get().is_none() {
            tracing::info!(
                model = %self.model_name,
                cache_dir = %self.cache_dir.display(),
                "Loading fastembed model; this may download ~60MB on first use"
            );

            let options = InitOptions::new(self.embedding_model.clone())
                .with_cache_dir(self.cache_dir.clone())
                .with_show_download_progress(true);

            let model =
                TextEmbedding::try_new(options).map_err(|err| EmbedError::ModelNotAvailable {
                    model: self.model_name.clone(),
                    reason: err.to_string(),
                })?;

            // If two threads race here the loser's model is discarded and the
            // winner's value is used — no correctness issue, just a wasted load.
            let _ = self.inner.set(model);
        }

        // SAFETY: we just set it above if it was absent.
        Ok(self.inner.get().expect("model was just initialised"))
    }
}

impl EmbeddingProvider for FastembedProvider {
    fn provider_name(&self) -> &'static str {
        "fastembed"
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbedError> {
        if input.is_empty() {
            return Err(EmbedError::EmptyInput);
        }

        let model = self.get_or_init_model()?;

        let mut results = model
            .embed(vec![input], None)
            .map_err(|err| EmbedError::Backend(format!("fastembed embed failed: {err}")))?;

        results
            .pop()
            .ok_or_else(|| EmbedError::Backend("fastembed returned empty result".to_string()))
    }

    fn embed_batch(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if inputs.is_empty() {
            return Ok(vec![]);
        }

        for input in inputs {
            if input.is_empty() {
                return Err(EmbedError::EmptyInput);
            }
        }

        let model = self.get_or_init_model()?;

        model
            .embed(inputs.to_vec(), None)
            .map_err(|err| EmbedError::Backend(format!("fastembed batch embed failed: {err}")))
    }
}

// === PRIVATE HELPERS ===

/// Resolve the model cache directory: `~/.vestige/models/<model_name>/`.
///
/// Falls back to a temp-adjacent path if the home directory cannot be determined.
fn resolve_cache_dir(model_name: &str) -> PathBuf {
    let base = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    base.join(".vestige").join("models").join(model_name)
}
