//! Smoke tests for the `fastembed` provider.
//!
//! Tests that download the model (~60MB) are `#[ignore]` by default.
//! Run with: `cargo test -p vestige-embed --features fastembed -- --ignored`

#![cfg(feature = "fastembed")]

use vestige_embed::{build_provider, EmbeddingsConfig, FastembedProvider};

fn fastembed_config(model: Option<&str>) -> EmbeddingsConfig {
    EmbeddingsConfig {
        provider: "fastembed".to_string(),
        model: model.map(str::to_string),
        dimensions: None,
    }
}

/// The provider must construct without any network access (lazy init).
#[test]
fn fastembed_provider_constructs_without_network() {
    let result = FastembedProvider::new("bge-small-en-v1.5");
    assert!(
        result.is_ok(),
        "FastembedProvider::new should succeed (no network needed), err: {:?}",
        result.err()
    );
}

/// `dimensions()` is known statically — no model download needed.
#[test]
fn fastembed_dimensions_are_384_for_default_model() {
    use vestige_embed::EmbeddingProvider;

    let provider = FastembedProvider::new("bge-small-en-v1.5").unwrap();
    assert_eq!(provider.dimensions(), 384);
}

/// The factory builds the default model correctly.
#[test]
fn build_provider_fastembed_default_constructs() {
    let result = build_provider(&fastembed_config(None));
    assert!(
        result.is_ok(),
        "build_provider(fastembed) should succeed, err: {:?}",
        result.err()
    );
    let provider = result.unwrap();
    assert_eq!(provider.provider_name(), "fastembed");
    assert_eq!(provider.dimensions(), 384);
}

/// An unrecognised model name returns `ModelNotAvailable`.
#[test]
fn fastembed_unknown_model_returns_error() {
    use vestige_embed::EmbedError;

    let result = FastembedProvider::new("totally-unknown-model-xyz");
    assert!(
        matches!(result, Err(EmbedError::ModelNotAvailable { .. })),
        "unknown model should return ModelNotAvailable"
    );
}

/// Full round-trip: embed a string and verify the vector shape and properties.
///
/// Downloads the model on first run (~60MB). Marked `#[ignore]` for CI.
#[test]
#[ignore = "downloads ~60MB model on first run; run with --ignored to execute"]
fn fastembed_embed_returns_correct_dimensions() {
    use vestige_embed::EmbeddingProvider;

    let provider = FastembedProvider::new("bge-small-en-v1.5").unwrap();
    let vector = provider.embed("hello world").expect("embed should succeed");

    assert_eq!(vector.len(), 384, "vector must have 384 dimensions");
}

/// Identical inputs must produce identical outputs.
///
/// Requires model download. Marked `#[ignore]` for CI.
#[test]
#[ignore = "downloads ~60MB model on first run; run with --ignored to execute"]
fn fastembed_embed_is_deterministic() {
    use vestige_embed::EmbeddingProvider;

    let provider = FastembedProvider::new("bge-small-en-v1.5").unwrap();

    let first = provider.embed("determinism test").unwrap();
    let second = provider.embed("determinism test").unwrap();

    assert_eq!(
        first, second,
        "embedding must be deterministic for the same input"
    );
}

/// `dimensions()` must match the actual vector length returned by `embed()`.
///
/// Requires model download. Marked `#[ignore]` for CI.
#[test]
#[ignore = "downloads ~60MB model on first run; run with --ignored to execute"]
fn fastembed_dimensions_matches_actual_output_length() {
    use vestige_embed::EmbeddingProvider;

    let provider = FastembedProvider::new("bge-small-en-v1.5").unwrap();
    let vector = provider.embed("dimension check").unwrap();

    assert_eq!(
        vector.len(),
        provider.dimensions(),
        "embed output length must equal dimensions()"
    );
}
