//! Deterministic representation derivation (PRD §11.3).
//!
//! Converts a raw memory body into the four [`RepresentationDepth`] variants
//! without any I/O, LLM calls, or allocation beyond the returned struct. V0
//! derivation uses sentence and word boundaries only; richer compression is
//! deferred to a post-V0 rewrite pass. All callers are expected to re-derive
//! when the body changes and compare against `content_hash` to detect drift.

use crate::types::RepresentationDepth;

/// Title is capped at 60 chars to fit in list views and agent summaries.
const MAX_TITLE_CHARS: usize = 60;

/// Output of [`derive()`] — the four text representations and a derived title.
///
/// The title is a display-only label (≤ 60 chars) and is **not** one of the
/// four [`RepresentationDepth`] variants. Use [`depth_pick`] to get the content
/// for a given depth.
pub struct DerivedRepresentations {
    /// Short display label, ≤ 60 chars, truncated at a word boundary.
    /// Derived from the first sentence of the body.
    pub title: String,
    /// First sentence of the body, trimmed. Maps to [`RepresentationDepth::OneLiner`].
    pub one_liner: String,
    /// Full trimmed body. Maps to [`RepresentationDepth::Summary`].
    pub summary: String,
    /// V0: same as `summary`. Reserved for LLM-compressed form in a later pass.
    /// Maps to [`RepresentationDepth::Compressed`].
    pub compressed: String,
    /// Full trimmed body without any modification. Maps to [`RepresentationDepth::Full`].
    pub full: String,
}

/// Derive all four representations from a raw memory body. Pure — no I/O.
///
/// Body is trimmed before processing. The first sentence (up to `.`, `!`, `?`,
/// or `\n`) becomes `one_liner`. A ≤ 60-char word-boundary truncation of that
/// sentence becomes `title`. All three `summary`, `compressed`, and `full` hold
/// the full trimmed body in V0 — later milestones will differentiate them.
pub fn derive(body: &str) -> DerivedRepresentations {
    let trimmed = body.trim();
    let title = derive_title(trimmed);
    let one_liner = first_sentence(trimmed).to_string();
    DerivedRepresentations {
        title,
        one_liner,
        summary: trimmed.to_string(),
        compressed: trimmed.to_string(),
        full: trimmed.to_string(),
    }
}

/// Select the text for a given [`RepresentationDepth`] from a
/// [`DerivedRepresentations`] value. Companion to [`derive()`].
pub fn depth_pick(d: RepresentationDepth, r: &DerivedRepresentations) -> &str {
    match d {
        RepresentationDepth::OneLiner => &r.one_liner,
        RepresentationDepth::Summary => &r.summary,
        RepresentationDepth::Compressed => &r.compressed,
        RepresentationDepth::Full => &r.full,
    }
}

// === PRIVATE HELPERS ===

/// Produce a title ≤ `MAX_TITLE_CHARS` chars from the body's first sentence,
/// truncating at the last word boundary that fits.
fn derive_title(body: &str) -> String {
    let candidate = first_sentence(body);
    if candidate.chars().count() <= MAX_TITLE_CHARS {
        return candidate.to_string();
    }
    truncate_at_word(candidate, MAX_TITLE_CHARS)
}

/// Return a borrow of the text up to (but not including) the first sentence
/// terminator (`.`, `!`, `?`, or `\n`), trimmed of surrounding whitespace.
/// Returns the full string when no terminator is found.
fn first_sentence(body: &str) -> &str {
    if let Some(end) = body.find(['.', '!', '?', '\n']) {
        body[..end].trim()
    } else {
        body
    }
}

/// Truncate `s` at the last complete word boundary that keeps the result
/// ≤ `max_chars` Unicode codepoints. Falls back to a hard codepoint cut for
/// a single oversized word.
fn truncate_at_word(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for word in s.split_whitespace() {
        let prospective = count + word.chars().count() + if out.is_empty() { 0 } else { 1 };
        if prospective > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        count = prospective;
    }
    if out.is_empty() {
        // single very long word — hard cut at codepoint boundary
        out.extend(s.chars().take(max_chars));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_body_keeps_full_text_as_title() {
        let d = derive("MCP is a thin adapter.");
        assert_eq!(d.title, "MCP is a thin adapter");
        assert_eq!(d.one_liner, "MCP is a thin adapter");
    }

    #[test]
    fn long_body_truncates_title_at_word_boundary() {
        let body = "This is a very long sentence that definitely exceeds the sixty character title limit by quite a margin honestly.";
        let d = derive(body);
        assert!(d.title.chars().count() <= MAX_TITLE_CHARS);
        assert!(!d.title.ends_with(' '));
        assert_eq!(d.full, body);
    }

    #[test]
    fn one_liner_takes_first_sentence() {
        let body = "First sentence. Second sentence with more detail.";
        let d = derive(body);
        assert_eq!(d.one_liner, "First sentence");
    }
}
