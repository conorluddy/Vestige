//! Per-tab logic for the browser. Each tab owns its own list/detail state and
//! a `reload`/`draw` pair. The event loop in the parent `browse` module routes
//! input to the active tab.

pub mod candidates;
pub mod memories;
