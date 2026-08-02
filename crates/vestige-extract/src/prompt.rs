//! Shared prompt construction and response parsing for LLM extraction backends.
//!
//! The `ollama`, `anthropic`, and `openai` providers differ only in transport; they all
//! send the same instruction + transcript and parse the same JSON response shape. Keeping
//! that logic here means one place to tune the prompt and one place to parse — the backends
//! stay thin HTTP adapters.

use std::str::FromStr;

use vestige_core::{MemoryType, NormalizedTurn};

use crate::error::ExtractError;
use crate::provider::ExtractedCandidate;

/// The system instruction sent to every LLM backend.
///
/// Asks for a strict JSON object so the response can be parsed deterministically.
pub const SYSTEM_PROMPT: &str = "\
You extract durable, reusable project memories from a slice of a coding-agent transcript.
Return ONLY a JSON object of the form:
{\"candidates\": [{\"type\": \"<decision|note|preference|observation|open_question>\", \"body\": \"<one self-contained memory>\", \"rationale\": \"<why it is worth keeping>\", \"confidence\": <0.0-1.0>}]}
Rules:
- Keep only facts, decisions, preferences, and open questions that stay true beyond this session.
- Skip greetings, tool noise, transient debugging chatter, and anything secret.
- Each body must be self-contained — readable without the transcript.
- If nothing is worth keeping, return {\"candidates\": []}.";

/// Render a batch of turns into the user-message text handed to the model.
pub fn render_transcript(turns: &[NormalizedTurn]) -> String {
    let mut out = String::from("Transcript slice:\n\n");
    for t in turns {
        out.push_str(&t.role);
        out.push_str(": ");
        out.push_str(t.text.trim());
        out.push('\n');
    }
    out
}

/// Parse a model's raw text response into [`ExtractedCandidate`]s.
///
/// Tolerates models that wrap the JSON in prose or markdown fences by extracting the first
/// `{ … }` span. Rows with an unknown `type` or empty `body` are skipped rather than failing
/// the whole batch. Returns [`ExtractError::Backend`] only when no JSON object can be found
/// at all.
pub fn parse_response(raw: &str) -> Result<Vec<ExtractedCandidate>, ExtractError> {
    let json_span = extract_json_object(raw)
        .ok_or_else(|| ExtractError::Backend(format!("no JSON object in model response: {raw}")))?;

    let parsed: ResponseEnvelope = serde_json::from_str(json_span)
        .map_err(|e| ExtractError::Backend(format!("malformed extraction JSON: {e}")))?;

    let candidates = parsed
        .candidates
        .into_iter()
        .filter_map(|row| {
            let body = row.body.trim().to_string();
            if body.is_empty() {
                return None;
            }
            let proposed_type = MemoryType::from_str(row.r#type.trim()).ok()?;
            let rationale = row
                .rationale
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty());
            Some(ExtractedCandidate {
                proposed_type,
                body,
                rationale,
                confidence: row.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
            })
        })
        .collect();

    Ok(candidates)
}

// === PRIVATE ===

#[derive(serde::Deserialize)]
struct ResponseEnvelope {
    #[serde(default)]
    candidates: Vec<ResponseRow>,
}

#[derive(serde::Deserialize)]
struct ResponseRow {
    r#type: String,
    body: String,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    confidence: Option<f32>,
}

/// Return the substring from the first `{` to its matching `}` (brace-balanced),
/// skipping braces inside JSON string literals. `None` if no balanced object exists.
fn extract_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_response() {
        let raw = r#"{"candidates":[{"type":"decision","body":"Use SQLite","rationale":"durability","confidence":0.9}]}"#;
        let out = parse_response(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].proposed_type, MemoryType::Decision);
        assert_eq!(out[0].body, "Use SQLite");
        assert_eq!(out[0].confidence, 0.9);
    }

    #[test]
    fn tolerates_markdown_fence_and_prose() {
        let raw = "Sure! Here you go:\n```json\n{\"candidates\":[{\"type\":\"note\",\"body\":\"x\"}]}\n```";
        let out = parse_response(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].proposed_type, MemoryType::Note);
        assert_eq!(out[0].confidence, 0.5); // defaulted
    }

    #[test]
    fn skips_unknown_type_and_empty_body() {
        let raw = r#"{"candidates":[
            {"type":"frobnicate","body":"x"},
            {"type":"note","body":"   "},
            {"type":"preference","body":"tabs not spaces"}
        ]}"#;
        let out = parse_response(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].proposed_type, MemoryType::Preference);
    }

    #[test]
    fn empty_candidates_is_ok() {
        let out = parse_response(r#"{"candidates":[]}"#).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn no_json_object_is_backend_error() {
        assert!(matches!(
            parse_response("I refuse"),
            Err(ExtractError::Backend(_))
        ));
    }

    #[test]
    fn confidence_clamped() {
        let raw = r#"{"candidates":[{"type":"note","body":"x","confidence":5.0}]}"#;
        let out = parse_response(raw).unwrap();
        assert_eq!(out[0].confidence, 1.0);
    }

    #[test]
    fn brace_inside_string_does_not_break_extraction() {
        let raw = r#"{"candidates":[{"type":"note","body":"has } brace"}]}"#;
        let out = parse_response(raw).unwrap();
        assert_eq!(out[0].body, "has } brace");
    }
}
