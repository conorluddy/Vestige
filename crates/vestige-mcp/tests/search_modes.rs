//! MCP smoke tests for the `vestige_search` tool — covers all three modes
//! plus the structured-error contract on invalid input.
//!
//! Calls the tool's pub `async fn` directly (no stdio framing). The rmcp
//! router itself is framework-tested; what matters here is the Vestige
//! contract: parameter validation codes, mode resolution, fallback
//! warnings, and response envelope shape (PRD §13.2 / §13.3).

use rmcp::handler::server::wrapper::Parameters;
use serde_json::Value;
use tempfile::TempDir;

use vestige_config::{build_init_config, VestigeConfig};
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::{EmbeddingProvider, FakeEmbeddingProvider};
use vestige_mcp::{SearchParams, VestigeServer};
use vestige_store::{NewEmbedding, Store};

// === HELPERS ===

/// Open a fresh project store + matching VestigeConfig for the test.
///
/// Returns the TempDir guard (so the directory survives the test scope) and
/// the wired-up MCP server. The default config has `embeddings: None` and
/// `search: None`, which means the engine resolves to the `fake` provider —
/// no network, no model downloads.
fn make_server(slug: &str) -> (TempDir, VestigeServer, ProjectId) {
    let tmp = TempDir::new().unwrap();
    let storage_path = tmp.path().join("memory.sqlite");
    let project_id = ProjectId::from_slug(slug);

    let mut store = Store::open(&storage_path).unwrap();
    store
        .ensure_project(&project_id, "MCP smoke test", None, None)
        .unwrap();

    let config: VestigeConfig = build_init_config(&project_id, "MCP smoke test", &storage_path);

    let server = VestigeServer::new(store, config, project_id.clone(), false);
    (tmp, server, project_id)
}

fn record_memory(store: &mut Store, project: &ProjectId, body: &str) -> MemoryId {
    let bundle = build_bundle(
        project,
        NewMemory {
            r#type: MemoryType::Note,
            body,
            importance: 0.5,
            source: None,
        },
    )
    .unwrap();
    let id = bundle.memory.id.clone();
    store.record_memory(&bundle).unwrap();
    id
}

fn embed_summary(
    store: &mut Store,
    memory_id: &MemoryId,
    provider: &FakeEmbeddingProvider,
    text: &str,
) {
    let repr_id = store
        .repr_id_for_depth(memory_id, RepresentationDepth::Summary)
        .unwrap()
        .expect("summary representation must exist");
    let vector = provider.embed(text).unwrap();
    store
        .record_embedding(&NewEmbedding {
            memory_id,
            representation_id: &repr_id,
            representation_type: "summary",
            provider: provider.provider_name(),
            model: provider.model_name(),
            vector: &vector,
        })
        .unwrap();
}

/// Pull the JSON envelope out of a successful CallToolResult.
fn envelope(result: &rmcp::model::CallToolResult) -> Value {
    let text = result
        .content
        .first()
        .expect("CallToolResult must have one content block")
        .as_text()
        .expect("content must be Text")
        .text
        .clone();
    serde_json::from_str(&text).expect("envelope must be valid JSON")
}

/// Parse the structured `{code, message, retryable}` body out of an ErrorData.
fn error_body(err: &rmcp::ErrorData) -> Value {
    serde_json::from_str(&err.message).expect("err.message must carry the structured body JSON")
}

/// Seed the MCP server's underlying store via a separate Store handle.
///
/// VestigeServer holds the store behind an Arc<Mutex<Inner>>; for test
/// preparation it's simpler to open a *second* connection to the same
/// SQLite file and seed through that. SQLite handles concurrent open
/// connections fine for separate operations; the server's connection
/// only kicks in when the tool method is called.
fn seed_via_second_connection(
    storage_path: &std::path::Path,
    project: &ProjectId,
    bodies: &[&str],
    embed: bool,
) -> Vec<MemoryId> {
    let mut store = Store::open(storage_path).unwrap();
    let provider = FakeEmbeddingProvider::new(64);
    let mut ids = Vec::new();
    for body in bodies {
        let id = record_memory(&mut store, project, body);
        if embed {
            embed_summary(&mut store, &id, &provider, body);
        }
        ids.push(id);
    }
    ids
}

// === TESTS ===

#[tokio::test]
async fn vestige_search_lexical_returns_results() {
    let (tmp, server, project) = make_server("mcp-lex");
    seed_via_second_connection(
        &tmp.path().join("memory.sqlite"),
        &project,
        &["Lexical search over project memory works."],
        false,
    );

    let result = server
        .vestige_search(Parameters(SearchParams {
            query: "lexical search".to_string(),
            mode: Some("lexical".to_string()),
            limit: 8,
            r#type: None,
            include_score_parts: None,
        }))
        .await
        .expect("lexical search should succeed");

    let env = envelope(&result);
    assert_eq!(env["mode"], "lexical");
    let results = env["results"].as_array().expect("results must be an array");
    assert!(
        !results.is_empty(),
        "lexical search should return at least one hit, got envelope: {env}"
    );
    assert_eq!(
        env["warnings"]
            .as_array()
            .expect("warnings must be an array")
            .len(),
        0
    );
}

#[tokio::test]
async fn vestige_search_semantic_returns_score_parts_with_vector() {
    let (tmp, server, project) = make_server("mcp-sem");
    seed_via_second_connection(
        &tmp.path().join("memory.sqlite"),
        &project,
        &["Semantic recall finds conceptually-related memories."],
        true,
    );

    let result = server
        .vestige_search(Parameters(SearchParams {
            query: "Semantic recall finds conceptually-related memories.".to_string(),
            mode: Some("semantic".to_string()),
            limit: 8,
            r#type: None,
            include_score_parts: Some(true),
        }))
        .await
        .expect("semantic search should succeed when embeddings exist");

    let env = envelope(&result);
    assert_eq!(env["mode"], "semantic");
    let results = env["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "should have at least one hit");

    let parts = &results[0]["score_parts"];
    assert!(
        parts.is_object(),
        "semantic results must carry score_parts, got: {results:?}"
    );
    let vector = parts["vector"].as_f64().expect("vector must be a number");
    assert!(
        vector > 0.99,
        "same-text embedding should score ~1.0 on the vector component, got {vector}"
    );
    assert_eq!(parts["fts"].as_f64().unwrap(), 0.0);
    assert_eq!(parts["importance"].as_f64().unwrap(), 0.0);
    assert_eq!(parts["type_boost"].as_f64().unwrap(), 0.0);
    assert_eq!(parts["total"].as_f64().unwrap(), vector);
}

#[tokio::test]
async fn vestige_search_hybrid_falls_back_to_lexical_when_no_embeddings() {
    let (tmp, server, project) = make_server("mcp-hyb-cold");
    seed_via_second_connection(
        &tmp.path().join("memory.sqlite"),
        &project,
        &["Hybrid mode degrades gracefully when no embeddings exist."],
        false, // critical: no embeddings → cold-start path
    );

    let result = server
        .vestige_search(Parameters(SearchParams {
            query: "graceful degradation".to_string(),
            mode: Some("hybrid".to_string()),
            limit: 8,
            r#type: None,
            include_score_parts: None,
        }))
        .await
        .expect("hybrid cold-start must Ok with fallback, not Err");

    let env = envelope(&result);
    assert_eq!(
        env["mode"], "lexical",
        "effective mode must be lexical after fallback"
    );
    let warnings = env["warnings"]
        .as_array()
        .expect("warnings must be an array");
    assert_eq!(warnings.len(), 1, "expected one fallback warning");
    let warning = warnings[0].as_str().unwrap();
    assert!(
        warning.contains("falling back to lexical"),
        "warning must describe the fallback, got: {warning}"
    );
    assert!(
        warning.contains("vestige embed --all"),
        "warning must hint at the fix, got: {warning}"
    );
}

#[tokio::test]
async fn vestige_search_invalid_mode_returns_structured_error() {
    let (_tmp, server, _project) = make_server("mcp-bad-mode");

    let err = server
        .vestige_search(Parameters(SearchParams {
            query: "anything".to_string(),
            mode: Some("psychic".to_string()), // not a real mode
            limit: 8,
            r#type: None,
            include_score_parts: None,
        }))
        .await
        .expect_err("invalid mode must return Err, not silently coerce");

    let body = error_body(&err);
    assert_eq!(
        body["code"], "INVALID_MODE",
        "structured error must carry the typed code, got: {body}"
    );
    assert_eq!(
        body["retryable"], false,
        "invalid mode is not retryable (input is wrong, retry won't help)"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("mode")
            || body["message"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("psychic"),
        "message should mention 'mode' or the bad value, got: {body}"
    );
}
