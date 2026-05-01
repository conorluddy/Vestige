//! Compile-time bundled skill files and install/list helpers.
//!
//! The `bundle` module owns the embedded `skills/vestige/` snapshot and exposes
//! [`bundle::install`] (write skills to a destination dir) and [`bundle::list`]
//! (enumerate skills with names and descriptions parsed from their `SKILL.md`
//! frontmatter).

pub mod bundle;
