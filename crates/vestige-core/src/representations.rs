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

/// One-liner cap for run-on bodies with no sentence terminator. `first_sentence`
/// falls back to the entire body when it finds no `!?.\n`, so without this cap
/// a terminator-free body of any length would produce an unbounded `one_liner`.
/// Properly terminated sentences are never capped here, even if longer than
/// this — only the unterminated fallback case is.
const MAX_ONE_LINER_CHARS: usize = 240;

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
    /// Capped at [`MAX_ONE_LINER_CHARS`] (word-boundary truncated) when the
    /// body has no sentence terminator at all.
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
    let one_liner = cap_one_liner(first_sentence(trimmed), trimmed);
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

/// Cap `sentence` at [`MAX_ONE_LINER_CHARS`] when `first_sentence` fell back
/// to the entire (unterminated) body — recognised by `sentence` being exactly
/// as long as `trimmed_body` in bytes, which only happens when no terminator
/// was found. A properly terminated sentence is always strictly shorter than
/// `trimmed_body` (it excludes at least the terminator) and is left untouched.
fn cap_one_liner(sentence: &str, trimmed_body: &str) -> String {
    let is_unterminated_fallback = sentence.len() == trimmed_body.len();
    if is_unterminated_fallback && sentence.chars().count() > MAX_ONE_LINER_CHARS {
        truncate_at_word(sentence, MAX_ONE_LINER_CHARS)
    } else {
        sentence.to_string()
    }
}

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
/// terminator, trimmed of surrounding whitespace. Returns the full string
/// when no terminator is found.
///
/// `!`, `?`, and `\n` always terminate. `.` only terminates when followed by
/// whitespace or end-of-string — so "V0.2", "e.g.", "1.5", and "config.toml"
/// stay in the one-liner instead of being chopped at the period.
fn first_sentence(body: &str) -> &str {
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '!' | '?' | '\n' => return body[..i].trim(),
            '.' => match chars.peek().map(|(_, c)| *c) {
                None => return body[..i].trim(),
                Some(n) if n.is_whitespace() => return body[..i].trim(),
                _ => continue,
            },
            _ => continue,
        }
    }
    body
}

/// Truncate `s` at the last complete word boundary that keeps the result
/// ≤ `max_chars` Unicode codepoints. Falls back to a hard codepoint cut for
/// a single oversized word. Never splits a UTF-8 codepoint. Shared by
/// [`derive_title`], [`cap_one_liner`], and callers outside this crate
/// (e.g. `vestige-cli`'s inbox listing) that need the same word/char-safe
/// truncation.
pub fn truncate_at_word(s: &str, max_chars: usize) -> String {
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

    #[test]
    fn period_inside_token_is_not_a_sentence_break() {
        let d = derive("V0.2 ships the assimilation inbox.");
        assert_eq!(d.one_liner, "V0.2 ships the assimilation inbox");
        assert!(d.title.starts_with("V0.2"));
    }

    #[test]
    fn period_in_filename_is_not_a_sentence_break() {
        let d = derive("Edit config.toml then restart.");
        assert_eq!(d.one_liner, "Edit config.toml then restart");
    }

    #[test]
    fn decimal_numbers_stay_intact() {
        let d = derive("Confidence threshold is 0.75.");
        assert_eq!(d.one_liner, "Confidence threshold is 0.75");
    }

    #[test]
    fn exclamation_still_terminates() {
        let d = derive("Whoa! Second part.");
        assert_eq!(d.one_liner, "Whoa");
    }

    #[test]
    fn run_on_body_without_terminator_caps_one_liner_at_word_boundary() {
        // No `!?.\n` anywhere, so `first_sentence` falls back to the whole
        // body. Build a body over 240 chars purely from whitespace-separated
        // words so the cap must engage.
        let body = "word ".repeat(80); // 400 chars, no terminator
        let d = derive(&body);
        assert!(
            d.one_liner.chars().count() <= MAX_ONE_LINER_CHARS,
            "one_liner must be capped at {MAX_ONE_LINER_CHARS} chars, got {}",
            d.one_liner.chars().count()
        );
        assert!(
            !d.one_liner.ends_with(' '),
            "capped one_liner must not end with a partial trailing word/space"
        );
        assert!(
            body.starts_with(&d.one_liner),
            "capped one_liner must be a clean word-boundary prefix of the body"
        );
    }

    #[test]
    fn terminated_body_under_cap_is_unaffected_by_one_liner_cap() {
        // Sanity: a normal terminated sentence well under the cap is not
        // touched by the new capping logic at all.
        let d = derive("First sentence. Second sentence with more detail.");
        assert_eq!(d.one_liner, "First sentence");
    }
}
