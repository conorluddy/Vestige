//! Crate-private formatting and error-conversion helpers shared across modules.

use time::OffsetDateTime;

use crate::{Result, StoreError};

pub(crate) fn rfc3339(t: OffsetDateTime) -> Result<String> {
    t.format(&time::format_description::well_known::Rfc3339)
        .map_err(StoreError::Time)
}

pub(crate) fn parse_rfc3339(s: &str, col: usize) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|e| {
        StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
            col,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))
    })
}

pub(crate) fn invalid_id_to_sqlite<E: std::error::Error + Send + Sync + 'static>(
    e: E,
) -> StoreError {
    StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(e),
    ))
}
