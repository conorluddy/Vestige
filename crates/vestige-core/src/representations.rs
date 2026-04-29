use crate::types::RepresentationDepth;

const MAX_TITLE_CHARS: usize = 60;

/// Deterministically derive the four required representations (PRD §11.3) from
/// a raw memory body. V0 derivation is intentionally simple — no LLM, no
/// heuristics beyond sentence/word boundary slicing. Authors can edit any
/// individual representation later.
pub struct DerivedRepresentations {
    pub title: String,
    pub one_liner: String,
    pub summary: String,
    pub compressed: String,
    pub full: String,
}

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

pub fn depth_pick(d: RepresentationDepth, r: &DerivedRepresentations) -> &str {
    match d {
        RepresentationDepth::OneLiner => &r.one_liner,
        RepresentationDepth::Summary => &r.summary,
        RepresentationDepth::Compressed => &r.compressed,
        RepresentationDepth::Full => &r.full,
    }
}

fn derive_title(body: &str) -> String {
    let candidate = first_sentence(body);
    if candidate.chars().count() <= MAX_TITLE_CHARS {
        return candidate.to_string();
    }
    truncate_at_word(candidate, MAX_TITLE_CHARS)
}

fn first_sentence(body: &str) -> &str {
    if let Some(end) = body.find(['.', '!', '?', '\n']) {
        body[..end].trim()
    } else {
        body
    }
}

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
