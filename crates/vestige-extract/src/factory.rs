//! Provider factory — selects an `ExtractionProvider` from an `ExtractionConfig`.
//!
//! Mirrors [`vestige_embed::build_provider`] exactly: `"fake"` always builds; the real
//! backends return [`ExtractError::ProviderDisabled`] when their feature flag is absent and
//! [`ExtractError::UnknownProvider`] for unrecognised names.

use serde::{Deserialize, Serialize};

use crate::error::ExtractError;
use crate::fake::FakeExtractionProvider;
use crate::provider::ExtractionProvider;

// === TYPES ===

/// Configuration for selecting and configuring an extraction provider.
///
/// Intended to be deserialised from the `[extraction]` block of `.vestige/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    /// Which backend to use: `"fake"` | `"ollama"` | `"anthropic"` | `"openai"`.
    pub provider: String,

    /// Model name, if the backend requires one. Ignored by `"fake"`.
    pub model: Option<String>,
}

// === PUBLIC API ===

/// Instantiate an [`ExtractionProvider`] from the given config.
///
/// Returns [`ExtractError::UnknownProvider`] for unrecognised names and
/// [`ExtractError::ProviderDisabled`] when a real provider's feature flag is not compiled in.
pub fn build_provider(cfg: &ExtractionConfig) -> Result<Box<dyn ExtractionProvider>, ExtractError> {
    match cfg.provider.as_str() {
        "fake" => Ok(Box::new(FakeExtractionProvider::default())),

        "ollama" => {
            #[cfg(feature = "ollama")]
            {
                let model = cfg.model.clone().unwrap_or_else(|| "llama3.2".to_string());
                Ok(Box::new(crate::OllamaExtractionProvider::new(
                    "http://localhost:11434".to_string(),
                    model,
                )))
            }
            #[cfg(not(feature = "ollama"))]
            {
                Err(ExtractError::ProviderDisabled("ollama"))
            }
        }

        "anthropic" => {
            #[cfg(feature = "anthropic")]
            {
                let model = cfg
                    .model
                    .clone()
                    .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
                Ok(Box::new(crate::AnthropicExtractionProvider::new(model)))
            }
            #[cfg(not(feature = "anthropic"))]
            {
                Err(ExtractError::ProviderDisabled("anthropic"))
            }
        }

        "openai" => {
            #[cfg(feature = "openai")]
            {
                let model = cfg
                    .model
                    .clone()
                    .unwrap_or_else(|| "gpt-4o-mini".to_string());
                Ok(Box::new(crate::OpenAiExtractionProvider::new(model)))
            }
            #[cfg(not(feature = "openai"))]
            {
                Err(ExtractError::ProviderDisabled("openai"))
            }
        }

        other => Err(ExtractError::UnknownProvider(other.to_string())),
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str) -> ExtractionConfig {
        ExtractionConfig {
            provider: provider.to_string(),
            model: None,
        }
    }

    #[test]
    fn fake_provider_builds_successfully() {
        let p = build_provider(&config("fake")).unwrap();
        assert_eq!(p.provider_name(), "fake");
        assert_eq!(p.model_name(), "deterministic");
    }

    #[test]
    fn unknown_provider_returns_error() {
        let result = build_provider(&config("gpt-magic"));
        assert!(matches!(result, Err(ExtractError::UnknownProvider(_))));
    }

    #[test]
    fn ollama_disabled_without_feature() {
        #[cfg(not(feature = "ollama"))]
        {
            let result = build_provider(&config("ollama"));
            assert!(matches!(
                result,
                Err(ExtractError::ProviderDisabled("ollama"))
            ));
        }
        #[cfg(feature = "ollama")]
        {
            assert!(build_provider(&config("ollama")).is_ok());
        }
    }

    #[test]
    fn anthropic_disabled_without_feature() {
        #[cfg(not(feature = "anthropic"))]
        {
            let result = build_provider(&config("anthropic"));
            assert!(matches!(
                result,
                Err(ExtractError::ProviderDisabled("anthropic"))
            ));
        }
    }

    #[test]
    fn openai_disabled_without_feature() {
        #[cfg(not(feature = "openai"))]
        {
            let result = build_provider(&config("openai"));
            assert!(matches!(
                result,
                Err(ExtractError::ProviderDisabled("openai"))
            ));
        }
    }
}
