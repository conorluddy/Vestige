//! Shared capture pipeline used by `remember`, `note`, `decision`,
//! `preference`, and `question`. Each subcommand parses its own args, then
//! funnels into [`record`].

use anyhow::Result;
use serde::Serialize;
use vestige_core::{build_bundle, MemoryType, NewMemory, NewSource, ProjectId};
use vestige_store::Store;

use crate::output::{emit_json, OutputFormat};

pub struct CaptureInput<'a> {
    pub r#type: MemoryType,
    pub body: &'a str,
    pub importance: f64,
    pub source_ref: Option<&'a str>,
    pub source_content: Option<&'a str>,
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

    match format {
        OutputFormat::Json => emit_json(&RecordedJson {
            id: id.to_string(),
            r#type: r#type.as_str(),
            truncated,
        }),
        OutputFormat::Text => {
            println!("Recorded {} {}", r#type.as_str(), id);
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
}
