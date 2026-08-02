//! OpenAI extraction provider — calls the Chat Completions API.
//!
//! Gated behind the `openai` cargo feature. Reads the API key from the `OPENAI_API_KEY`
//! environment variable at call time. Blocking `reqwest` client.

use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use vestige_core::NormalizedTurn;

use crate::error::ExtractError;
use crate::prompt::{parse_response, render_transcript, SYSTEM_PROMPT};
use crate::provider::{ExtractedCandidate, ExtractionProvider};

const API_URL: &str = "https://api.openai.com/v1/chat/completions";
const ENV_KEY: &str = "OPENAI_API_KEY";

// === TYPES ===

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    response_format: ResponseFormat,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    r#type: &'static str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: String,
}

/// Extraction provider backed by the OpenAI Chat Completions API.
pub struct OpenAiExtractionProvider {
    model: String,
    client: Client,
}

// === PUBLIC API ===

impl OpenAiExtractionProvider {
    /// Create a new provider for the given OpenAI model id.
    pub fn new(model: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self { model, client }
    }
}

impl ExtractionProvider for OpenAiExtractionProvider {
    fn provider_name(&self) -> &'static str {
        "openai"
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
            response_format: ResponseFormat {
                r#type: "json_object",
            },
        };

        let response = self
            .client
            .post(API_URL)
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .map_err(|err| ExtractError::Network(format!("OpenAI API unreachable: {err}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            return Err(ExtractError::Backend(format!(
                "OpenAI returned HTTP {status}: {text}"
            )));
        }

        let parsed: ChatResponse = response.json().map_err(|err| {
            ExtractError::Backend(format!("malformed response from OpenAI: {err}"))
        })?;

        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        parse_response(&text)
    }
}
