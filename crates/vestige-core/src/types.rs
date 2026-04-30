use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::OffsetDateTime;

use crate::error::CoreError;
use crate::ids::{MemoryId, ProjectId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    Observation,
    Note,
    Decision,
    Preference,
    ProjectSummary,
    OpenQuestion,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Note => "note",
            Self::Decision => "decision",
            Self::Preference => "preference",
            Self::ProjectSummary => "project_summary",
            Self::OpenQuestion => "open_question",
        }
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryType {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "observation" => Ok(Self::Observation),
            "note" => Ok(Self::Note),
            "decision" => Ok(Self::Decision),
            "preference" => Ok(Self::Preference),
            "project_summary" => Ok(Self::ProjectSummary),
            "open_question" => Ok(Self::OpenQuestion),
            other => Err(CoreError::InvalidMemoryType(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Deleted,
}

impl MemoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

impl FromStr for MemoryStatus {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "deleted" => Ok(Self::Deleted),
            other => Err(CoreError::InvalidMemoryStatus(other.to_string())),
        }
    }
}

/// Progressive disclosure depth (PRD §5.2). L0 (handle) is just the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationDepth {
    OneLiner,
    Summary,
    Compressed,
    Full,
}

impl RepresentationDepth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OneLiner => "one_liner",
            Self::Summary => "summary",
            Self::Compressed => "compressed",
            Self::Full => "full",
        }
    }
}

impl FromStr for RepresentationDepth {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "one_liner" | "oneliner" => Ok(Self::OneLiner),
            "summary" => Ok(Self::Summary),
            "compressed" | "compressed_body" => Ok(Self::Compressed),
            "full" | "full_body" => Ok(Self::Full),
            other => Err(CoreError::InvalidDepth(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub name: String,
    pub root_path: Option<String>,
    pub git_remote: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub project_id: ProjectId,
    pub r#type: MemoryType,
    pub status: MemoryStatus,
    pub confidence: f64,
    pub importance: f64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Representation {
    pub memory_id: MemoryId,
    pub depth: RepresentationDepth,
    pub content: String,
    pub token_count: Option<i64>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySource {
    pub memory_id: MemoryId,
    pub source_type: String,
    pub source_ref: Option<String>,
    /// Verbatim snippet, capped at 2 KiB by the CLI before persistence.
    pub source_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvent {
    pub project_id: ProjectId,
    pub event_type: String,
    pub payload_json: Option<String>,
}

/// Counts of memories by status, scoped to a single project. Returned by
/// `Store::memory_counts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCounts {
    pub active: i64,
    pub deleted: i64,
}
