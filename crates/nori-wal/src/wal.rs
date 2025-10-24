//! High-level WAL (Write-Ahead Log) API.
//!
//! Provides a simple interface for append-only logging with automatic
//! recovery, rotation, and configurable durability guarantees.

use crate::record::Record;
use crate::recovery::{self, RecoveryInfo};
use crate::segment::{FsyncPolicy, Position, SegmentConfig, SegmentError, SegmentManager};
use nori_observe::{Meter, NoopMeter};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the WAL.
#[derive(Debug, Clone)]
pub struct WalConfig {
    /// Directory to store WAL segments.
    pub dir: PathBuf,
    /// Maximum size of a segment before rotation (default: 128MB).
    pub max_segment_size: u64,
    /// Fsync policy for durability (default: Batch with 5ms window).
    pub fsync_policy: FsyncPolicy,
    /// Node ID for observability events.
    pub node_id: u32,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("wal"),
            max_segment_size: 128 * 1024 * 1024, // 128 MiB
            fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),
            node_id: 0,
        }
    }
}

/// Write-Ahead Log with automatic recovery and rotation.
///
/// # Example
///
/// ```no_run
/// use nori_wal::{Wal, WalConfig, Record};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = WalConfig::default();
///     let wal = Wal::open(config).await?;
///
///     // Append a record
///     let record = Record::put(b"key", b"value");
///     let pos = wal.append(&record).await?;
///
///     // Explicitly sync to disk
///     wal.sync().await?;
///
///     Ok(())
/// }
/// ```
pub struct Wal {
    manager: Arc<SegmentManager>,
    config: WalConfig,
}

impl Wal {
    /// Opens a WAL, performing recovery if needed.
    ///
    /// This will scan all existing segments, validate records, and truncate
    /// any corruption found. Returns the opened WAL and recovery information.
    pub async fn open(config: WalConfig) -> Result<(Self, RecoveryInfo), SegmentError> {
        Self::open_with_meter(config, Arc::new(NoopMeter)).await
    }

    /// Opens a WAL with a custom observability meter.
    pub async fn open_with_meter(
        config: WalConfig,
        meter: Arc<dyn Meter>,
    ) -> Result<(Self, RecoveryInfo), SegmentError> {
        // Create directory if it doesn't exist
        tokio::fs::create_dir_all(&config.dir).await?;

        // Perform recovery
        let recovery_info = recovery::recover(&config.dir, meter.clone(), config.node_id).await?;

        // Create segment manager
        let segment_config = SegmentConfig {
            dir: config.dir.clone(),
            max_segment_size: config.max_segment_size,
            fsync_policy: config.fsync_policy,
        };

        let manager = SegmentManager::new(segment_config, meter, config.node_id).await?;

        Ok((
            Self {
                manager: Arc::new(manager),
                config,
            },
            recovery_info,
        ))
    }

    /// Appends a record to the WAL.
    ///
    /// Returns the position where the record was written.
    /// Depending on the fsync policy, the record may or may not be
    /// immediately synced to disk.
    pub async fn append(&self, record: &Record) -> Result<Position, SegmentError> {
        self.manager.append(record).await
    }

    /// Flushes buffered data to the OS (but doesn't fsync).
    pub async fn flush(&self) -> Result<(), SegmentError> {
        self.manager.flush().await
    }

    /// Syncs all data to disk (fsync).
    ///
    /// This ensures all appended records are durable.
    pub async fn sync(&self) -> Result<(), SegmentError> {
        self.manager.sync().await
    }

    /// Returns the current write position in the WAL.
    pub async fn current_position(&self) -> Position {
        self.manager.current_position().await
    }

    /// Reads records starting from the given position.
    ///
    /// Returns an iterator that can be used to scan records.
    pub async fn read_from(
        &self,
        position: Position,
    ) -> Result<crate::segment::SegmentReader, SegmentError> {
        self.manager.read_from(position).await
    }

    /// Returns the WAL configuration.
    pub fn config(&self) -> &WalConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_wal_basic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let config = WalConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let (wal, recovery_info) = Wal::open(config).await.unwrap();

        // First open should have no records
        assert_eq!(recovery_info.valid_records, 0);

        // Append some records
        for i in 0..10 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            wal.append(&record).await.unwrap();
        }

        wal.sync().await.unwrap();

        // Read them back
        let mut reader = wal
            .read_from(Position {
                segment_id: 0,
                offset: 0,
            })
            .await
            .unwrap();

        let mut count = 0;
        while reader.next_record().await.unwrap().is_some() {
            count += 1;
        }

        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn test_wal_recovery_on_reopen() {
        let temp_dir = TempDir::new().unwrap();
        let config = WalConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Write some records
        {
            let (wal, _) = Wal::open(config.clone()).await.unwrap();

            for i in 0..5 {
                let key = format!("key{}", i);
                let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
                wal.append(&record).await.unwrap();
            }

            wal.sync().await.unwrap();
        }

        // Reopen - should recover the records
        {
            let (wal, recovery_info) = Wal::open(config.clone()).await.unwrap();

            assert_eq!(recovery_info.valid_records, 5);
            assert_eq!(recovery_info.segments_scanned, 1);
            assert!(!recovery_info.corruption_detected);

            // Should be able to append more records
            let record = Record::put(b"new_key".as_slice(), b"new_value".as_slice());
            wal.append(&record).await.unwrap();
            wal.sync().await.unwrap();
        }

        // Reopen again - should have all records
        {
            let (_wal, recovery_info) = Wal::open(config).await.unwrap();
            assert_eq!(recovery_info.valid_records, 6);
        }
    }

    #[tokio::test]
    async fn test_wal_with_different_fsync_policies() {
        let temp_dir = TempDir::new().unwrap();

        // Test with Always policy
        {
            let config = WalConfig {
                dir: temp_dir.path().join("always"),
                fsync_policy: FsyncPolicy::Always,
                ..Default::default()
            };

            let (wal, _) = Wal::open(config).await.unwrap();
            let record = Record::put(b"key".as_slice(), b"value".as_slice());
            wal.append(&record).await.unwrap();
        }

        // Test with Batch policy
        {
            let config = WalConfig {
                dir: temp_dir.path().join("batch"),
                fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
                ..Default::default()
            };

            let (wal, _) = Wal::open(config).await.unwrap();
            let record = Record::put(b"key".as_slice(), b"value".as_slice());
            wal.append(&record).await.unwrap();
            wal.sync().await.unwrap();
        }

        // Test with OS policy
        {
            let config = WalConfig {
                dir: temp_dir.path().join("os"),
                fsync_policy: FsyncPolicy::Os,
                ..Default::default()
            };

            let (wal, _) = Wal::open(config).await.unwrap();
            let record = Record::put(b"key".as_slice(), b"value".as_slice());
            wal.append(&record).await.unwrap();
            wal.sync().await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_wal_current_position() {
        let temp_dir = TempDir::new().unwrap();
        let config = WalConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let (wal, _) = Wal::open(config).await.unwrap();

        let pos1 = wal.current_position().await;
        assert_eq!(pos1.offset, 0);

        let record = Record::put(b"key".as_slice(), b"value".as_slice());
        wal.append(&record).await.unwrap();

        let pos2 = wal.current_position().await;
        assert!(pos2.offset > 0);
    }

    #[tokio::test]
    async fn test_wal_tombstone_records() {
        let temp_dir = TempDir::new().unwrap();
        let config = WalConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let (wal, _) = Wal::open(config).await.unwrap();

        // Write a PUT
        let put_record = Record::put(b"key".as_slice(), b"value".as_slice());
        wal.append(&put_record).await.unwrap();

        // Write a DELETE
        let delete_record = Record::delete(b"key".as_slice());
        wal.append(&delete_record).await.unwrap();

        wal.sync().await.unwrap();

        // Read back
        let mut reader = wal
            .read_from(Position {
                segment_id: 0,
                offset: 0,
            })
            .await
            .unwrap();

        let (rec1, _) = reader.next_record().await.unwrap().unwrap();
        assert!(!rec1.tombstone);

        let (rec2, _) = reader.next_record().await.unwrap().unwrap();
        assert!(rec2.tombstone);
    }
}
