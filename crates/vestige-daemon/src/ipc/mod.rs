//! Daemon IPC surfaces. V0.5 ships two: a JSON status file (read-only, atomic write)
//! and a Unix-domain control socket (Wave 4).

pub mod methods;
pub mod server;
pub mod status_file;
