//! Smoke tests for the `ollama` provider.
//!
//! Tests that require a live Ollama instance are `#[ignore]` by default.
//! Run with: `cargo test -p vestige-embed --features ollama -- --ignored`

#![cfg(feature = "ollama")]

use vestige_embed::{
    build_provider, EmbedError, EmbeddingProvider, EmbeddingsConfig, OllamaProvider,
};

fn ollama_config(model: Option<&str>, dimensions: Option<usize>) -> EmbeddingsConfig {
    EmbeddingsConfig {
        provider: "ollama".to_string(),
        model: model.map(str::to_string),
        dimensions,
    }
}

/// The provider must construct without any network access.
#[test]
fn ollama_provider_constructs_without_network() {
    let provider = OllamaProvider::new(
        "http://localhost:11434".to_string(),
        "nomic-embed-text".to_string(),
        768,
    );
    assert_eq!(provider.provider_name(), "ollama");
    assert_eq!(provider.model_name(), "nomic-embed-text");
    assert_eq!(provider.dimensions(), 768);
}

/// `default_local()` uses sensible defaults.
#[test]
fn ollama_default_local_has_expected_defaults() {
    let provider = OllamaProvider::default_local();
    assert_eq!(provider.provider_name(), "ollama");
    assert_eq!(provider.model_name(), "nomic-embed-text");
    assert_eq!(provider.dimensions(), 768);
}

/// The factory builds an Ollama provider from config without network.
#[test]
fn build_provider_ollama_constructs_without_network() {
    let result = build_provider(&ollama_config(None, None));
    assert!(
        result.is_ok(),
        "build_provider(ollama) should succeed, err: {:?}",
        result.err()
    );

    let provider = result.unwrap();
    assert_eq!(provider.provider_name(), "ollama");
    assert_eq!(provider.dimensions(), 768);
}

/// Calling `embed()` against a definitely-dead URL returns `EmbedError::Backend`.
#[test]
fn ollama_dead_url_returns_backend_error() {
    let provider = OllamaProvider::new(
        "http://127.0.0.1:19999".to_string(), // port that should never be open
        "nomic-embed-text".to_string(),
        768,
    );

    let result = provider.embed("this should fail");
    assert!(
        matches!(result, Err(EmbedError::Backend(_))),
        "dead URL should return EmbedError::Backend, got: {result:?}"
    );
}

/// Empty input returns `EmbedError::EmptyInput` without hitting the network.
#[test]
fn ollama_empty_input_returns_error_without_network() {
    let provider = OllamaProvider::new(
        "http://127.0.0.1:19999".to_string(),
        "nomic-embed-text".to_string(),
        768,
    );

    let result = provider.embed("");
    assert!(
        matches!(result, Err(EmbedError::EmptyInput)),
        "empty input should return EmbedError::EmptyInput: {result:?}"
    );
}

/// Full round-trip with a live Ollama instance.
///
/// Requires `ollama run nomic-embed-text` to be available locally.
/// Marked `#[ignore]` for CI.
#[test]
#[ignore = "requires live Ollama instance; run with --ignored to execute"]
fn ollama_embed_returns_correct_dimensions() {
    let provider = OllamaProvider::default_local();
    let vector = provider.embed("hello world").expect("embed should succeed");

    assert_eq!(
        vector.len(),
        768,
        "nomic-embed-text should return 768-dim vectors"
    );
}

/// Identical inputs must produce identical outputs.
///
/// Requires live Ollama. Marked `#[ignore]` for CI.
#[test]
#[ignore = "requires live Ollama instance; run with --ignored to execute"]
fn ollama_embed_is_deterministic() {
    let provider = OllamaProvider::default_local();

    let first = provider.embed("determinism test").unwrap();
    let second = provider.embed("determinism test").unwrap();

    assert_eq!(
        first, second,
        "embedding must be deterministic for the same input"
    );
}
