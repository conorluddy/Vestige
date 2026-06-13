//! Config TOML data model — structs, defaults, and `VestigeConfig` helpers.
//!
//! This module is pure: it owns the wire-level layout of `.vestige/config.toml`
//! and nothing else. No filesystem I/O, no process spawning.
//!
//! # Wire format
//!
//! ```toml
//! project_id   = "proj_vestige"
//! project_name = "Vestige"
//! scope        = "project"        # optional, default "project"
//!
//! [storage]
//! mode = "user_data"
//! path = "~/.vestige/projects/proj_vestige/memory.sqlite"
//!
//! [recall]
//! default_depth              = "one_liner"
//! max_results                = 8
//! include_global_preferences = false
//!
//! [mcp]
//! allow_record_observation = true
//! allow_record_decision    = true
//! allow_forget             = false
//!
//! # Optional V0.1+ sections — omit for full V0 compat:
//! # [embeddings]
//! # [search]
//! ```
//!
//! Missing optional sections deserialise to `None`; re-serialising a config
//! loaded from a V0 file will not emit those sections.

use serde::{Deserialize, Serialize};

use vestige_core::ProjectId;
use vestige_embed::EmbeddingsConfig;

use crate::{ConfigError, Result};

/// Default embedding provider when no `[embeddings]` section is set.
///
/// `"fake"` works without network or model downloads — sensible for tests
/// and first-run experience. Real semantic recall requires switching to
/// `"fastembed"` or `"ollama"` and re-running `vestige embed --all`.
const DEFAULT_PROVIDER: &str = "fake";

/// Default value for the `scope` field — `"project"`.
fn default_scope() -> String {
    "project".into()
}

/// Serde default helper returning `true`.
fn default_true() -> bool {
    true
}

// === TOP-LEVEL CONFIG ===

/// Root config struct, mirroring `.vestige/config.toml` one-to-one.
///
/// Serialised with `toml::to_string_pretty` by [`write_config`](crate::write_config);
/// deserialised with `toml::from_str` by [`read_config`](crate::read_config).
///
/// Optional sections ([`embeddings`](Self::embeddings), [`search`](Self::search))
/// use `Option<_>` rather than a defaulted struct so that a V0 config round-trips
/// without gaining new sections it didn't have.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VestigeConfig {
    /// Serialised `ProjectId`, e.g. `"proj_vestige"`.
    ///
    /// Use [`VestigeConfig::project_id`] to parse this into a typed
    /// [`ProjectId`]. The raw string is kept here so the TOML round-trip is
    /// lossless even if the prefix rules change.
    pub project_id: String,

    /// Human-readable project name written to config at `vestige init` time.
    ///
    /// Displayed in `vestige status` and used as the source display name on
    /// memories. Defaults to the directory basename when `--name` is omitted.
    pub project_name: String,

    /// Memory scope for this project. Always `"project"` in V0.
    ///
    /// Reserved for future cross-project scoping (V0.7). Do not branch on this
    /// value until that milestone lands.
    #[serde(default = "default_scope")]
    pub scope: String,

    /// SQLite storage location (TOML `[storage]`).
    #[serde(default)]
    pub storage: StorageConfig,

    /// Recall/search defaults (TOML `[recall]`).
    #[serde(default)]
    pub recall: RecallConfig,

    /// MCP capability gates (TOML `[mcp]`).
    #[serde(default)]
    pub mcp: McpConfig,

    /// Optional embedding provider config (TOML `[embeddings]`). V0.1+.
    ///
    /// `None` when the section is absent — `vestige init` does not emit it.
    /// Presence enables semantic/hybrid search; absence falls back to lexical FTS.
    #[serde(default)]
    pub embeddings: Option<EmbeddingsConfigSection>,

    /// Optional search-mode defaults (TOML `[search]`). V0.1+.
    ///
    /// `None` when the section is absent — existing V0 configs stay unaffected.
    #[serde(default)]
    pub search: Option<SearchConfigSection>,

    /// Optional assimilation inbox config (TOML `[assimilation]`). V0.2+.
    ///
    /// `None` when the section is absent — existing V0/V0.1 configs round-trip
    /// without change. Presence opts the project into the candidate inbox review
    /// flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assimilation: Option<AssimilationConfig>,

    /// Optional trace behaviour config (TOML `[traces]`). V0.3+.
    ///
    /// `None` when the section is absent — all defaults apply. Presence allows
    /// disabling trace writes, tuning the FIFO cap, query-text truncation, and
    /// per-surface toggles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traces: Option<TracesConfig>,

    /// Optional daemon runtime config (TOML `[daemon]`). V0.5+.
    ///
    /// `None` when the section is absent — all defaults apply and the daemon
    /// remains disabled. Presence opts the project into the background daemon
    /// runtime and allows tuning sweep cadences, TTLs, and socket paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonConfig>,
}

// === SECTION STRUCTS ===

/// Configuration for the embedding provider (`[embeddings]` in `.vestige/config.toml`).
///
/// All fields are optional — omitting the section keeps behaviour identical to V0
/// (lexical FTS only, `"fake"` provider for tests).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct EmbeddingsConfigSection {
    /// Embedding backend. `"fake"` | `"fastembed"` | `"ollama"`. Default: `"fake"`.
    ///
    /// `"fake"` works out of the box and is suitable for tests. Switch to
    /// `"fastembed"` for real semantic recall after installing with
    /// `cargo build --features fastembed`.
    pub provider: Option<String>,

    /// Model identifier passed to the provider.
    ///
    /// Omit to use the provider's recommended default (e.g. `bge-small-en-v1.5`
    /// for fastembed). Set explicitly to pin a specific model version.
    pub model: Option<String>,

    /// Vector dimensionality. Omit to use the provider's native dimensions.
    ///
    /// Must match the dimensions of any existing embeddings in the store — changing
    /// this on an existing project requires a full re-embed (`vestige embed --all`).
    pub dimensions: Option<usize>,

    /// Which representation kinds to embed. PRD §6.2 recommends `["summary", "compressed"]`.
    ///
    /// Omit to use the provider default. Values are matched against
    /// `MemoryRepresentationKind` string names.
    pub default_representations: Option<Vec<String>>,
}

/// Borrow conversion: section → runtime [`EmbeddingsConfig`].
///
/// Used by every code path that needs to build an embedding provider from a
/// loaded `.vestige/config.toml`. The CLI and MCP layers used to carry their
/// own copies of this mapping; this `From` impl is the single source of truth.
impl From<&EmbeddingsConfigSection> for EmbeddingsConfig {
    fn from(section: &EmbeddingsConfigSection) -> Self {
        EmbeddingsConfig {
            provider: section
                .provider
                .clone()
                .unwrap_or_else(|| DEFAULT_PROVIDER.into()),
            model: section.model.clone(),
            dimensions: section.dimensions,
        }
    }
}

/// Build an [`EmbeddingsConfig`] from an optional section, defaulting to
/// [`DEFAULT_PROVIDER`] when absent.
///
/// Provided as a free function (not a `From<Option<_>> for EmbeddingsConfig`
/// impl) because Rust's orphan rules block trait impls when both `Option` and
/// `EmbeddingsConfig` are foreign types.
pub fn embeddings_config_for(section: Option<&EmbeddingsConfigSection>) -> EmbeddingsConfig {
    match section {
        Some(s) => s.into(),
        None => EmbeddingsConfig {
            provider: DEFAULT_PROVIDER.into(),
            model: None,
            dimensions: None,
        },
    }
}

/// Configuration for search behaviour (`[search]` in `.vestige/config.toml`).
///
/// Controls the fallback search mode when no explicit `--mode` flag is passed.
/// See `vestige_core::memory::search::resolve_default_mode` for the full
/// precedence chain (explicit flag → config default → `"lexical"`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct SearchConfigSection {
    /// Default retrieval strategy. `"lexical"` | `"semantic"` | `"hybrid"`.
    ///
    /// Defaults to `"lexical"` when absent (backwards-compatible with V0).
    /// Set to `"hybrid"` once embeddings are configured for best recall quality.
    pub default_mode: Option<String>,
}

/// Storage location for the SQLite database (`[storage]` in `.vestige/config.toml`).
///
/// The `path` field stores the resolved `~/.vestige/projects/<id>/memory.sqlite`
/// path using `~` notation. Call [`VestigeConfig::resolved_storage_path`] to
/// expand it to an absolute [`PathBuf`](std::path::PathBuf).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageConfig {
    /// Storage strategy. Always `"user_data"` in V0.
    ///
    /// `"user_data"` means the DB lives in `~/.vestige/projects/` — outside the
    /// repo and never committed. Alternative modes are reserved for future use.
    pub mode: String,

    /// `~`-prefixed path to `memory.sqlite`.
    ///
    /// Written at `vestige init` time by [`build_init_config`](crate::build_init_config).
    /// Expand with [`VestigeConfig::resolved_storage_path`] before use.
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            mode: "user_data".into(),
            path: String::new(),
        }
    }
}

/// Recall and search defaults (`[recall]` in `.vestige/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallConfig {
    /// Representation depth returned by `vestige context` and `vestige search`.
    ///
    /// `"one_liner"` | `"summary"` | `"compressed"` | `"full"`.
    /// Default: `"one_liner"` (token-efficient; agents request deeper on demand).
    pub default_depth: String,

    /// Maximum results returned per search or context query. Default: `8`.
    ///
    /// Agents should page via `--limit`/`--offset` rather than raising this cap.
    pub max_results: u32,

    /// Whether to merge global (cross-project) preferences into context output.
    ///
    /// Always `false` in V0 — cross-project recall is deferred to V0.7.
    pub include_global_preferences: bool,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self {
            default_depth: "one_liner".into(),
            max_results: 8,
            include_global_preferences: false,
        }
    }
}

/// MCP capability gates (`[mcp]` in `.vestige/config.toml`).
///
/// Each flag controls whether the corresponding MCP tool is enabled.
/// Disable tools you don't want an agent to invoke autonomously.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpConfig {
    /// Permit the `vestige_remember` MCP tool for observation-type memories. Default: `true`.
    #[serde(default = "default_true")]
    pub allow_record_observation: bool,

    /// Permit the `vestige_remember` MCP tool for decision-type memories. Default: `true`.
    #[serde(default = "default_true")]
    pub allow_record_decision: bool,

    /// Permit the `vestige_forget` MCP tool (soft-delete). Default: `false`.
    ///
    /// Off by default — destructive action requires explicit opt-in. The underlying
    /// operation is always a soft-delete; no data is permanently removed.
    #[serde(default)]
    pub allow_forget: bool,

    /// Permit `vestige_propose_candidate` MCP tool. Default: `true`.
    ///
    /// Agents may propose candidates by default. Disable to make the inbox
    /// read-only from the MCP surface.
    #[serde(default = "default_true")]
    pub allow_propose_candidate: bool,

    /// Permit MCP-driven candidate approval. Default: `false`.
    ///
    /// Off by default — approval promotes a candidate to a durable memory and
    /// requires explicit opt-in. Approval tools are not shipped in V0.2 but the
    /// gate exists for forward-compatibility.
    #[serde(default)]
    pub allow_candidate_approval: bool,

    /// Permit MCP-driven candidate rejection. Default: `false`.
    ///
    /// Off by default — rejection requires explicit opt-in. Rejection tools are
    /// not shipped in V0.2 but the gate exists for forward-compatibility.
    #[serde(default)]
    pub allow_candidate_rejection: bool,

    /// Permit the `vestige_scan_sessions` MCP tool (session-log ingestion). Default: `false`.
    ///
    /// Off by default — passive transcript scanning is an explicit opt-in. The tool only
    /// reads redacted turns and advances scan cursors; candidates still require the agent
    /// to call `vestige_propose_candidate`.
    #[serde(default)]
    pub allow_scan_sessions: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            allow_record_observation: true,
            allow_record_decision: true,
            allow_forget: false,
            allow_propose_candidate: true,
            allow_candidate_approval: false,
            allow_candidate_rejection: false,
            allow_scan_sessions: false,
        }
    }
}

// === ASSIMILATION CONFIG ===

/// Assimilation inbox behaviour (`[assimilation]` in `.vestige/config.toml`). V0.2+.
///
/// Controls whether new captures flow into the candidate inbox or write
/// directly as durable memories. `None` (absent section) keeps V0 / V0.1
/// behaviour — direct write is the default for explicit user commands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssimilationConfig {
    /// Whether the assimilation inbox is active. Default: `true`.
    ///
    /// When `false`, `vestige candidate add` and `vestige_propose_candidate`
    /// return a `CANDIDATE_DISABLED` error rather than writing to the inbox.
    #[serde(default = "default_assimilation_enabled")]
    pub enabled: bool,

    /// Default capture mode for agent-driven capture (e.g. `vestige-auto-memorise`).
    ///
    /// `"candidate"` (default) — writes to the inbox for human review.
    /// `"memory"` — writes a durable memory immediately, bypassing the inbox.
    #[serde(default = "default_capture_mode")]
    pub default_capture: CaptureMode,
}

impl Default for AssimilationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_capture: CaptureMode::Candidate,
        }
    }
}

/// How an agent-driven capture lands — in the inbox or directly as a memory.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Write to the candidate inbox for human review before promotion.
    Candidate,
    /// Write directly as a durable memory, bypassing the inbox.
    Memory,
}

fn default_assimilation_enabled() -> bool {
    true
}

fn default_capture_mode() -> CaptureMode {
    CaptureMode::Candidate
}

// === DAEMON CONFIG ===

/// Default master-switch for the daemon: opt-in, disabled by default.
pub const DAEMON_DEFAULT_ENABLED: bool = false;

/// Default embed-sweep cadence: 10 minutes.
pub const DAEMON_DEFAULT_EMBED_SWEEP_INTERVAL_SECS: u64 = 600;

/// Default trace-VACUUM cadence: 24 hours.
pub const DAEMON_DEFAULT_TRACE_PRUNE_INTERVAL_SECS: u64 = 86_400;

/// Default candidate stale-TTL: disabled (0 = off).
pub const DAEMON_DEFAULT_CANDIDATE_TTL_DAYS: u32 = 0;

/// Default cadence for checking candidate TTLs: 1 hour.
pub const DAEMON_DEFAULT_CANDIDATE_TTL_SWEEP_INTERVAL_SECS: u64 = 3_600;

/// Default log level passed to `tracing`.
pub const DAEMON_DEFAULT_LOG_LEVEL: &str = "info";

/// Daemon runtime behaviour (`[daemon]` in `.vestige/config.toml`). V0.5+.
///
/// All fields are `Option` so absence in TOML is unambiguous and
/// `daemon_config_for` can layer defaults without guessing "was this
/// explicitly set or did serde fill it in?".
///
/// Omitting the section entirely is the same as writing it with all defaults.
/// The daemon is **opt-in** — `enabled` defaults to `false`.
///
/// # Wire format
///
/// ```toml
/// [daemon]
/// enabled                            = false
/// embed_sweep_interval_secs          = 600
/// trace_prune_interval_secs          = 86400
/// candidate_ttl_days                 = 0
/// candidate_ttl_sweep_interval_secs  = 3600
/// log_level                          = "info"
/// # socket_path      = "~/.vestige/daemon.sock"      # optional override
/// # status_file_path = "~/.vestige/daemon.status.json" # optional override
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DaemonConfig {
    /// Master switch. Default `false` — daemon is opt-in.
    pub enabled: Option<bool>,

    /// Embed sweep cadence in seconds. Default: `600` (10 minutes).
    pub embed_sweep_interval_secs: Option<u64>,

    /// Trace VACUUM cadence in seconds. Default: `86400` (24 hours).
    pub trace_prune_interval_secs: Option<u64>,

    /// Candidate stale-TTL in days; `0` = disabled. Default: `0`.
    pub candidate_ttl_days: Option<u32>,

    /// How often to check candidate TTLs, in seconds. Default: `3600` (1 hour).
    pub candidate_ttl_sweep_interval_secs: Option<u64>,

    /// Log level passed to `tracing` (`error`, `warn`, `info`, `debug`, `trace`).
    /// Default: `"info"`.
    pub log_level: Option<String>,

    /// Override `~/.vestige/daemon.sock` (mainly for tests).
    pub socket_path: Option<String>,

    /// Override `~/.vestige/daemon.status.json`.
    pub status_file_path: Option<String>,
}

/// Fully-resolved daemon configuration with all `Option`s collapsed to concrete values.
///
/// Produced by [`daemon_config_for`]; used by daemon implementation code so it
/// never needs to call `.unwrap_or` on individual fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDaemonConfig {
    /// Whether the daemon is enabled.
    pub enabled: bool,
    /// Embed sweep cadence in seconds.
    pub embed_sweep_interval_secs: u64,
    /// Trace VACUUM cadence in seconds.
    pub trace_prune_interval_secs: u64,
    /// Candidate stale-TTL in days; `0` = disabled.
    pub candidate_ttl_days: u32,
    /// How often to check candidate TTLs, in seconds.
    pub candidate_ttl_sweep_interval_secs: u64,
    /// Log level passed to `tracing`.
    pub log_level: String,
    /// Path to the daemon Unix socket. `None` means use the default.
    pub socket_path: Option<String>,
    /// Path to the daemon status JSON file. `None` means use the default.
    pub status_file_path: Option<String>,
}

/// Resolve a [`ResolvedDaemonConfig`] from an optional `[daemon]` section,
/// applying all documented defaults when the section is absent or a field is
/// `None`.
///
/// Provided as a free function (mirroring `traces_config_for` and
/// `embeddings_config_for`) rather than a `From<Option<_>>` impl to avoid
/// orphan-rule conflicts and to keep the call site readable.
pub fn daemon_config_for(section: Option<&DaemonConfig>) -> ResolvedDaemonConfig {
    let default = DaemonConfig::default();
    let s = section.unwrap_or(&default);
    ResolvedDaemonConfig {
        enabled: s.enabled.unwrap_or(DAEMON_DEFAULT_ENABLED),
        embed_sweep_interval_secs: s
            .embed_sweep_interval_secs
            .unwrap_or(DAEMON_DEFAULT_EMBED_SWEEP_INTERVAL_SECS),
        trace_prune_interval_secs: s
            .trace_prune_interval_secs
            .unwrap_or(DAEMON_DEFAULT_TRACE_PRUNE_INTERVAL_SECS),
        candidate_ttl_days: s
            .candidate_ttl_days
            .unwrap_or(DAEMON_DEFAULT_CANDIDATE_TTL_DAYS),
        candidate_ttl_sweep_interval_secs: s
            .candidate_ttl_sweep_interval_secs
            .unwrap_or(DAEMON_DEFAULT_CANDIDATE_TTL_SWEEP_INTERVAL_SECS),
        log_level: s
            .log_level
            .clone()
            .unwrap_or_else(|| DAEMON_DEFAULT_LOG_LEVEL.to_owned()),
        socket_path: s.socket_path.clone(),
        status_file_path: s.status_file_path.clone(),
    }
}

// === TRACES CONFIG ===

/// Default FIFO cap: maximum `query_events` rows per project.
pub const TRACES_DEFAULT_MAX_PER_PROJECT: usize = 10_000;

/// Default max bytes stored for `query_text`.
pub const TRACES_DEFAULT_TRUNCATE_QUERY_TEXT_BYTES: usize = 1_024;

fn default_traces_enabled() -> bool {
    true
}

fn default_traces_max_per_project() -> usize {
    TRACES_DEFAULT_MAX_PER_PROJECT
}

fn default_traces_truncate_query_text_bytes() -> usize {
    TRACES_DEFAULT_TRUNCATE_QUERY_TEXT_BYTES
}

fn default_traces_caller_cli() -> bool {
    true
}

fn default_traces_caller_mcp() -> bool {
    true
}

/// Trace-write behaviour for `query_events` (`[traces]` in `.vestige/config.toml`). V0.3+.
///
/// All fields default to enabled/permissive so omitting the section is identical
/// to writing it with all defaults. This avoids disrupting existing projects.
///
/// # Wire format
///
/// ```toml
/// [traces]
/// enabled                    = true
/// max_per_project            = 10000
/// truncate_query_text_bytes  = 1024
/// trace_caller_cli           = true
/// trace_caller_mcp           = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TracesConfig {
    /// Master switch. `false` skips all trace writes; existing rows remain
    /// readable via `vestige trace` and replay. Default: `true`.
    #[serde(default = "default_traces_enabled")]
    pub enabled: bool,

    /// FIFO cap: maximum `query_events` rows kept per project. After every
    /// insert the oldest rows beyond this limit are deleted. Default: `10000`.
    #[serde(default = "default_traces_max_per_project")]
    pub max_per_project: usize,

    /// Maximum bytes stored in `query_text`. Truncation happens at the nearest
    /// UTF-8 codepoint boundary at or below this limit (bytes, not chars —
    /// consistent with the PRD source-snippet rule). Default: `1024`.
    #[serde(default = "default_traces_truncate_query_text_bytes")]
    pub truncate_query_text_bytes: usize,

    /// When `false`, CLI-originated recall calls are not traced. Default: `true`.
    #[serde(default = "default_traces_caller_cli")]
    pub trace_caller_cli: bool,

    /// When `false`, MCP-originated recall calls are not traced. Default: `true`.
    #[serde(default = "default_traces_caller_mcp")]
    pub trace_caller_mcp: bool,
}

impl Default for TracesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_per_project: TRACES_DEFAULT_MAX_PER_PROJECT,
            truncate_query_text_bytes: TRACES_DEFAULT_TRUNCATE_QUERY_TEXT_BYTES,
            trace_caller_cli: true,
            trace_caller_mcp: true,
        }
    }
}

/// Resolve a [`TracesConfig`] from an optional section, applying all defaults
/// when the section is absent.
///
/// Provided as a free function (mirroring `embeddings_config_for`) rather than
/// a `From<Option<_>>` impl to avoid orphan-rule conflicts.
pub fn traces_config_for(section: Option<&TracesConfig>) -> TracesConfig {
    match section {
        Some(c) => c.clone(),
        None => TracesConfig::default(),
    }
}

// === METHODS ===

impl VestigeConfig {
    /// Parse `project_id` into a typed [`ProjectId`].
    ///
    /// Returns [`ConfigError::InvalidProjectId`] if the stored string does not
    /// carry the required `proj_` prefix — this typically means the config was
    /// hand-edited or produced by a pre-prefix build.
    pub fn project_id(&self) -> Result<ProjectId> {
        use std::str::FromStr;
        ProjectId::from_str(&self.project_id)
            .map_err(|e| ConfigError::InvalidProjectId(e.to_string()))
    }

    /// Expand `storage.path` from `~`-notation to an absolute [`PathBuf`](std::path::PathBuf).
    ///
    /// Delegates to the internal `expand_tilde` helper in `vestige_config::paths`.
    /// Returns [`ConfigError::NoHome`] if `$HOME` cannot be resolved.
    pub fn resolved_storage_path(&self) -> Result<std::path::PathBuf> {
        crate::paths::expand_tilde(&self.storage.path)
    }
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal V0 config TOML (no optional sections) — must parse and
    /// round-trip cleanly. This is the backwards-compatibility baseline.
    const V0_TOML: &str = r#"
project_id   = "proj_test"
project_name = "Test Project"

[storage]
mode = "user_data"
path = "~/.vestige/projects/proj_test/memory.sqlite"

[recall]
default_depth              = "one_liner"
max_results                = 8
include_global_preferences = false

[mcp]
allow_record_observation = true
allow_record_decision    = true
allow_forget             = false
"#;

    #[test]
    fn v0_config_round_trips_without_assimilation_section() {
        let config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        assert_eq!(config.project_id, "proj_test");
        // No [assimilation] section — field must be None, not a defaulted struct.
        assert!(
            config.assimilation.is_none(),
            "absent [assimilation] must deserialise as None"
        );
        // Re-serialise and confirm the section is absent in the output.
        let serialised = toml::to_string_pretty(&config).expect("must serialise");
        assert!(
            !serialised.contains("[assimilation]"),
            "skip_serializing_if must suppress the absent section"
        );
    }

    #[test]
    fn assimilation_section_parses_candidate_mode() {
        let toml_str = format!(
            "{}\n[assimilation]\nenabled = true\ndefault_capture = \"candidate\"\n",
            V0_TOML
        );
        let config: VestigeConfig = toml::from_str(&toml_str).expect("must parse");
        let assimilation = config.assimilation.expect("[assimilation] must be Some");
        assert!(assimilation.enabled);
        assert_eq!(assimilation.default_capture, CaptureMode::Candidate);
    }

    #[test]
    fn assimilation_section_parses_memory_mode() {
        let toml_str = format!(
            "{}\n[assimilation]\nenabled = true\ndefault_capture = \"memory\"\n",
            V0_TOML
        );
        let config: VestigeConfig = toml::from_str(&toml_str).expect("must parse");
        let assimilation = config.assimilation.expect("[assimilation] must be Some");
        assert_eq!(assimilation.default_capture, CaptureMode::Memory);
    }

    #[test]
    fn mcp_config_new_field_defaults() {
        let mcp = McpConfig::default();
        assert!(
            mcp.allow_propose_candidate,
            "allow_propose_candidate must default to true"
        );
        assert!(
            !mcp.allow_candidate_approval,
            "allow_candidate_approval must default to false"
        );
        assert!(
            !mcp.allow_candidate_rejection,
            "allow_candidate_rejection must default to false"
        );
    }

    #[test]
    fn mcp_config_new_fields_deserialise_from_v0_toml() {
        // A V0 [mcp] section has no candidate gates — they must default correctly.
        let config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        assert!(config.mcp.allow_propose_candidate);
        assert!(!config.mcp.allow_candidate_approval);
        assert!(!config.mcp.allow_candidate_rejection);
    }

    // ─────────────────────────────────────────────────────────────────
    // [traces] config block tests (V0.3 M7)
    // ─────────────────────────────────────────────────────────────────

    /// Full `[traces]` block round-trips through TOML write + read without data loss.
    #[test]
    fn traces_config_round_trips() {
        let toml_str = format!(
            "{}\n[traces]\nenabled = true\nmax_per_project = 5000\ntruncate_query_text_bytes = 512\ntrace_caller_cli = false\ntrace_caller_mcp = true\n",
            V0_TOML
        );
        let config: VestigeConfig = toml::from_str(&toml_str).expect("must parse with [traces]");
        let traces = config.traces.clone().expect("[traces] must be Some");
        assert!(traces.enabled);
        assert_eq!(traces.max_per_project, 5000);
        assert_eq!(traces.truncate_query_text_bytes, 512);
        assert!(!traces.trace_caller_cli);
        assert!(traces.trace_caller_mcp);

        // Re-serialise and parse again — full round-trip.
        let re_serialised = toml::to_string_pretty(&config).expect("must serialise");
        let re_parsed: VestigeConfig =
            toml::from_str(&re_serialised).expect("re-serialised TOML must parse");
        let re_traces = re_parsed
            .traces
            .expect("[traces] must survive re-serialisation");
        assert_eq!(re_traces.max_per_project, 5000);
        assert!(!re_traces.trace_caller_cli);
    }

    /// When the `[traces]` block is completely absent, all defaults apply.
    #[test]
    fn traces_config_absent_block_uses_defaults() {
        let config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        assert!(
            config.traces.is_none(),
            "absent [traces] must deserialise as None"
        );
        // traces_config_for(None) must return defaults.
        let defaults = traces_config_for(None);
        assert!(defaults.enabled);
        assert_eq!(defaults.max_per_project, TRACES_DEFAULT_MAX_PER_PROJECT);
        assert_eq!(
            defaults.truncate_query_text_bytes,
            TRACES_DEFAULT_TRUNCATE_QUERY_TEXT_BYTES
        );
        assert!(defaults.trace_caller_cli);
        assert!(defaults.trace_caller_mcp);
    }

    /// When the block is present but individual keys are missing, each key
    /// falls back to its own default.
    #[test]
    fn traces_config_partial_block_uses_per_key_defaults() {
        // Only `enabled` is present; every other key must default.
        let toml_str = format!("{}\n[traces]\nenabled = false\n", V0_TOML);
        let config: VestigeConfig = toml::from_str(&toml_str).expect("must parse partial [traces]");
        let traces = config
            .traces
            .expect("[traces] must be Some when block present");
        assert!(!traces.enabled, "explicit false must be honoured");
        assert_eq!(
            traces.max_per_project, TRACES_DEFAULT_MAX_PER_PROJECT,
            "max_per_project must default when absent"
        );
        assert_eq!(
            traces.truncate_query_text_bytes, TRACES_DEFAULT_TRUNCATE_QUERY_TEXT_BYTES,
            "truncate_query_text_bytes must default when absent"
        );
        assert!(
            traces.trace_caller_cli,
            "trace_caller_cli must default to true"
        );
        assert!(
            traces.trace_caller_mcp,
            "trace_caller_mcp must default to true"
        );
    }

    /// V0 config must not gain a `[traces]` section when re-serialised.
    #[test]
    fn traces_config_absent_suppressed_on_serialise() {
        let config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        let serialised = toml::to_string_pretty(&config).expect("must serialise");
        assert!(
            !serialised.contains("[traces]"),
            "skip_serializing_if must suppress absent [traces] section"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // [daemon] config block tests (V0.5)
    // ─────────────────────────────────────────────────────────────────

    /// When the `[daemon]` block is absent, `config.daemon` is `None` and
    /// `daemon_config_for(None)` returns the documented defaults.
    #[test]
    fn daemon_config_defaults_when_absent() {
        let config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        assert!(
            config.daemon.is_none(),
            "absent [daemon] must deserialise as None"
        );
        // V0 TOML must not gain a [daemon] section on re-serialisation.
        let serialised = toml::to_string_pretty(&config).expect("must serialise");
        assert!(
            !serialised.contains("[daemon]"),
            "skip_serializing_if must suppress absent [daemon] section"
        );
        // daemon_config_for(None) must return all documented defaults.
        let resolved = daemon_config_for(None);
        assert!(
            !resolved.enabled,
            "daemon must be opt-in (enabled defaults to false)"
        );
        assert_eq!(
            resolved.embed_sweep_interval_secs,
            DAEMON_DEFAULT_EMBED_SWEEP_INTERVAL_SECS
        );
        assert_eq!(
            resolved.trace_prune_interval_secs,
            DAEMON_DEFAULT_TRACE_PRUNE_INTERVAL_SECS
        );
        assert_eq!(
            resolved.candidate_ttl_days,
            DAEMON_DEFAULT_CANDIDATE_TTL_DAYS
        );
        assert_eq!(
            resolved.candidate_ttl_sweep_interval_secs,
            DAEMON_DEFAULT_CANDIDATE_TTL_SWEEP_INTERVAL_SECS
        );
        assert_eq!(resolved.log_level, DAEMON_DEFAULT_LOG_LEVEL);
        assert!(resolved.socket_path.is_none());
        assert!(resolved.status_file_path.is_none());
    }

    /// A partial `[daemon]` section — only `enabled` and
    /// `embed_sweep_interval_secs` set — preserves those values and fills the
    /// rest from defaults.
    #[test]
    fn daemon_config_partial_overrides() {
        let toml_str = format!(
            "{}\n[daemon]\nenabled = true\nembed_sweep_interval_secs = 120\n",
            V0_TOML
        );
        let config: VestigeConfig = toml::from_str(&toml_str).expect("must parse partial [daemon]");
        let daemon = config
            .daemon
            .as_ref()
            .expect("[daemon] must be Some when block present");

        // Explicitly set fields survive the round-trip.
        assert_eq!(daemon.enabled, Some(true));
        assert_eq!(daemon.embed_sweep_interval_secs, Some(120));

        // Unset fields are None (unambiguously absent).
        assert!(daemon.trace_prune_interval_secs.is_none());
        assert!(daemon.candidate_ttl_days.is_none());
        assert!(daemon.log_level.is_none());

        // Resolver fills the gaps with defaults.
        let resolved = daemon_config_for(Some(daemon));
        assert!(resolved.enabled, "explicit true must be honoured");
        assert_eq!(
            resolved.embed_sweep_interval_secs, 120,
            "explicit 120 must be honoured"
        );
        assert_eq!(
            resolved.trace_prune_interval_secs, DAEMON_DEFAULT_TRACE_PRUNE_INTERVAL_SECS,
            "unset field must use default"
        );
        assert_eq!(
            resolved.candidate_ttl_days, DAEMON_DEFAULT_CANDIDATE_TTL_DAYS,
            "unset field must use default"
        );
        assert_eq!(
            resolved.log_level, DAEMON_DEFAULT_LOG_LEVEL,
            "unset field must use default"
        );
    }

    /// A fully-populated `DaemonConfig` round-trips through TOML write + read
    /// without data loss.
    #[test]
    fn daemon_config_round_trip() {
        let original = DaemonConfig {
            enabled: Some(true),
            embed_sweep_interval_secs: Some(300),
            trace_prune_interval_secs: Some(43_200),
            candidate_ttl_days: Some(7),
            candidate_ttl_sweep_interval_secs: Some(1_800),
            log_level: Some("debug".to_owned()),
            socket_path: Some("/tmp/test-daemon.sock".to_owned()),
            status_file_path: Some("/tmp/test-daemon.status.json".to_owned()),
        };

        // Embed into a full config so TOML serialisation uses the [daemon] section header.
        let mut config: VestigeConfig = toml::from_str(V0_TOML).expect("V0 TOML must parse");
        config.daemon = Some(original.clone());

        let serialised = toml::to_string_pretty(&config).expect("must serialise");
        assert!(
            serialised.contains("[daemon]"),
            "section header must appear"
        );

        let re_parsed: VestigeConfig =
            toml::from_str(&serialised).expect("re-serialised TOML must parse");
        let re_daemon = re_parsed
            .daemon
            .expect("[daemon] must survive re-serialisation");

        assert_eq!(re_daemon, original, "round-trip must be lossless");
    }
}
