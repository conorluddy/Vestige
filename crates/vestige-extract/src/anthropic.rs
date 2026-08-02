//! Anthropic extraction provider — calls the Claude Messages API.
//!
//! Gated behind the `anthropic` cargo feature. Reads the API key from the
//! `ANTHROPIC_API_KEY` environment variable at call time. Blocking `reqwest` client.

use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use vestige_core::NormalizedTurn;

use crate::error::ExtractError;
use crate::prompt::{parse_response, render_transcript, SYSTEM_PROMPT};
use crate::provider::{ExtractedCandidate, ExtractionProvider};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const ENV_KEY: &str = "ANTHROPIC_API_KEY";

// === TYPES ===

#[derive(Debug, Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<Message<'a>>,
}

#[derive(Debug, Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: String,
}

/// Extraction provider backed by the Anthropic Claude Messages API.
pub struct AnthropicExtractionProvider {
    model: String,
    client: Client,
}

// === PUBLIC API ===

impl AnthropicExtractionProvider {
    /// Create a new provider for the given Claude model id.
    pub fn new(model: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { model, client }
    }
}

impl ExtractionProvider for AnthropicExtractionProvider {
    fn provider_name(&self) -> &'static str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn extract(&self, turns: &[NormalizedTurn]) -> Result<Vec<ExtractedCandidate>, ExtractError> {
        if turns.is_empty() {
            return Err(ExtractError::EmptyInput);
        }
        let api_key =
            std::env::var(ENV_KEY).map_err(|_| ExtractError::MissingCredential(ENV_KEY))?;

        let user = render_transcript(turns);
        let body = MessagesRequest {
            model: &self.model,
            max_tokens: 1024,
            system: SYSTEM_PROMPT,
            messages: vec![Message {
                role: "user",
                content: &user,
            }],
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .map_err(|err| ExtractError::Network(format!("Anthropic API unreachable: {err}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(ExtractError::Backend(format!(
                "Anthropic returned HTTP {status}: {text}"
            )));
        }

        let parsed: MessagesResponse = response.json().map_err(|err| {
            ExtractError::Backend(format!("malformed response from Anthropic: {err}"))
        })?;

        let text = parsed
            .content
            .into_iter()
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");
        parse_response(&text)
    }
}
