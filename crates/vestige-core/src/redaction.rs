//! Secret-redaction for ingest snippets.
//!
//! Exposes a single pure function [`redact_secrets`] that scrubs secret-shaped
//! substrings from arbitrary text before it is stored as a source snippet.
//! Idempotent: running it on already-redacted text returns the same string.
//! No I/O, no allocation beyond the returned `String`.

// ============================================================
// === PUBLIC API =============================================
// ============================================================

/// Scrub secret-shaped substrings from `input`, replacing each with a fixed
/// placeholder.  Pure and idempotent: running it on already-redacted text
/// yields the same string.
///
/// # What is scrubbed
///
/// | Pattern                                  | Example input                        | Output                            |
/// |------------------------------------------|--------------------------------------|-----------------------------------|
/// | `.env`-style secret assignments          | `API_KEY=abc123`                     | `API_KEY=[REDACTED]`              |
/// | Bearer / Authorization header            | `Authorization: Bearer eyJ…`         | `Authorization: Bearer [REDACTED]`|
/// | Known API-key prefixes (`sk-`, `ghp_`, …)| `sk-proj-AAAA…`                      | `[REDACTED]`                      |
/// | AWS access-key IDs                       | `AKIAIOSFODNN7EXAMPLE`               | `[REDACTED]`                      |
/// | PEM private-key blocks                   | `-----BEGIN RSA PRIVATE KEY-----…`   | `[REDACTED PRIVATE KEY]`          |
///
/// # What is preserved
///
/// Normal prose, file paths, `port=8080`, UUIDs (8-4-4-4-12 hex with dashes),
/// 40-char git SHAs, and any text that does not match a known secret shape.
pub fn redact_secrets(input: &str) -> String {
    let mut s = input.to_owned();
    s = redact_pem_blocks(s);
    s = redact_bearer_tokens(s);
    s = redact_env_assignments(s);
    s = redact_known_prefixes(s);
    s
}

// ============================================================
// === PRIVATE HELPERS ========================================
// ============================================================

const REDACTED: &str = "[REDACTED]";
const REDACTED_KEY: &str = "[REDACTED PRIVATE KEY]";

// --- PEM private-key blocks ---------------------------------

fn redact_pem_blocks(mut s: String) -> String {
    while let Some(begin) = find_begin_pem(&s) {
        // Find the matching END marker after begin
        let end_marker_start = match s[begin..].find("-----END") {
            Some(rel) => begin + rel,
            None => {
                // Malformed / truncated block — redact from BEGIN to end-of-string
                s.replace_range(begin.., REDACTED_KEY);
                break;
            }
        };
        // Advance past `-----END … -----`
        let end_marker_end = match s[end_marker_start..].find("-----\n") {
            Some(rel) => end_marker_start + rel + 6,
            None => match s[end_marker_start..].find("-----") {
                Some(rel) => end_marker_start + rel + 5,
                None => s.len(),
            },
        };
        s.replace_range(begin..end_marker_end, REDACTED_KEY);
    }
    s
}

fn find_begin_pem(s: &str) -> Option<usize> {
    let marker = "-----BEGIN";
    let mut search_from = 0;
    while let Some(rel) = s[search_from..].find(marker) {
        let pos = search_from + rel;
        // Must contain PRIVATE KEY somewhere before the closing `-----`
        let rest = &s[pos + marker.len()..];
        if let Some(close) = rest.find("-----") {
            let header_content = &rest[..close];
            if header_content.contains("PRIVATE KEY") {
                return Some(pos);
            }
        }
        search_from = pos + marker.len();
    }
    None
}

// --- Bearer tokens ------------------------------------------

/// Handles:
///   `Bearer <token>`
///   `authorization: bearer <token>` (case-insensitive)
fn redact_bearer_tokens(s: String) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;

    loop {
        let search_region = &lower[cursor..];
        let rel = match search_region.find("bearer ") {
            Some(r) => r,
            None => break,
        };
        let bearer_pos = cursor + rel;
        // Append everything up to and including "Bearer " (preserve original case)
        let bearer_end = bearer_pos + "bearer ".len();
        out.push_str(&s[cursor..bearer_end]);

        // The token is everything up to the next whitespace / end-of-string
        let token_start = bearer_end;
        let token_len = s[token_start..]
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(s[token_start..].len());
        let raw_token = &s[token_start..token_start + token_len];

        if is_already_redacted(raw_token) {
            out.push_str(raw_token);
        } else {
            out.push_str(REDACTED);
        }

        cursor = token_start + token_len;
    }
    out.push_str(&s[cursor..]);
    out
}

// --- .env-style secret assignments --------------------------

/// Scrubs `SECRET_KEY=somevalue` → `SECRET_KEY=[REDACTED]`.
/// Only triggers when the key name contains a secret-bearing keyword.
/// Preserves benign assignments like `port=8080`, `debug=true`.
fn redact_env_assignments(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    let bytes = s.as_bytes();

    while cursor < bytes.len() {
        // Find next `=`
        let eq_rel = match s[cursor..].find('=') {
            Some(r) => r,
            None => break,
        };
        let eq_pos = cursor + eq_rel;

        // Walk backwards to collect the key name (alphanumeric + `_`)
        let key_start = key_name_start(bytes, eq_pos);
        let key = &s[key_start..eq_pos];

        if key.is_empty() || !is_secret_key_name(key) {
            // Not a secret assignment — emit up through the `=` and advance
            out.push_str(&s[cursor..eq_pos + 1]);
            cursor = eq_pos + 1;
            continue;
        }

        // Emit everything up to (and including) the key and `=`
        out.push_str(&s[cursor..eq_pos + 1]);

        // Collect the value: runs until whitespace, `"`, `'`, or end-of-string
        // but strip surrounding quotes first
        let after_eq = &s[eq_pos + 1..];
        let (value_len, strip_quote) = measure_env_value(after_eq);
        let raw_value = &after_eq[..value_len];

        if is_already_redacted(raw_value)
            || is_already_redacted(raw_value.trim_matches(strip_quote))
        {
            out.push_str(raw_value);
        } else {
            // Preserve outer quote character if present
            if strip_quote != '\0' && after_eq.starts_with(strip_quote) {
                out.push(strip_quote);
            }
            out.push_str(REDACTED);
            if strip_quote != '\0' && raw_value.ends_with(strip_quote) {
                out.push(strip_quote);
            }
        }

        cursor = eq_pos + 1 + value_len;
    }
    out.push_str(&s[cursor..]);
    out
}

fn key_name_start(bytes: &[u8], eq_pos: usize) -> usize {
    let mut i = eq_pos;
    while i > 0 {
        let b = bytes[i - 1];
        if b.is_ascii_alphanumeric() || b == b'_' {
            i -= 1;
        } else {
            break;
        }
    }
    i
}

/// Returns (total_bytes_consumed_including_quotes, quote_char_or_nul).
fn measure_env_value(s: &str) -> (usize, char) {
    if s.is_empty() {
        return (0, '\0');
    }
    let first = s.chars().next().unwrap_or('\0');
    if first == '"' || first == '\'' {
        // Quoted: consume until matching close quote (or end)
        let inner = &s[first.len_utf8()..];
        let close = inner.find(first).unwrap_or(inner.len());
        let total = first.len_utf8() + close + first.len_utf8();
        return (total.min(s.len()), first);
    }
    // Unquoted: consume until whitespace
    let len = s.find(|c: char| c.is_ascii_whitespace()).unwrap_or(s.len());
    (len, '\0')
}

/// True if the key name contains a secret-bearing keyword (case-insensitive).
fn is_secret_key_name(key: &str) -> bool {
    let lower = key.to_lowercase();
    SECRET_KEY_WORDS.iter().any(|&word| lower.contains(word))
}

const SECRET_KEY_WORDS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "passwd",
    "private_key",
    "privatekey",
    "access_key",
    "accesskey",
    "auth_key",
    "authkey",
    "credential",
    "client_secret",
];

// --- Known API-key prefixes ---------------------------------

/// Scrubs tokens with well-known prefixes that are unambiguously API keys:
///   `sk-…`  (OpenAI / Anthropic style)
///   `ghp_…` / `gho_…` / `ghs_…` / `ghr_…` / `github_pat_…`
///   `xoxb-…` / `xoxa-…` / `xoxp-…` / `xoxr-…` / `xoxs-…` (Slack)
///   `AKIA…` (AWS access key IDs — 20-char all-caps alphanumeric)
fn redact_known_prefixes(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;

    while cursor < s.len() {
        if let Some((match_start, match_len)) = find_known_prefix_token(&s, cursor) {
            out.push_str(&s[cursor..match_start]);
            let raw = &s[match_start..match_start + match_len];
            if is_already_redacted(raw) {
                out.push_str(raw);
            } else {
                out.push_str(REDACTED);
            }
            cursor = match_start + match_len;
        } else {
            break;
        }
    }
    out.push_str(&s[cursor..]);
    out
}

/// Returns `(absolute_start, length)` of the first known-prefix token at or
/// after `from`.
fn find_known_prefix_token(s: &str, from: usize) -> Option<(usize, usize)> {
    let slice = &s[from..];
    let lower = slice.to_lowercase();

    // Collect candidate positions for each prefix, pick the earliest.
    let mut best: Option<(usize, usize)> = None; // (abs_pos, len)

    for prefix in KNOWN_PREFIXES {
        let Some(rel) = lower.find(prefix) else {
            continue;
        };
        // Only match at a token boundary (start of string or preceded by non-alnum/non-hyphen/non-underscore)
        if rel > 0 {
            let prev_byte = slice.as_bytes()[rel - 1];
            if is_token_char(prev_byte) {
                continue;
            }
        }
        let abs = from + rel;
        let token_end = s[abs..]
            .find(|c: char| c.is_ascii_whitespace() || c == '"' || c == '\'' || c == ',')
            .map(|r| abs + r)
            .unwrap_or(s.len());
        let len = token_end - abs;
        // Require minimum length to avoid false positives
        if len < prefix.len() + 4 {
            continue;
        }
        if best.map_or(true, |(b, _)| abs < b) {
            best = Some((abs, len));
        }
    }

    // AWS AKIA… — all-caps 20-char identifier
    if let Some((abs, len)) = find_aws_key(s, from) {
        if best.map_or(true, |(b, _)| abs < b) {
            best = Some((abs, len));
        }
    }

    best
}

const KNOWN_PREFIXES: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxa-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "glpat-", // GitLab personal access token
    "npm_",   // npm token
    "dp.pt.", // DigitalOcean personal access token prefix
];

fn find_aws_key(s: &str, from: usize) -> Option<(usize, usize)> {
    // AWS access key IDs start with AKIA, AGPA, AIDA, AROA, AIPA, ANPA, ANVA, ASIA
    // followed by exactly 16 uppercase alphanumeric characters (total 20).
    let prefixes = [
        "AKIA", "AGPA", "AIDA", "AROA", "AIPA", "ANPA", "ANVA", "ASIA",
    ];
    let slice = &s[from..];
    let mut best: Option<(usize, usize)> = None;

    for prefix in prefixes {
        let Some(rel) = slice.find(prefix) else {
            continue;
        };
        // Token boundary check
        if rel > 0 && is_token_char(slice.as_bytes()[rel - 1]) {
            continue;
        }
        let abs = from + rel;
        let rest = &s[abs + prefix.len()..];
        // Must be followed by exactly 16 uppercase alphanumeric chars
        let suffix_len = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .count();
        if suffix_len < 16 {
            continue;
        }
        let len = prefix.len() + suffix_len.min(20);
        if best.map_or(true, |(b, _)| abs < b) {
            best = Some((abs, len));
        }
    }
    best
}

#[inline]
fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
}

#[inline]
fn is_already_redacted(s: &str) -> bool {
    s.contains(REDACTED) || s.contains(REDACTED_KEY)
}

// ============================================================
// === TESTS ==================================================
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Env-style assignments ---

    #[test]
    fn scrubs_api_key_assignment() {
        assert_eq!(redact_secrets("API_KEY=supersecret"), "API_KEY=[REDACTED]");
    }

    #[test]
    fn scrubs_secret_assignment() {
        assert_eq!(
            redact_secrets("MY_SECRET=abc123xyz"),
            "MY_SECRET=[REDACTED]"
        );
    }

    #[test]
    fn scrubs_token_assignment() {
        assert_eq!(
            redact_secrets("GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxx"),
            "GITHUB_TOKEN=[REDACTED]"
        );
    }

    #[test]
    fn scrubs_password_assignment() {
        assert_eq!(redact_secrets("PASSWORD=hunter2"), "PASSWORD=[REDACTED]");
    }

    #[test]
    fn scrubs_quoted_env_value() {
        // Surrounding quotes are preserved; the secret value within is replaced.
        assert_eq!(
            redact_secrets(r#"API_KEY="mysecret""#),
            r#"API_KEY="[REDACTED]""#
        );
    }

    #[test]
    fn preserves_benign_assignment() {
        assert_eq!(redact_secrets("port=8080"), "port=8080");
        assert_eq!(redact_secrets("debug=true"), "debug=true");
        assert_eq!(redact_secrets("max_connections=100"), "max_connections=100");
    }

    // --- Bearer tokens ---

    #[test]
    fn scrubs_bearer_token() {
        assert_eq!(
            redact_secrets("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9"),
            "Authorization: Bearer [REDACTED]"
        );
    }

    #[test]
    fn scrubs_bearer_lowercase() {
        let result = redact_secrets("curl -H 'authorization: bearer supersecrettoken123'");
        assert!(result.contains("[REDACTED]"), "got: {result}");
        assert!(!result.contains("supersecrettoken123"), "got: {result}");
    }

    // --- Known API-key prefixes ---

    #[test]
    fn scrubs_sk_prefix() {
        let input = "key: sk-proj-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"), "got: {result}");
        assert!(
            !result.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
            "got: {result}"
        );
    }

    #[test]
    fn scrubs_ghp_prefix() {
        let input = "GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz";
        let result = redact_secrets(input);
        // The env-assignment pass fires first; the result has [REDACTED] either way
        assert!(result.contains("[REDACTED]"), "got: {result}");
        assert!(
            !result.contains("abcdefghijklmnopqrstuvwxyz"),
            "got: {result}"
        );
    }

    #[test]
    fn scrubs_standalone_ghp_token() {
        // Not in a key=value context — the known-prefix pass must catch it
        let input = "token ghp_abcdefghijklmnopqrstuvwxyz";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"), "got: {result}");
    }

    #[test]
    fn scrubs_slack_token() {
        // Use a clearly-fake token (wrong segment lengths, not a real credential).
        let input = "slack_token=xoxb-FAKE-TOKEN-VALUE-FOR-TESTING-ONLY";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"), "got: {result}");
    }

    #[test]
    fn scrubs_aws_akia_key() {
        let input = "aws_key=AKIAIOSFODNN7EXAMPLE";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"), "got: {result}");
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"), "got: {result}");
    }

    #[test]
    fn scrubs_standalone_aws_key() {
        let input = "Access key: AKIAIOSFODNN7EXAMPLE";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED]"), "got: {result}");
    }

    // --- Private-key blocks ---

    #[test]
    fn scrubs_pem_private_key_block() {
        let input =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQ==\n-----END RSA PRIVATE KEY-----\n";
        let result = redact_secrets(input);
        assert_eq!(result.trim(), "[REDACTED PRIVATE KEY]");
    }

    #[test]
    fn scrubs_ec_private_key_block() {
        let input =
            "key:\n-----BEGIN EC PRIVATE KEY-----\ndeadbeef==\n-----END EC PRIVATE KEY-----";
        let result = redact_secrets(input);
        assert!(result.contains("[REDACTED PRIVATE KEY]"), "got: {result}");
        assert!(!result.contains("deadbeef"), "got: {result}");
    }

    // --- Preserved benign text ---

    #[test]
    fn preserves_normal_sentence() {
        let s = "The quick brown fox jumps over the lazy dog.";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn preserves_uuid() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(redact_secrets(uuid), uuid);
    }

    #[test]
    fn preserves_git_sha() {
        let sha = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert_eq!(redact_secrets(sha), sha);
    }

    #[test]
    fn preserves_port_assignment() {
        assert_eq!(redact_secrets("port=8080"), "port=8080");
    }

    // --- Idempotency ---

    #[test]
    fn idempotent_on_env_assignment() {
        let once = redact_secrets("API_KEY=supersecret");
        let twice = redact_secrets(&once);
        assert_eq!(once, twice, "second pass changed output");
    }

    #[test]
    fn idempotent_on_bearer() {
        let once = redact_secrets("Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
        let twice = redact_secrets(&once);
        assert_eq!(once, twice, "second pass changed output");
    }

    #[test]
    fn idempotent_on_pem_block() {
        let input = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkq==\n-----END PRIVATE KEY-----\n";
        let once = redact_secrets(input);
        let twice = redact_secrets(&once);
        assert_eq!(once, twice, "second pass changed output");
    }

    #[test]
    fn idempotent_on_aws_key() {
        let input = "AKIAIOSFODNN7EXAMPLE";
        let once = redact_secrets(input);
        let twice = redact_secrets(&once);
        assert_eq!(once, twice, "second pass changed output");
    }

    // --- UTF-8 / multibyte ---

    #[test]
    fn handles_multibyte_without_panic() {
        let input = "Héllo wörld — API_KEY=sécrét_value — end";
        let result = redact_secrets(input);
        // Secret value is scrubbed
        assert!(result.contains("[REDACTED]"), "got: {result}");
        // Non-secret multibyte text survives
        assert!(result.contains("Héllo"), "got: {result}");
        assert!(result.contains("wörld"), "got: {result}");
    }

    #[test]
    fn preserves_non_secret_multibyte() {
        let input = "Ünité testing est très important.";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn idempotent_on_multibyte_input() {
        let input = "API_KEY=sécrét — path=/usr/local/bin — port=9000";
        let once = redact_secrets(input);
        let twice = redact_secrets(&once);
        assert_eq!(once, twice);
    }
}
