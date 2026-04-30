//! Ollama embedding provider — delegates to a locally-running Ollama instance.
//!
//! Gated behind the `ollama` cargo feature. Uses a blocking `reqwest` client
//! so it fits the synchronous `EmbeddingProvider` trait without requiring async.

use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use crate::error::EmbedError;
use crate::provider::EmbeddingProvider;

// === TYPES ===

/// JSON body sent to `POST /api/embeddings` on the Ollama daemon.
#[derive(Debug, Serialize)]
struct OllamaEmbedRequest<'a> {
    /// Ollama model name (e.g. `"nomic-embed-text"`).
    model: &'a str,
    /// The text to embed.
    prompt: &'a str,
}

/// JSON response body returned by the Ollama `/api/embeddings` endpoint.
#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    /// The embedding vector — length must match [`OllamaProvider::dimensions`].
    embedding: Vec<f32>,
}

/// Embedding provider that calls a locally-running Ollama instance.
///
/// Default base URL: `http://localhost:11434`.
/// Default model: `nomic-embed-text` (768 dims).
pub struct OllamaProvider {
    base_url: String,
    model: String,
    dimensions: usize,
    client: Client,
}

// === PUBLIC API ===

impl OllamaProvider {
    /// Create a new provider.
    ///
    /// # Arguments
    /// - `base_url` — Ollama base URL, e.g. `"http://localhost:11434"`.
    /// - `model` — Ollama model name, e.g. `"nomic-embed-text"`.
    /// - `dimensions` — Expected output dimension count. Must match the model.
    pub fn new(base_url: String, model: String, dimensions: usize) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            base_url,
            model,
            dimensions,
            client,
        }
    }

    /// Create a provider with default settings: `nomic-embed-text` at localhost.
    pub fn default_local() -> Self {
        Self::new(
            "http://localhost:11434".to_string(),
            "nomic-embed-text".to_string(),
            768,
        )
    }
}

impl EmbeddingProvider for OllamaProvider {
    /// Returns `"ollama"` — the stable provider key stored in the database.
    fn provider_name(&self) -> &'static str {
        "ollama"
    }

    /// Returns the Ollama model name (e.g. `"nomic-embed-text"`).
    fn model_name(&self) -> &str {
        &self.model
    }

    /// Returns the expected output dimension count as configured at construction.
    ///
    /// This must match the actual model; a mismatch will cause silent corruption
    /// in the vector index. The default (`768`) matches `nomic-embed-text`.
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Send a single embedding request to the Ollama daemon.
    ///
    /// Requires a running Ollama instance at the configured `base_url`.
    /// Returns [`EmbedError::EmptyInput`] for empty strings and
    /// [`EmbedError::Backend`] for HTTP errors or malformed responses.
    /// Network failures (daemon not running) surface as [`EmbedError::Backend`]
    /// with a hint to check if Ollama is running.
    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbedError> {
        if input.is_empty() {
            return Err(EmbedError::EmptyInput);
        }

        let url = format!("{}/api/embeddings", self.base_url);

        tracing::debug!(
            model = %self.model,
            url = %url,
            "Sending embed request to Ollama"
        );

        let body = OllamaEmbedRequest {
            model: &self.model,
            prompt: input,
        };

        let response = self.client.post(&url).json(&body).send().map_err(|err| {
            EmbedError::Backend(format!(
                "Ollama not reachable at {}: {}. Is Ollama running?",
                self.base_url, err
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(EmbedError::Backend(format!(
                "Ollama returned HTTP {status}: {body}"
            )));
        }

        let parsed: OllamaEmbedResponse = response
            .json()
            .map_err(|err| EmbedError::Backend(format!("malformed response from Ollama: {err}")))?;

        if parsed.embedding.is_empty() {
            return Err(EmbedError::Backend(
                "Ollama returned an empty embedding vector".to_string(),
            ));
        }

        Ok(parsed.embedding)
    }
}
