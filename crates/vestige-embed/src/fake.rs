//! Deterministic hash-based embedding provider for testing.
//!
//! Never needs a model download or network access. Produces L2-normalised
//! vectors that are stable across runs for the same (input, dimensions) pair.

use sha2::{Digest, Sha256};

use crate::error::EmbedError;
use crate::provider::EmbeddingProvider;

// === TYPES ===

/// A deterministic embedding provider that derives vectors from SHA-256 hashes.
///
/// Suitable for unit and integration tests. Not semantically meaningful —
/// two similar texts will produce dissimilar vectors.
pub struct FakeEmbeddingProvider {
    dimensions: usize,
}

// === PUBLIC API ===

impl FakeEmbeddingProvider {
    /// Create a provider with the given output dimension count.
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

impl Default for FakeEmbeddingProvider {
    fn default() -> Self {
        Self::new(64)
    }
}

impl EmbeddingProvider for FakeEmbeddingProvider {
    fn provider_name(&self) -> &'static str {
        "fake"
    }

    fn model_name(&self) -> &str {
        "deterministic-sha256"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, input: &str) -> Result<Vec<f32>, EmbedError> {
        if input.is_empty() {
            return Err(EmbedError::EmptyInput);
        }

        let raw = derive_embedding_bytes(input, self.dimensions);
        let vector = bytes_to_f32_vector(&raw);
        let normalised = l2_normalise(vector);

        Ok(normalised)
    }
}

// === PRIVATE HELPERS ===

/// Produce `dimensions * 2` bytes by hashing the input and tiling the digest.
///
/// Each pair of bytes is later interpreted as a u16 and mapped to a well-formed
/// f32 in [-1, 1], avoiding NaN/Inf bit patterns.
fn derive_embedding_bytes(input: &str, dimensions: usize) -> Vec<u8> {
    let digest = Sha256::digest(input.as_bytes());
    let digest_bytes = digest.as_slice(); // 32 bytes

    let total_bytes = dimensions * 2;
    let mut raw = Vec::with_capacity(total_bytes);
    while raw.len() < total_bytes {
        let remaining = total_bytes - raw.len();
        let take = remaining.min(digest_bytes.len());
        raw.extend_from_slice(&digest_bytes[..take]);
    }
    raw
}

/// Interpret a byte slice as `f32` values in the range [-1.0, 1.0].
///
/// We map each byte pair (0–65535) to a float in [-1, 1] rather than
/// interpreting raw bit patterns, which can produce NaN/Inf and break
/// L2 normalisation.
fn bytes_to_f32_vector(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| {
            let value = u16::from_le_bytes([chunk[0], chunk[1]]) as f32;
            // Map [0, 65535] → [-1.0, 1.0]
            (value / 32767.5) - 1.0
        })
        .collect()
}

/// L2-normalise a vector in place. Returns the normalised vector.
///
/// Panics (in debug) if the vector is all-zero — tiling a SHA-256 digest
/// guarantees this never happens in practice.
fn l2_normalise(mut vector: Vec<f32>) -> Vec<f32> {
    let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();

    // SHA-256 output is never all-zero, so this path is only reachable if
    // the tiling logic is broken.
    if norm == 0.0 {
        tracing::error!("FakeEmbeddingProvider produced an all-zero vector before normalisation");
        return vector;
    }

    for v in &mut vector {
        *v /= norm;
    }
    vector
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determinism_same_input_same_vector() {
        let provider = FakeEmbeddingProvider::default();
        let a = provider.embed("hello world").unwrap();
        let b = provider.embed("hello world").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn dimension_count_is_respected() {
        let dims = 128;
        let provider = FakeEmbeddingProvider::new(dims);
        let vector = provider.embed("test").unwrap();
        assert_eq!(vector.len(), dims);
    }

    #[test]
    fn default_dimensions_are_64() {
        let provider = FakeEmbeddingProvider::default();
        assert_eq!(provider.dimensions(), 64);
        let vector = provider.embed("anything").unwrap();
        assert_eq!(vector.len(), 64);
    }

    #[test]
    fn l2_norm_is_approximately_one() {
        let provider = FakeEmbeddingProvider::default();
        let vector = provider.embed("normalisation check").unwrap();
        let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "L2 norm was {norm}, expected ~1.0"
        );
    }

    #[test]
    fn empty_input_returns_error() {
        let provider = FakeEmbeddingProvider::default();
        let result = provider.embed("");
        assert!(matches!(result, Err(EmbedError::EmptyInput)));
    }

    #[test]
    fn different_inputs_produce_different_vectors() {
        let provider = FakeEmbeddingProvider::default();
        let a = provider.embed("apple").unwrap();
        let b = provider.embed("orange").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn vector_is_never_all_zero() {
        let provider = FakeEmbeddingProvider::default();
        let vector = provider.embed("zero check").unwrap();
        let any_nonzero = vector.iter().any(|&v| v != 0.0);
        assert!(
            any_nonzero,
            "vector must not be all-zero after normalisation"
        );
    }
}
