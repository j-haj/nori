//! Append-only write-ahead log with recovery and rotation.
//!
//! Implements a write-ahead log (WAL) with:
//! - Varint-encoded records with CRC32C checksumming
//! - Configurable fsync policies (always, batch, os)
//! - Automatic segment rotation at 128MB
//! - Crash recovery with partial-tail truncation
//! - Observability via nori-observe

pub mod record;

pub use record::{Compression, Record, RecordError};
