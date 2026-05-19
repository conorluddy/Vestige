//! JSON-RPC 2.0 method shapes and dispatcher for the daemon control socket.
//!
//! Three methods (semantic compression — one verb, multiple intents):
//!   - `daemon.status` — returns a [`DaemonStatus`] snapshot (no params).
//!   - `daemon.kick` — runs a job now
//!     (`{ job: "embed"|"prune"|"ttl", project_id?: String }`).
//!   - `daemon.register_project` — upserts a project into the registry
//!     (`{ project_id: String, project_name: String, repo_root: String }`).
//!
//! # StatusProvider design
//!
//! Status is provided through a trait with a `Pin<Box<dyn Future>>` return so
//! we avoid the `async_trait` crate (not a workspace dependency). The production
//! implementation in `lib.rs` wraps the scheduler's `build_status` helper.
//!
//! # Locking note — daemon.kick / Embed
//!
//! `dispatch_kick` holds the registry `Mutex` across the embed awaits. This is
//! safe because worker threads communicate over an `mpsc` channel and never
//! try to re-acquire the registry lock — there is no deadlock risk. The IPC
//! socket is one-request-per-connection, so contention between concurrent
//! `kick` calls is not a concern in V0.5. Wave 5 can introduce a kick-channel
//! to remove the lock-across-await if needed.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};

use vestige_config::ResolvedDaemonConfig;

use crate::errors::{DaemonError, StructuredError};
use crate::ipc::status_file::DaemonStatus;
use crate::registry::ProjectRegistry;

// === TYPES ===

/// A parsed JSON-RPC 2.0 request.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Client-chosen request identifier; echoed back in the response.
    pub id: serde_json::Value,
    /// Method name (e.g. `"daemon.status"`).
    pub method: String,
    /// Method-specific parameters; `null` / `{}` if omitted.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A JSON-RPC 2.0 response — exactly one of `result` / `error` is present.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echoed from the request.
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object returned in `JsonRpcResponse::error`.
///
/// `code` follows the standard JSON-RPC 2.0 ranges:
/// - `-32700` parse error, `-32600` invalid request, `-32601` method not found,
///   `-32602` invalid params, `-32603` internal error.
/// - `-32000` to `-32099` server-defined application errors.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    /// Structured error envelope for agents; `None` on protocol-level errors.
    pub data: Option<StructuredError>,
}

// --- Method param / result shapes ---

/// Parameters for `daemon.kick`.
#[derive(Debug, Clone, Deserialize)]
pub struct KickParams {
    pub job: KickJob,
    /// Restrict the kick to one project. `None` = all registered projects.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Job class accepted by `daemon.kick`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KickJob {
    Embed,
    Prune,
    Ttl,
}

/// Result payload returned by `daemon.kick`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KickResult {
    /// Always `true` in V0.5 — field reserved for Wave 5 async-queue semantics.
    pub queued: bool,
    /// RFC-3339 timestamp when the kick was processed.
    pub queued_at: String,
    /// Number of projects that ran (or attempted) the job.
    pub projects_queued: u32,
}

/// Parameters for `daemon.register_project`.
#[derive(Debug, Clone, Deserialize)]
pub struct RegisterProjectParams {
    pub project_id: String,
    pub project_name: String,
    pub repo_root: String,
}

/// Result payload returned by `daemon.register_project`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterProjectResult {
    /// `true` on a new insert, `false` on an idempotent no-op.
    pub registered: bool,
    pub project_id: String,
}

// === STATUS PROVIDER ===

/// Builds a [`DaemonStatus`] snapshot on demand.
///
/// Implemented by `SchedulerStatusProvider` in `lib.rs`. The `call` method
/// returns a boxed future so the dispatcher can await it without the
/// `async_trait` crate.
pub trait StatusProvider: Send + Sync {
    fn current_status(&self) -> Pin<Box<dyn Future<Output = DaemonStatus> + Send + '_>>;
}

// === PUBLIC API ===

/// Async dispatcher — maps a parsed [`JsonRpcRequest`] to a [`JsonRpcResponse`].
///
/// Pure logic layer: no I/O, no socket handling. `server.rs` owns framing.
///
/// `ttl_days_default` is read from the daemon's resolved config at startup and
/// passed in so the TTL kick can use the configured value without the dispatcher
/// needing to own the full config struct. A `KickParams` may not override this
/// in V0.5; the config value is authoritative.
///
/// `config_tx` is the watch sender for live config reload. `daemon.reload_config`
/// re-reads the config from disk and pushes the new value to the scheduler via
/// this sender. Provider changes are NOT applied live — they require daemon restart.
pub async fn dispatch(
    registry: Arc<Mutex<ProjectRegistry>>,
    status_provider: &dyn StatusProvider,
    request: JsonRpcRequest,
    ttl_days_default: u32,
    config_tx: &watch::Sender<ResolvedDaemonConfig>,
) -> JsonRpcResponse {
    if request.jsonrpc != "2.0" {
        return invalid_request_response(
            request.id,
            format!("jsonrpc must be \"2.0\", got {:?}", request.jsonrpc),
        );
    }

    match request.method.as_str() {
        "daemon.status" => dispatch_status(status_provider, request.id).await,
        "daemon.kick" => {
            dispatch_kick(registry, request.id, request.params, ttl_days_default).await
        }
        "daemon.register_project" => {
            dispatch_register_project(registry, request.id, request.params).await
        }
        "daemon.reload_config" => dispatch_reload_config(request.id, config_tx),
        _ => method_not_found_response(request.id, &request.method),
    }
}

/// Build a parse-error response (used by `server.rs` when framing fails before
/// a valid `JsonRpcRequest` can be deserialized).
pub fn parse_error_response(detail: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id: serde_json::Value::Null,
        result: None,
        error: Some(JsonRpcError {
            code: -32700,
            message: format!("parse error: {detail}"),
            data: None,
        }),
    }
}

// === PRIVATE DISPATCH HELPERS ===

async fn dispatch_status(
    status_provider: &dyn StatusProvider,
    id: serde_json::Value,
) -> JsonRpcResponse {
    let status = status_provider.current_status().await;
    match serde_json::to_value(status) {
        Ok(v) => ok_response(id, v),
        Err(e) => internal_error_response(id, format!("serialization failed: {e}")),
    }
}

async fn dispatch_kick(
    registry: Arc<Mutex<ProjectRegistry>>,
    id: serde_json::Value,
    params: serde_json::Value,
    ttl_days_default: u32,
) -> JsonRpcResponse {
    let params: KickParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return invalid_params_response(id, e.to_string()),
    };

    // Validate and collect target project IDs while holding the lock briefly.
    let project_ids: Vec<vestige_core::ProjectId> = {
        let guard = registry.lock().await;
        match params.project_id {
            Some(ref pid_str) => {
                let pid = match vestige_core::ProjectId::from_str(pid_str) {
                    Ok(p) => p,
                    Err(e) => {
                        return invalid_params_response(id, format!("invalid project_id: {e}"));
                    }
                };
                if guard.get(&pid).is_none() {
                    return error_response(
                        id,
                        DaemonError::ProjectNotRegistered {
                            project_id: pid_str.clone(),
                        }
                        .structured(),
                        "project not registered",
                    );
                }
                vec![pid]
            }
            None => guard.project_ids().cloned().collect(),
        }
        // Guard dropped here.
    };

    let queued_at = now_rfc3339();
    let mut projects_queued = 0u32;
    let kick_start = std::time::Instant::now();

    tracing::info!(
        job = ?params.job,
        project_id = ?params.project_id,
        "kick request received"
    );

    // Dispatch to each project worker. Hold the registry lock around each
    // individual worker call. Workers communicate over an `mpsc` channel and
    // never re-acquire this lock, so there is no deadlock risk.
    for pid in &project_ids {
        let guard = registry.lock().await;
        let worker = guard.get(pid);

        match params.job {
            KickJob::Embed => match worker {
                Some(w) => match w.embed().await {
                    Ok(summary) => {
                        drop(guard);
                        projects_queued += 1;
                        tracing::info!(
                            project = %pid.as_str(),
                            representations_processed = summary.representations_processed,
                            embeddings_added = summary.embeddings_added,
                            finished_at = %summary.finished_at,
                            "kick embed ok"
                        );
                    }
                    Err(e) => {
                        drop(guard);
                        tracing::warn!(project = %pid.as_str(), error = %e, "kick embed failed");
                    }
                },
                None => {
                    drop(guard);
                    tracing::warn!(
                        project = %pid.as_str(),
                        "kick embed: worker disappeared between enumeration and dispatch; skipping"
                    );
                }
            },
            KickJob::Prune => match worker {
                Some(w) => match w.prune().await {
                    Ok(summary) => {
                        drop(guard);
                        projects_queued += 1;
                        tracing::info!(
                            project = %pid.as_str(),
                            vacuumed = summary.vacuumed,
                            finished_at = %summary.finished_at,
                            "kick prune ok"
                        );
                    }
                    Err(e) => {
                        drop(guard);
                        tracing::warn!(project = %pid.as_str(), error = %e, "kick prune failed");
                    }
                },
                None => {
                    drop(guard);
                    tracing::warn!(
                        project = %pid.as_str(),
                        "kick prune: worker disappeared between enumeration and dispatch; skipping"
                    );
                }
            },
            KickJob::Ttl => match worker {
                Some(w) => match w.ttl(ttl_days_default).await {
                    Ok(summary) => {
                        drop(guard);
                        projects_queued += 1;
                        tracing::info!(
                            project = %pid.as_str(),
                            candidates_expired = summary.candidates_expired,
                            ttl_days = summary.ttl_days,
                            finished_at = %summary.finished_at,
                            "kick ttl ok"
                        );
                    }
                    Err(e) => {
                        drop(guard);
                        tracing::warn!(project = %pid.as_str(), error = %e, "kick ttl failed");
                    }
                },
                None => {
                    drop(guard);
                    tracing::warn!(
                        project = %pid.as_str(),
                        "kick ttl: worker disappeared between enumeration and dispatch; skipping"
                    );
                }
            },
        }
    }

    tracing::info!(
        job = ?params.job,
        projects_queued,
        elapsed_ms = kick_start.elapsed().as_millis(),
        "kick completed"
    );

    ok_response(
        id,
        serde_json::to_value(KickResult {
            queued: true,
            queued_at,
            projects_queued,
        })
        .expect("KickResult serializes"),
    )
}

/// Reload daemon configuration from disk and push the new cadences to the scheduler.
///
/// # What is reloaded
///
/// Only job cadences and the candidate TTL value are applied live:
/// - `embed_sweep_interval_secs`
/// - `trace_prune_interval_secs`
/// - `candidate_ttl_sweep_interval_secs`
/// - `candidate_ttl_days`
///
/// The scheduler picks up the new values on its next `'outer: loop` iteration
/// (i.e. at the next interval rebuild, not mid-tick). The response field
/// `applied_at: "next-tick"` reflects this honest semantics.
///
/// # What is NOT reloaded
///
/// Provider changes (e.g. switching from `fake` to `fastembed`) require a
/// daemon restart — provider lifecycle includes model load which cannot be
/// hot-swapped safely in V0.5.
fn dispatch_reload_config(
    id: serde_json::Value,
    config_tx: &watch::Sender<ResolvedDaemonConfig>,
) -> JsonRpcResponse {
    // Re-read config from the project config file.
    // `daemon_config_for(None)` walks the PRD §9.3 identity chain and resolves
    // the global `~/.vestige/` daemon config, applying all defaults.
    let new_config = vestige_config::daemon_config_for(None);

    if config_tx.send(new_config.clone()).is_err() {
        return internal_error_response(
            id,
            "scheduler dropped config receiver — daemon may be shutting down".into(),
        );
    }

    tracing::info!(
        embed_sweep_interval_secs = new_config.embed_sweep_interval_secs,
        trace_prune_interval_secs = new_config.trace_prune_interval_secs,
        candidate_ttl_sweep_interval_secs = new_config.candidate_ttl_sweep_interval_secs,
        candidate_ttl_days = new_config.candidate_ttl_days,
        "daemon.reload_config: new config sent to scheduler"
    );

    let result = serde_json::json!({
        "reloaded": true,
        "applied_at": "next-tick",
        "embed_sweep_interval_secs": new_config.embed_sweep_interval_secs,
        "trace_prune_interval_secs": new_config.trace_prune_interval_secs,
        "candidate_ttl_sweep_interval_secs": new_config.candidate_ttl_sweep_interval_secs,
        "candidate_ttl_days": new_config.candidate_ttl_days,
        "note": "Provider changes require daemon restart — reload only updates job cadences and TTL config."
    });

    ok_response(id, result)
}

async fn dispatch_register_project(
    registry: Arc<Mutex<ProjectRegistry>>,
    id: serde_json::Value,
    params: serde_json::Value,
) -> JsonRpcResponse {
    let params: RegisterProjectParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return invalid_params_response(id, e.to_string()),
    };

    let project_id = match vestige_core::ProjectId::from_str(&params.project_id) {
        Ok(p) => p,
        Err(e) => return invalid_params_response(id, format!("invalid project_id: {e}")),
    };

    let mut guard = registry.lock().await;
    let was_new = guard.get(&project_id).is_none();

    if let Err(e) = guard.ensure_registered(
        project_id.clone(),
        params.project_name,
        PathBuf::from(params.repo_root),
    ) {
        return error_response(id, e.structured(), "register_project failed");
    }

    ok_response(
        id,
        serde_json::to_value(RegisterProjectResult {
            registered: was_new,
            project_id: project_id.as_str().to_string(),
        })
        .expect("RegisterProjectResult serializes"),
    )
}

// === RESPONSE BUILDERS ===

fn ok_response(id: serde_json::Value, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

/// Server-defined application error (code -32000).
///
/// Used for all `DaemonError`-derived failures that reach the IPC boundary.
pub fn error_response(
    id: serde_json::Value,
    structured: StructuredError,
    message: &str,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32000,
            message: message.to_string(),
            data: Some(structured),
        }),
    }
}

fn method_not_found_response(id: serde_json::Value, method: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }),
    }
}

fn invalid_params_response(id: serde_json::Value, detail: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32602,
            message: format!("invalid params: {detail}"),
            data: None,
        }),
    }
}

fn invalid_request_response(id: serde_json::Value, detail: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32600,
            message: format!("invalid request: {detail}"),
            data: None,
        }),
    }
}

pub(crate) fn internal_error_response(id: serde_json::Value, detail: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code: -32603,
            message: format!("internal error: {detail}"),
            data: None,
        }),
    }
}

// === PRIVATE HELPERS ===

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

// === TESTS ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use crate::ipc::status_file::{DaemonStatus, ProjectStatus};
    use vestige_store::Store;

    // --- Fake StatusProvider ---

    struct FakeStatusProvider {
        status: DaemonStatus,
    }

    impl StatusProvider for FakeStatusProvider {
        fn current_status(&self) -> Pin<Box<dyn Future<Output = DaemonStatus> + Send + '_>> {
            let s = self.status.clone();
            Box::pin(async move { s })
        }
    }

    fn fake_status() -> DaemonStatus {
        DaemonStatus {
            schema_version: 1,
            version: "0.5.0-test".to_string(),
            pid: 99,
            started_at: "2026-05-19T12:00:00Z".to_string(),
            uptime_secs: 42,
            projects: vec![ProjectStatus {
                project_id: vestige_core::ProjectId::from_slug("test"),
                project_name: "Test".to_string(),
                repo_root: "/tmp/test".to_string(),
                last_embed_run: None,
                last_prune_run: None,
                last_ttl_run: None,
                pending_embeds: 0,
                memory_count: 0,
                candidate_count: 0,
                last_memory_at: None,
            }],
            next_jobs: Vec::new(),
        }
    }

    fn make_request(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: method.to_string(),
            params,
        }
    }

    fn empty_registry() -> Arc<Mutex<crate::registry::ProjectRegistry>> {
        Arc::new(Mutex::new(crate::registry::ProjectRegistry::new(5000)))
    }

    /// Build a dummy config watch sender for tests that don't exercise reload.
    fn dummy_config_tx() -> watch::Sender<ResolvedDaemonConfig> {
        let (tx, _rx) = watch::channel(vestige_config::daemon_config_for(None));
        tx
    }

    // -----------------------------------------------------------------------

    /// `daemon.status` returns the snapshot supplied by the StatusProvider.
    #[tokio::test]
    async fn dispatch_status_returns_snapshot() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request("daemon.status", serde_json::json!({}));
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.error.is_none(), "no error expected: {:?}", resp.error);
        let result = resp.result.expect("result must be present");
        let status: DaemonStatus = serde_json::from_value(result).unwrap();
        assert_eq!(status.pid, 99);
        assert_eq!(status.uptime_secs, 42);
        assert_eq!(status.projects.len(), 1);
        assert_eq!(status.projects[0].project_name, "Test");
    }

    /// An unknown method name returns error code -32601 (method not found).
    #[tokio::test]
    async fn dispatch_unknown_method_returns_method_not_found() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request("daemon.no_such_method", serde_json::json!({}));
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.result.is_none());
        let err = resp.error.expect("error must be present");
        assert_eq!(err.code, -32601, "expected method-not-found code");
    }

    /// An invalid `job` value returns error code -32602 (invalid params).
    #[tokio::test]
    async fn dispatch_kick_with_invalid_params_returns_invalid_params() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request("daemon.kick", serde_json::json!({"job": "garbage"}));
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.result.is_none());
        let err = resp.error.expect("error must be present");
        assert_eq!(err.code, -32602, "expected invalid-params code");
    }

    /// Registering the same project twice with an already-registered entry in
    /// the in-memory registry returns `registered=false` without touching
    /// `storage_path_for`. We pre-insert a worker using `discover_and_spawn_in`
    /// from a TempDir so we control the DB path completely.
    ///
    /// Full round-trip idempotency over the socket is covered by
    /// `tests/ipc_integration.rs::daemon_register_project_idempotent`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_register_idempotent() {
        use tempfile::TempDir;
        use vestige_embed::FakeEmbeddingProvider;

        let provider = FakeStatusProvider {
            status: fake_status(),
        };

        // Build a real registry with one pre-seeded project so that the second
        // `register_project` call hits the `already registered` branch.
        let tmp = TempDir::new().unwrap();
        let project_id = vestige_core::ProjectId::from_slug("idem");
        let pid_dir = tmp.path().join(project_id.as_str());
        std::fs::create_dir_all(&pid_dir).unwrap();
        let db_path = pid_dir.join("memory.sqlite");
        {
            let mut store = Store::open(&db_path).unwrap();
            store
                .ensure_project(&project_id, "Idem", Some("/tmp"), None)
                .unwrap();
        }
        let mut reg = crate::registry::ProjectRegistry::new(5000);
        reg.discover_and_spawn_with_provider_for_tests(
            tmp.path(),
            Arc::new(FakeEmbeddingProvider::default()),
        )
        .unwrap();
        let registry = Arc::new(Mutex::new(reg));

        // First call: project already in the registry → registered=false (no new insert).
        let params = serde_json::json!({
            "project_id": project_id.as_str(),
            "project_name": "Idem",
            "repo_root": "/tmp"
        });
        let req1 = make_request("daemon.register_project", params.clone());
        let resp1 = dispatch(registry.clone(), &provider, req1, 0, &dummy_config_tx()).await;
        let result1 = resp1.result.expect("first call must succeed");
        let r1: RegisterProjectResult = serde_json::from_value(result1).unwrap();
        assert!(
            !r1.registered,
            "project was already registered; must return false"
        );

        // Second call: still in registry → still registered=false.
        let req2 = make_request("daemon.register_project", params.clone());
        let resp2 = dispatch(registry.clone(), &provider, req2, 0, &dummy_config_tx()).await;
        let result2 = resp2.result.expect("second call must succeed");
        let r2: RegisterProjectResult = serde_json::from_value(result2).unwrap();
        assert!(!r2.registered, "second call must also return false");

        // Explicitly shut down workers to prevent Drop from blocking.
        let reg = Arc::try_unwrap(registry)
            .unwrap_or_else(|_| panic!("expected sole Arc owner"))
            .into_inner();
        reg.shutdown_all().await;
    }

    /// `daemon.kick` with `job: "prune"` on an empty registry reports 0 projects queued.
    ///
    /// Prune is now implemented — on an empty registry it just has no projects to work on.
    #[tokio::test]
    async fn dispatch_kick_prune_empty_registry_returns_zero() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request("daemon.kick", serde_json::json!({"job": "prune"}));
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result must be present");
        let kick: KickResult = serde_json::from_value(result).unwrap();
        assert!(kick.queued);
        assert_eq!(kick.projects_queued, 0);
    }

    /// Kicking embed on an empty registry reports 0 projects queued (not an error).
    #[tokio::test]
    async fn dispatch_kick_embed_empty_registry_returns_zero() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request("daemon.kick", serde_json::json!({"job": "embed"}));
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result must be present");
        let kick: KickResult = serde_json::from_value(result).unwrap();
        assert!(kick.queued);
        assert_eq!(kick.projects_queued, 0);
        assert!(!kick.queued_at.is_empty());
    }

    /// Kicking a project that is not in the registry returns PROJECT_NOT_REGISTERED.
    #[tokio::test]
    async fn dispatch_kick_unknown_project_returns_not_registered() {
        let provider = FakeStatusProvider {
            status: fake_status(),
        };
        let registry = empty_registry();
        let req = make_request(
            "daemon.kick",
            serde_json::json!({"job": "embed", "project_id": "proj_nonexistent"}),
        );
        let resp = dispatch(registry, &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.result.is_none());
        let err = resp.error.expect("error must be present");
        assert_eq!(err.code, -32000);
        let data = err.data.expect("structured error data must be present");
        assert_eq!(data.code, "PROJECT_NOT_REGISTERED");
    }

    /// Seeded project round-trip: kick embed against a real worker succeeds.
    ///
    /// Uses a multi-thread runtime to avoid single-threaded executor stalls
    /// when the worker OS thread sends its oneshot reply.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_kick_embed_with_real_worker_succeeds() {
        use tempfile::TempDir;
        use vestige_embed::FakeEmbeddingProvider;

        let provider = FakeStatusProvider {
            status: fake_status(),
        };

        let tmp = TempDir::new().unwrap();
        let project_id = vestige_core::ProjectId::from_slug("kick-real");

        // Seed DB in canonical subdirectory layout.
        let pid_dir = tmp.path().join(project_id.as_str());
        std::fs::create_dir_all(&pid_dir).unwrap();
        let db_path = pid_dir.join("memory.sqlite");
        {
            let mut store = Store::open(&db_path).unwrap();
            store
                .ensure_project(&project_id, "Kick Real", Some("/tmp"), None)
                .unwrap();
        }

        let mut reg = crate::registry::ProjectRegistry::new(5000);
        reg.discover_and_spawn_with_provider_for_tests(
            tmp.path(),
            Arc::new(FakeEmbeddingProvider::default()),
        )
        .unwrap();
        assert_eq!(reg.project_ids().count(), 1, "one project discovered");

        let registry = Arc::new(Mutex::new(reg));

        let req = make_request(
            "daemon.kick",
            serde_json::json!({"job": "embed", "project_id": project_id.as_str()}),
        );
        let resp = dispatch(registry.clone(), &provider, req, 0, &dummy_config_tx()).await;

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.expect("result must be present");
        let kick: KickResult = serde_json::from_value(result).unwrap();
        assert!(kick.queued);
        assert_eq!(kick.projects_queued, 1, "one project ran the embed");

        // Explicitly shut down workers to prevent Drop from blocking.
        let reg = Arc::try_unwrap(registry)
            .unwrap_or_else(|_| panic!("expected sole Arc owner"))
            .into_inner();
        reg.shutdown_all().await;
    }

    /// `daemon.reload_config` returns `reloaded: true` and the new cadences.
    ///
    /// Uses a real watch channel to confirm the new config value is pushed.
    #[tokio::test]
    async fn dispatch_reload_config_returns_success_and_updates_watch() {
        let (config_tx, config_rx) = watch::channel(vestige_config::daemon_config_for(None));

        let id = serde_json::json!(42);
        let resp = dispatch_reload_config(id.clone(), &config_tx);

        // Response shape.
        assert!(resp.error.is_none(), "expected no error: {:?}", resp.error);
        let result = resp.result.expect("result must be present");
        assert_eq!(result["reloaded"], true, "reloaded must be true");
        assert_eq!(
            result["applied_at"], "next-tick",
            "must be honest about timing"
        );
        assert!(
            result["embed_sweep_interval_secs"].is_u64(),
            "embed interval must be present"
        );
        assert!(
            result["note"].is_string(),
            "note about provider restart must be present"
        );

        // Watch channel must have received a new value.
        assert!(
            config_rx.has_changed().unwrap_or(false),
            "watch receiver must see a new config after reload_config"
        );
    }

    /// `daemon.reload_config` returns an internal error when the watch sender
    /// is closed (i.e. the scheduler has exited).
    #[tokio::test]
    async fn dispatch_reload_config_with_dropped_receiver_returns_internal_error() {
        let (config_tx, config_rx) = watch::channel(vestige_config::daemon_config_for(None));
        // Drop the receiver to simulate a shutdown scheduler.
        drop(config_rx);

        let id = serde_json::json!(99);
        let resp = dispatch_reload_config(id, &config_tx);

        assert!(resp.result.is_none(), "must not have a result");
        let err = resp.error.expect("must have an error");
        assert_eq!(err.code, -32603, "must be internal error code");
    }
}
