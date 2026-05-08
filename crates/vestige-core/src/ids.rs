//! Newtype ID wrappers — [`MemoryId`], [`ProjectId`], [`EmbeddingId`], and [`CandidateId`].
//!
//! All IDs carry a mandatory prefix (`mem_`, `proj_`, `emb_`, `cand_`) followed by a
//! ULID. The prefix check is enforced at parse time via [`FromStr`], so any
//! value of these types is proof-of-validity through the type system. Never
//! pass bare `String`s where a typed ID belongs.
//!
//! `ProjectId` deviates from pure ULID generation: callers may also construct
//! one from a pre-derived slug (git remote hash or repo-path hash) via
//! [`ProjectId::from_slug`].

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

use crate::error::CoreError;

const MEMORY_PREFIX: &str = "mem_";
const PROJECT_PREFIX: &str = "proj_";
const EMBEDDING_PREFIX: &str = "emb_";
const CANDIDATE_PREFIX: &str = "cand_";
const TRACE_PREFIX: &str = "trace_";

// === PUBLIC TYPES ===

/// A validated memory identifier of the form `mem_<ULID>`.
///
/// Construct with [`MemoryId::new`] for new memories, or parse an existing
/// string with [`FromStr`]. Rejects any string that lacks the `mem_` prefix;
/// returns [`CoreError::InvalidId`] on failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(String);

/// A validated project identifier of the form `proj_<slug-or-ULID>`.
///
/// Most code generates these via [`ProjectId::from_slug`] rather than
/// [`ProjectId::new`], because project identity is derived from a stable
/// source (git remote hash or repo-path hash) per PRD §9.3. Rejects any
/// string that lacks the `proj_` prefix; returns [`CoreError::InvalidId`]
/// on failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl MemoryId {
    /// Generate a new memory ID using a fresh ULID.
    pub fn new() -> Self {
        Self(format!("{MEMORY_PREFIX}{}", Ulid::new()))
    }

    /// Borrow the underlying string representation (e.g. for SQL bindings).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectId {
    /// Generate a new project ID using a fresh ULID. Prefer [`ProjectId::from_slug`]
    /// when identity must be derived from a stable source (git remote hash, etc.).
    pub fn new() -> Self {
        Self(format!("{PROJECT_PREFIX}{}", Ulid::new()))
    }

    /// Build a project ID from a pre-derived slug (name or hash). The `proj_`
    /// prefix is prepended automatically; do not include it in `slug`.
    ///
    /// Used by `vestige-config` after resolving the PRD §9.3 identity chain:
    /// explicit `--name` → git remote hash → repo-path hash.
    pub fn from_slug(slug: impl Into<String>) -> Self {
        Self(format!("{PROJECT_PREFIX}{}", slug.into()))
    }

    /// Borrow the underlying string representation (e.g. for SQL bindings).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for MemoryId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with(MEMORY_PREFIX) {
            return Err(CoreError::InvalidId(format!(
                "memory id must start with `{MEMORY_PREFIX}`, got `{s}`"
            )));
        }
        Ok(Self(s.to_string()))
    }
}

impl FromStr for ProjectId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with(PROJECT_PREFIX) {
            return Err(CoreError::InvalidId(format!(
                "project id must start with `{PROJECT_PREFIX}`, got `{s}`"
            )));
        }
        Ok(Self(s.to_string()))
    }
}

/// A validated embedding ID of the form `emb_<ULID>`.
///
/// Wraps a `String` to carry proof-of-validation through the type system.
/// Construct with [`EmbeddingId::new`] or parse from a string with [`FromStr`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddingId(String);

impl EmbeddingId {
    /// Generate a new embedding ID using a fresh ULID.
    pub fn new() -> Self {
        Self(format!("{EMBEDDING_PREFIX}{}", Ulid::new()))
    }

    /// Borrow the underlying string representation (e.g. for SQL bindings).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for EmbeddingId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EmbeddingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for EmbeddingId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with(EMBEDDING_PREFIX) {
            return Err(CoreError::InvalidId(format!(
                "embedding id must start with `{EMBEDDING_PREFIX}`, got `{s}`"
            )));
        }
        Ok(Self(s.to_string()))
    }
}

/// A validated candidate identifier of the form `cand_<ULID>`.
///
/// Used by the V0.2 assimilation inbox for candidates awaiting review.
/// Construct with [`CandidateId::generate`] for new candidates, or parse
/// an existing string with [`FromStr`]. Rejects any string that lacks the
/// `cand_` prefix; returns [`CoreError::InvalidCandidateId`] on failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateId(String);

impl CandidateId {
    /// Validate and wrap a pre-formed candidate ID string.
    pub fn new(id: impl Into<String>) -> Result<Self, CoreError> {
        let s = id.into();
        if !s.starts_with(CANDIDATE_PREFIX) {
            return Err(CoreError::InvalidCandidateId { value: s });
        }
        Ok(Self(s))
    }

    /// Generate a new candidate ID using a fresh ULID.
    pub fn generate() -> Self {
        Self(format!("{CANDIDATE_PREFIX}{}", Ulid::new()))
    }

    /// Borrow the underlying string representation (e.g. for SQL bindings).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CandidateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CandidateId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with(CANDIDATE_PREFIX) {
            return Err(CoreError::InvalidCandidateId {
                value: s.to_string(),
            });
        }
        Ok(Self(s.to_string()))
    }
}

/// A validated query trace identifier of the form `trace_<ULID>`.
///
/// Traces are write-only audit records produced by every search/expand/context
/// call (PRD §6.3, §8.1). Construct with [`TraceId::new`] for new traces, or
/// parse an existing string with [`FromStr`]. Rejects any string that lacks the
/// `trace_` prefix; returns [`CoreError::InvalidId`] on failure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceId(String);

impl TraceId {
    /// Generate a new trace ID using a fresh ULID.
    pub fn new() -> Self {
        Self(format!("{TRACE_PREFIX}{}", Ulid::new()))
    }

    /// Borrow the underlying string representation (e.g. for SQL bindings).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TraceId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with(TRACE_PREFIX) {
            return Err(CoreError::InvalidId(format!(
                "trace id must start with `{TRACE_PREFIX}`, got `{s}`"
            )));
        }
        Ok(Self(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_id_roundtrip() {
        let id = MemoryId::new();
        assert!(id.as_str().starts_with("mem_"));
        let parsed = MemoryId::from_str(id.as_str()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn memory_id_rejects_bad_prefix() {
        assert!(MemoryId::from_str("proj_foo").is_err());
    }

    #[test]
    fn project_id_from_slug() {
        let id = ProjectId::from_slug("vestige");
        assert_eq!(id.as_str(), "proj_vestige");
    }

    #[test]
    fn embedding_id_roundtrip() {
        let id = EmbeddingId::new();
        assert!(id.as_str().starts_with("emb_"));
        let parsed = EmbeddingId::from_str(id.as_str()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn embedding_id_rejects_bad_prefix() {
        assert!(EmbeddingId::from_str("mem_foo").is_err());
        assert!(EmbeddingId::from_str("proj_foo").is_err());
    }

    #[test]
    fn embedding_id_display_matches_as_str() {
        let id = EmbeddingId::new();
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn candidate_id_generate_has_correct_prefix() {
        let id = CandidateId::generate();
        assert!(id.as_str().starts_with("cand_"));
    }

    #[test]
    fn candidate_id_roundtrip() {
        let id = CandidateId::generate();
        let parsed = CandidateId::from_str(id.as_str()).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn candidate_id_new_validates_prefix() {
        assert!(CandidateId::new("cand_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_ok());
        assert!(matches!(
            CandidateId::new("mem_foo"),
            Err(CoreError::InvalidCandidateId { .. })
        ));
    }

    #[test]
    fn candidate_id_rejects_wrong_prefix() {
        assert!(CandidateId::from_str("mem_foo").is_err());
        assert!(CandidateId::from_str("proj_bar").is_err());
    }

    #[test]
    fn candidate_id_rejects_empty() {
        assert!(CandidateId::from_str("").is_err());
    }

    #[test]
    fn trace_id_new_has_correct_prefix() {
        let id = TraceId::new();
        assert!(id.as_str().starts_with("trace_"), "got: {}", id.as_str());
    }

    #[test]
    fn trace_id_roundtrip() {
        let id = TraceId::new();
        let parsed = TraceId::from_str(id.as_str()).unwrap();
        assert_eq!(id, parsed);
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn trace_id_rejects_wrong_prefix() {
        // Wrong prefix — `mem_` should be rejected.
        assert!(TraceId::from_str("mem_01ARZ3NDEKTSV4RRFFQ69G5FAV").is_err());
        assert!(TraceId::from_str("proj_foo").is_err());
        assert!(TraceId::from_str("cand_foo").is_err());
        assert!(TraceId::from_str("emb_foo").is_err());
    }

    #[test]
    fn trace_id_rejects_empty() {
        assert!(TraceId::from_str("").is_err());
    }

    #[test]
    fn trace_id_display_matches_as_str() {
        let id = TraceId::new();
        assert_eq!(id.to_string(), id.as_str());
    }
}
