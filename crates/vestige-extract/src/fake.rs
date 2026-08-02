//! Deterministic extraction provider for testing.
//!
//! Never needs a model or network access. Emits one [`ExtractedCandidate`] per turn whose
//! trimmed text meets a minimum length, so tests get a predictable, stable candidate count
//! for a given transcript fixture.

use vestige_core::{MemoryType, NormalizedTurn};

use crate::error::ExtractError;
use crate::provider::{ExtractedCandidate, ExtractionProvider};

// === TYPES ===

/// A deterministic extraction provider that proposes a `Note` per qualifying turn.
///
/// Suitable for unit and integration tests. Not semantically meaningful — it does no
/// real summarisation; it simply mirrors each turn over a length threshold into a
/// candidate so the ingestion pipeline can be exercised end-to-end without a model.
pub struct FakeExtractionProvider {
    /// Minimum trimmed-text length for a turn to yield a candidate.
    min_chars: usize,
}

// === PUBLIC API ===

impl FakeExtractionProvider {
    /// Create a provider that yields a candidate for every turn with at least `min_chars`
    /// characters of trimmed text.
    pub fn new(min_chars: usize) -> Self {
        Self { min_chars }
    }
}

impl Default for FakeExtractionProvider {
    fn default() -> Self {
        Self::new(1)
    }
}

impl ExtractionProvider for FakeExtractionProvider {
    fn provider_name(&self) -> &'static str {
        "fake"
    }

    fn model_name(&self) -> &str {
        "deterministic"
    }

    fn extract(&self, turns: &[NormalizedTurn]) -> Result<Vec<ExtractedCandidate>, ExtractError> {
        if turns.is_empty() {
            return Err(ExtractError::EmptyInput);
        }

        let candidates = turns
            .iter()
            .filter(|t| t.text.trim().chars().count() >= self.min_chars)
            .map(|t| {
                let body = truncate_chars(t.text.trim(), 200);
                ExtractedCandidate {
                    proposed_type: MemoryType::Note,
                    body,
                    rationale: Some(format!("fake extraction from {} turn", t.role)),
                    confidence: 0.5,
                }
            })
            .collect();

        Ok(candidates)
    }
}

// === PRIVATE HELPERS ===

/// Truncate to at most `max` characters (codepoints, not bytes), appending nothing.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, text: &str) -> NormalizedTurn {
        NormalizedTurn {
            role: role.to_string(),
            text: text.to_string(),
            line: 1,
        }
    }

    #[test]
    fn empty_batch_returns_error() {
        let p = FakeExtractionProvider::default();
        assert!(matches!(p.extract(&[]), Err(ExtractError::EmptyInput)));
    }

    #[test]
    fn one_candidate_per_qualifying_turn() {
        let p = FakeExtractionProvider::new(3);
        let turns = vec![
            turn("user", "hello there"),
            turn("assistant", "ok"), // 2 chars — below threshold, skipped
            turn("user", "we decided to use SQLite"),
        ];
        let out = p.extract(&turns).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].proposed_type, MemoryType::Note);
        assert!(out[0].body.starts_with("hello"));
    }

    #[test]
    fn deterministic_across_runs() {
        let p = FakeExtractionProvider::default();
        let turns = vec![turn("user", "remember this fact")];
        assert_eq!(p.extract(&turns).unwrap(), p.extract(&turns).unwrap());
    }

    #[test]
    fn body_truncated_to_200_chars() {
        let p = FakeExtractionProvider::default();
        let long = "x".repeat(500);
        let out = p.extract(&[turn("user", &long)]).unwrap();
        assert_eq!(out[0].body.chars().count(), 200);
    }
}
