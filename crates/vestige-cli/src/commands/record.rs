//! Shared capture pipeline used by `remember`, `note`, `decision`,
//! `preference`, and `question`. Each subcommand parses its own args, then
//! funnels into [`record`].
//!
//! `--supersedes <mem_id>` (issue #131) is a two-step, non-atomic sequence
//! on top of the same pipeline: record the new memory, then soft-delete and
//! link the old one via `Store::supersede_memory`. This is the same accepted
//! window as `approve_candidate`'s `TODO(v0.3)` — a failure between the two
//! calls leaves the new memory recorded but the old one un-superseded.

use anyhow::Result;
use serde::Serialize;
use vestige_core::{build_bundle, MemoryId, MemoryType, NewMemory, NewSource, ProjectId};
use vestige_store::Store;

use crate::output::{emit_json, OutputFormat};

pub struct CaptureInput<'a> {
    pub r#type: MemoryType,
    pub body: &'a str,
    pub importance: f64,
    pub source_ref: Option<&'a str>,
    pub source_content: Option<&'a str>,
    /// Soft-delete and link this memory as superseded by the one being recorded.
    pub supersedes: Option<&'a MemoryId>,
}

pub fn record(
    store: &mut Store,
    project_id: &ProjectId,
    input: CaptureInput<'_>,
    format: OutputFormat,
) -> Result<()> {
    let source = match (input.source_ref, input.source_content) {
        (None, None) => None,
        (r, c) => Some(NewSource {
            source_type: "cli",
            source_ref: r,
            source_content: c,
        }),
    };

    let bundle = build_bundle(
        project_id,
        NewMemory {
            r#type: input.r#type,
            body: input.body,
            importance: input.importance,
            source,
        },
    )?;
    let truncated = bundle.source.as_ref().map(|s| s.truncated).unwrap_or(false);
    let id = bundle.memory.id.clone();
    let r#type = bundle.memory.r#type;
    store.record_memory(&bundle)?;

    // Step 2 of the non-atomic supersede window documented above: the new
    // memory (`id`) is already durably recorded at this point regardless of
    // what happens next.
    let supersedes = match input.supersedes {
        Some(old_id) => {
            if !store.supersede_memory(old_id, &id)? {
                anyhow::bail!(
                    "recorded {id} but could not supersede `{old_id}` — it may not exist \
                     or is already deleted/superseded"
                );
            }
            Some(old_id.to_string())
        }
        None => None,
    };

    match format {
        OutputFormat::Json => emit_json(&RecordedJson {
            id: id.to_string(),
            r#type: r#type.as_str(),
            truncated,
            supersedes,
        }),
        OutputFormat::Text => {
            println!("Recorded {} {}", r#type.as_str(), id);
            if let Some(old_id) = &supersedes {
                println!("Superseded {old_id}");
            }
            if truncated {
                eprintln!(
                    "warning: source content truncated at {} bytes (UTF-8 boundary)",
                    vestige_core::SOURCE_SNIPPET_MAX_BYTES
                );
            }
            Ok(())
        }
    }
}

#[derive(Serialize)]
struct RecordedJson<'a> {
    id: String,
    r#type: &'a str,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    supersedes: Option<String>,
}
