//! Ollama extraction provider — delegates to a locally-running Ollama instance.
//!
//! Gated behind the `ollama` cargo feature. Uses a blocking `reqwest` client so it fits the
//! synchronous [`ExtractionProvider`] trait without requiring async. This is the default
//! real provider for daemon-mode session-log extraction (no API key required).

use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use vestige_core::NormalizedTurn;

use crate::error::ExtractError;
use crate::prompt::{parse_response, render_transcript, SYSTEM_PROMPT};
use crate::provider::{ExtractedCandidate, ExtractionProvider};

// === TYPES ===

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    /// Ask Ollama to constrain output to a JSON object.
    format: &'a str,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
}

/// Extraction provider that calls a locally-running Ollama instance.
pub struct OllamaExtractionProvider {
    base_url: String,
    model: String,
    client: Client,
}

// === PUBLIC API ===

impl OllamaExtractionProvider {
    /// Create a new provider for the given Ollama base URL and model name.
    pub fn new(base_url: String, model: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self {
            base_url,
            model,
            client,
        }
    }
}

impl ExtractionProvider for OllamaExtractionProvider {
    fn provider_name(&self) -> &'static str {
        "ollama"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn extract(&self, turns: &[NormalizedTurn]) -> Result<Vec<ExtractedCandidate>, ExtractError> {
        if turns.is_empty() {
            return Err(ExtractError::EmptyInput);
        }

        let user = render_transcript(turns);
        let url = format!("{}/api/chat", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: &user,
                },
            ],
            stream: false,
            format: "json",
        };

        let response = self.client.post(&url).json(&body).send().map_err(|err| {
            ExtractError::Network(format!(
                "Ollama not reachable at {}: {}. Is Ollama running?",
                self.base_url, err
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(ExtractError::Backend(format!(
                "Ollama returned HTTP {status}: {text}"
            )));
        }

        let parsed: ChatResponse = response.json().map_err(|err| {
            ExtractError::Backend(format!("malformed response from Ollama: {err}"))
        })?;

        parse_response(&parsed.message.content)
    }
}
