//! Crate-private formatting and error-conversion helpers shared across modules.
//!
//! All timestamps stored in SQLite are RFC-3339 strings in UTC. These helpers
//! centralise the `time` ↔ `String` round-trip so individual modules never
//! hand-roll format strings. Error conversion helpers normalise `thiserror`
//! wrapping for the common case of an ID parse failure on a SQLite column.

use time::OffsetDateTime;

use crate::{Result, StoreError};

/// Format an [`OffsetDateTime`] as an RFC-3339 string (UTC).
pub(crate) fn rfc3339(t: OffsetDateTime) -> Result<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .map_err(StoreError::Time)
}

/// Parse an RFC-3339 string back into an [`OffsetDateTime`].
///
/// `col` is the SQLite column index used to build a meaningful
/// [`rusqlite::Error::FromSqlConversionFailure`] if parsing fails.
pub(crate) fn parse_rfc3339(s: &str, col: usize) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|e| {
        StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            col,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))
    })
}

/// Wrap an ID parse error as a `StoreError::Sqlite` column-conversion failure.
///
/// Used wherever a `MemoryId::from_str` / `ProjectId::from_str` fails on data
/// read from a TEXT column — gives consistent error shape across all row
/// mappers without duplicating the `rusqlite::Error::FromSqlConversionFailure`
/// boilerplate.
pub(crate) fn invalid_id_to_sqlite<E: std::error::Error + Send + Sync + 'static>(
    e: E,
) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}
