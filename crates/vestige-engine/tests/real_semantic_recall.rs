//! Opt-in real-model regression; excluded from network-free default tests.

#![cfg(feature = "fastembed")]

use tempfile::TempDir;
use vestige_core::{build_bundle, MemoryType, NewMemory, ProjectId, RepresentationDepth};
use vestige_embed::FastembedProvider;
use vestige_engine::{embed::rebuild_embeddings, search::search_semantic, Caller, TracesConfig};
use vestige_store::Store;

#[test]
#[ignore = "requires the FastEmbed model cache (downloads on first run)"]
fn bge_retrieves_paraphrases_without_keyword_overlap() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(tmp.path().join("memory.sqlite")).unwrap();
    let project = ProjectId::from_slug("real-semantic");
    store
        .ensure_project(&project, "Semantic", None, None)
        .unwrap();
    let cases = [
        ("Deletion is reversible. Forget flips status to deleted; restore makes a memory active again.", "How can I bring back something I removed?"),
        ("Only approved candidates become durable memories. Agents propose suggestions; humans review the inbox.", "Who decides whether a suggestion is kept?"),
        ("Use SQLite as the canonical local store. Memory is scoped to each repository.", "Where does the project persist its records?"),
    ];
    let mut expected = Vec::new();
    for (body, _) in &cases {
        let bundle = build_bundle(
            &project,
            NewMemory {
                r#type: MemoryType::Note,
                body,
                importance: 0.5,
                source: None,
            },
        )
        .unwrap();
        expected.push(bundle.memory.id.clone());
        store.record_memory(&bundle).unwrap();
    }
    let provider = FastembedProvider::new("bge-small-en-v1.5").unwrap();
    rebuild_embeddings(
        &mut store,
        &project,
        &provider,
        &[
            RepresentationDepth::Summary,
            RepresentationDepth::Compressed,
        ],
    )
    .unwrap();
    for ((_, query), expected) in cases.iter().zip(expected) {
        let outcome = search_semantic(
            &store,
            &project,
            query,
            None,
            3,
            &provider,
            Caller::Cli,
            &TracesConfig::default(),
        )
        .unwrap();
        assert_eq!(outcome.scored[0].card.id, expected, "{query}");
        let distinct: std::collections::HashSet<_> =
            outcome.scored.iter().map(|row| &row.card.id).collect();
        assert_eq!(distinct.len(), 3, "each result should be a distinct memory");
    }
}
