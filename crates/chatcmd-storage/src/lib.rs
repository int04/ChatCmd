//! Async SQLite persistence for ChatCMD's direct local runtime.

#![forbid(unsafe_code)]

mod device_identity;
mod importer;
mod path;
mod repository;
mod writer;

pub use importer::LegacyImporter;
pub use path::{DataPathError, resolve_database_path};
pub use repository::{CURRENT_SCHEMA_VERSION, MAX_TERMINAL_CHUNK_BYTES, SqliteRepository};
pub use writer::{EventWriter, EventWriterOptions};
