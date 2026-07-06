//! Retro v3 file-based knowledge store.
//!
//! Markdown files under `<root>/knowledge/` are the source of truth.
//! SQLite (`index.db`) is a disposable, rebuildable index — files always win.

mod node;

pub use node::{Node, NodeType, Scope};
