//! Session-log ingestion pipeline — `SessionSource` trait + adapters (V0.5.3).
//!
//! This module provides:
//!
//! - [`source`] — the [`SessionSource`] trait, [`NormalizedTurn`], [`DiscoveredSession`] types.
//! - [`claude_code`] — [`ClaudeCodeSource`]: discovers `~/.claude/projects` transcripts,
//!   decodes dash-encoded cwd directory names, and maps each session to a registered project.
//! - [`IngestError`] — typed error enum for this pipeline layer.
//!
//! # Design
//!
//! Sessions that map to no registered project are **skipped, never misattributed**.
//! Provenance wiring and redaction (`SourceKind::SessionLog`, secret scrubbing) land in
//! Wave 2 parallel issues (#101 / #102). This PR is Wave 1: the trait surface + adapter.

pub mod claude_code;
pub mod codex;
pub mod source;

use thiserror::Error;

// === PUBLIC TYPES ===

/// Errors produced by the session-log ingestion pipeline.
///
/// # Error surface decisions
///
/// - `Io` covers filesystem discovery / file read failures — transient, retryable.
/// - `Json` covers unrecoverable file-level JSON errors (per-line errors are swallowed
///   silently with `.ok()` to tolerate partial / truncated transcripts).
/// - `Config` covers project identity resolution failures from `vestige-config`.
/// - `NoHome` covers home-directory resolution failures on restricted environments.
#[derive(Debug, Error)]
pub enum IngestError {
    /// A filesystem operation (directory walk, file read) failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A file-level JSON parse error (distinct from per-line tolerance).
    ///
    /// Per-line parse errors during [`source::SessionSource::read_turns`] are swallowed
    /// silently. This variant is reserved for structural failures (e.g. the file is
    /// entirely unreadable as UTF-8).
    #[error("json: {0}")]
    Json(serde_json::Error),

    /// Project identity resolution failed (bad TOML, invalid project_id prefix, etc.).
    #[error("config: {0}")]
    Config(String),

    /// Home directory could not be determined (`$HOME` unset, `directories::BaseDirs` failed).
    #[error("home directory not found — set $HOME or ensure a user profile is available")]
    NoHome,
}

// === RE-EXPORTS ===

pub use claude_code::ClaudeCodeSource;
pub use codex::CodexSource;
pub use source::{DiscoveredSession, NormalizedTurn, SessionSource};
