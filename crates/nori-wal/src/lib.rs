//! Append-only write-ahead log with recovery and rotation.
//!
//! Implements a write-ahead log (WAL) with:
//! - Varint-encoded records with CRC32C checksumming
//! - Configurable fsync policies (always, batch, os)
//! - Automatic segment rotation at 128MB
//! - Crash recovery with partial-tail truncation
//! - Observability via nori-observe
//!
//! # Example
//!
//! ```no_run
//! use nori_wal::{Wal, WalConfig, Record};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = WalConfig::default();
//!     let (wal, recovery_info) = Wal::open(config).await?;
//!
//!     println!("Recovered {} records", recovery_info.valid_records);
//!
//!     // Append records
//!     let record = Record::put(b"key".as_slice(), b"value".as_slice());
//!     let pos = wal.append(&record).await?;
//!
//!     // Sync to disk
//!     wal.sync().await?;
//!
//!     Ok(())
//! }
//! ```

mod prealloc;
pub mod record;
pub mod recovery;
pub mod segment;
pub mod wal;

pub use record::{Compression, Record, RecordError};
pub use recovery::RecoveryInfo;
pub use segment::{
    FsyncPolicy, Position, SegmentConfig, SegmentError, SegmentManager, SegmentReader,
};
pub use wal::{Wal, WalConfig};
