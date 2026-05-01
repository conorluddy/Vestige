//! Subcommand modules. Each module owns one `run` entry point; shared capture
//! logic lives in [`capture`] and [`record`].

pub mod capture;
pub mod context;
pub mod decision;
pub mod embed;
pub mod embeddings;
pub mod forget;
pub mod init;
pub mod list;
pub mod mcp;
pub mod note;
pub mod preference;
pub mod question;
pub mod recall;
pub mod record;
pub mod reindex;
pub mod remember;
pub mod restore;
pub mod search;
pub mod search_shared;
pub mod show;
pub mod skills;
pub mod status;
