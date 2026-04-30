//! Provider factory — selects an `EmbeddingProvider` from an `EmbeddingsConfig`.

use serde::{Deserialize, Serialize};

use crate::error::EmbedError;
use crate::fake::FakeEmbeddingProvider;
use crate::provider::EmbeddingProvider;

// === TYPES ===

/// Configuration for selecting and configuring an embedding provider.
///
/// Intended to be deserialised from `.vestige/config.toml` or CLI flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    /// Which backend to use: `"fake"` | `"fastembed"` | `"ollama"`.
    pub provider: String,

    /// Model name, if the backend requires one. Ignored by `"fake"`.
    pub model: Option<String>,

    /// Output dimension count. Defaults to `64` for `"fake"`.
    pub dimensions: Option<usize>,
}

// === PUBLIC API ===

/// Instantiate an [`EmbeddingProvider`] from the given config.
///
/// Returns [`EmbedError::UnknownProvider`] for unrecognised names.
/// Returns [`EmbedError::ProviderDisabled`] when a real provider's feature flag
/// is not compiled in.
pub fn build_provider(cfg: &EmbeddingsConfig) -> Result<Box<dyn EmbeddingProvider>, EmbedError> {
    match cfg.provider.as_str() {
        "fake" => Ok(Box::new(FakeEmbeddingProvider::new(
            cfg.dimensions.unwrap_or(64),
        ))),

        "fastembed" => {
            #[cfg(feature = "fastembed")]
            {
                // PR7 fills this in.
                Err(EmbedError::Backend(
                    "fastembed not implemented yet".to_string(),
                ))
            }
            #[cfg(not(feature = "fastembed"))]
            {
                Err(EmbedError::ProviderDisabled("fastembed"))
            }
        }

        "ollama" => {
            #[cfg(feature = "ollama")]
            {
                // PR7 fills this in.
                Err(EmbedError::Backend(
                    "ollama not implemented yet".to_string(),
                ))
            }
            #[cfg(not(feature = "ollama"))]
            {
                Err(EmbedError::ProviderDisabled("ollama"))
            }
        }

        other => Err(EmbedError::UnknownProvider(other.to_string())),
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str) -> EmbeddingsConfig {
        EmbeddingsConfig {
            provider: provider.to_string(),
            model: None,
            dimensions: None,
        }
    }

    #[test]
    fn fake_provider_builds_successfully() {
        let provider = build_provider(&config("fake")).unwrap();
        assert_eq!(provider.provider_name(), "fake");
        assert_eq!(provider.dimensions(), 64);
    }

    #[test]
    fn fake_provider_respects_custom_dimensions() {
        let cfg = EmbeddingsConfig {
            provider: "fake".to_string(),
            model: None,
            dimensions: Some(128),
        };
        let provider = build_provider(&cfg).unwrap();
        assert_eq!(provider.dimensions(), 128);
    }

    #[test]
    fn unknown_provider_returns_error() {
        let result = build_provider(&config("gpt-magic"));
        assert!(matches!(result, Err(EmbedError::UnknownProvider(_))));
    }

    #[test]
    fn fastembed_disabled_without_feature() {
        // This test passes when the `fastembed` feature is NOT active (default).
        #[cfg(not(feature = "fastembed"))]
        {
            let result = build_provider(&config("fastembed"));
            assert!(matches!(
                result,
                Err(EmbedError::ProviderDisabled("fastembed"))
            ));
        }
        // If the feature is enabled, the stub Backend error is returned instead.
        #[cfg(feature = "fastembed")]
        {
            let result = build_provider(&config("fastembed"));
            assert!(matches!(result, Err(EmbedError::Backend(_))));
        }
    }

    #[test]
    fn ollama_disabled_without_feature() {
        #[cfg(not(feature = "ollama"))]
        {
            let result = build_provider(&config("ollama"));
            assert!(matches!(
                result,
                Err(EmbedError::ProviderDisabled("ollama"))
            ));
        }
        #[cfg(feature = "ollama")]
        {
            let result = build_provider(&config("ollama"));
            assert!(matches!(result, Err(EmbedError::Backend(_))));
        }
    }
}
