//! Integration tests for vestige-embed: verifies that `build_provider` wires
//! up a working provider end-to-end using the public crate API.

use vestige_embed::{build_provider, EmbeddingsConfig};

fn fake_config(dimensions: Option<usize>) -> EmbeddingsConfig {
    EmbeddingsConfig {
        provider: "fake".to_string(),
        model: None,
        dimensions,
    }
}

#[test]
fn fake_provider_embeds_and_round_trips() {
    let provider = build_provider(&fake_config(None)).expect("fake provider should build");

    let vector = provider.embed("hello").expect("embed should succeed");

    assert_eq!(vector.len(), 64, "default dimensions should be 64");

    // L2-normalised — norm must be ≈ 1.0.
    let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "L2 norm should be ~1.0, got {norm}"
    );
}

#[test]
fn fake_provider_batch_embeds_correctly() {
    let provider = build_provider(&fake_config(Some(32))).expect("fake provider should build");

    let inputs = ["alpha", "beta", "gamma"];
    let batch = provider
        .embed_batch(&inputs)
        .expect("batch embed should succeed");

    assert_eq!(batch.len(), 3, "batch length must match input length");
    for (i, vector) in batch.iter().enumerate() {
        assert_eq!(vector.len(), 32, "vector {i} has wrong dimension");
    }

    // Determinism: calling again should produce the same result.
    let batch2 = provider
        .embed_batch(&inputs)
        .expect("second batch embed should succeed");
    assert_eq!(batch, batch2, "embed_batch must be deterministic");
}

#[test]
fn build_unknown_provider_returns_error() {
    use vestige_embed::EmbedError;

    let cfg = EmbeddingsConfig {
        provider: "unicorn".to_string(),
        model: None,
        dimensions: None,
    };
    let result = build_provider(&cfg);
    assert!(
        matches!(result, Err(EmbedError::UnknownProvider(_))),
        "unknown provider should return UnknownProvider error"
    );
}
