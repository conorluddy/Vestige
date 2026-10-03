# Real semantic recall

The `fake` backend makes deterministic test vectors and has no semantic
meaning. For this project's dogfood setup, use FastEmbed with
`bge-small-en-v1.5`: local CPU inference, no API key, and no separate service.
The model is downloaded on first use and cached in
`~/.vestige/models/bge-small-en-v1.5/`.

## Install and configure

From the source checkout:

```bash
cargo install --path crates/vestige-cli --locked --features fastembed
vestige daemon restart
```

For a published package, use `cargo install vestige --features fastembed`.
The feature must be present in the installed binary used by CLI, MCP, and
daemon; a source build alone does not update those running processes.

Set the project's `.vestige/config.toml`:

```toml
[embeddings]
provider = "fastembed"
model = "bge-small-en-v1.5"
```

Restart a running daemon after changing its project config, or send its
`daemon.reload_config` IPC request. Then replace the project's existing vectors:

```bash
vestige reindex --embeddings
vestige embeddings status
vestige recall "How can I bring back something I removed?" --semantic
vestige recall "Who decides whether a suggestion is kept?" --hybrid --score-parts
```

Expect `provider=fastembed`, `model=bge-small-en-v1.5`, and 384 dimensions.
These are paraphrase checks for deletion/restore and candidate approval;
they should retrieve the corresponding memories even without exact wording.
Hybrid remains the default. `--lexical` is useful for exact names and errors.

## Index maintenance and failure handling

`embed` and `reindex` use the same project provider/model/dimensions as search.
CLI flags override the config. Switching providers resets inherited model and
dimension settings unless explicitly overridden, avoiding a model name from
one backend leaking into another.

```bash
vestige embed --all --dry-run --json
vestige embed --all
```

Embedding is idempotent: already-current vectors are skipped. A nonzero exit
code and a per-representation failure report signal partial indexing failure.
`reindex --embeddings` does inference and vector validation before changing
the index, then swaps vectors in one SQLite transaction. Model load, inference,
insertion, or concurrent-revision failures preserve the previous index.

Stored passages are embedded as plain text. Retrieval queries receive the
search instruction recommended by the [BGE model card](https://huggingface.co/BAAI/bge-small-en-v1.5);
trace replay uses the same query path. Candidate dedup embeds the candidate as
a passage because it compares statements rather than retrieval questions.
Semantic recall selects each memory's best matching representation before
applying the result limit, so summary and compressed vectors do not produce
duplicate results or count a recall twice.

## Verification

Default tests require no model or service. An explicit real-model regression
checks that paraphrases retrieve the intended memories as their first result:

```bash
cargo test -p vestige-engine --features fastembed --test real_semantic_recall -- --ignored
```

This command downloads the model if it is not cached. Run it once online;
subsequent inference uses the local cache. Ollama remains supported behind
`--features ollama` for users who already run a local embedding service.
