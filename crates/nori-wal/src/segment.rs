//! WAL segment file management with automatic rotation at 128MB.
//!
//! Segments are numbered sequentially (e.g., 000000.wal, 000001.wal) and rotated
//! when they reach the configured size limit (default 128MB per context/30_storage.yaml).

use crate::record::Record;
use nori_observe::{Meter, VizEvent, WalEvt, WalKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::time::Instant;

const DEFAULT_SEGMENT_SIZE: u64 = 134_217_728; // 128 MiB

#[derive(Debug, Error)]
pub enum SegmentError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Record error: {0}")]
    Record(#[from] crate::record::RecordError),
    #[error("Segment not found: {0}")]
    NotFound(u64),
}

/// Position in the WAL (segment ID + byte offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub segment_id: u64,
    pub offset: u64,
}

/// Fsync policy for durability vs performance tradeoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// Always fsync after every write (maximum durability, lowest performance).
    Always,
    /// Batch fsyncs within a time window (balanced durability and performance).
    /// Fsyncs will happen at most once per the specified duration.
    Batch(Duration),
    /// Let the OS handle fsyncing (best performance, least durability).
    Os,
}

impl Default for FsyncPolicy {
    fn default() -> Self {
        // Default to batch with 5ms window per context/30_storage.yaml
        FsyncPolicy::Batch(Duration::from_millis(5))
    }
}

/// Configuration for segment behavior.
#[derive(Debug, Clone)]
pub struct SegmentConfig {
    /// Maximum size of a segment in bytes before rotation.
    pub max_segment_size: u64,
    /// Directory to store segment files.
    pub dir: PathBuf,
    /// Fsync policy for durability.
    pub fsync_policy: FsyncPolicy,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: PathBuf::from("wal"),
            fsync_policy: FsyncPolicy::default(),
        }
    }
}

/// A single WAL segment file.
struct SegmentFile {
    id: u64,
    file: File,
    size: u64,
    #[allow(dead_code)]
    path: PathBuf,
}

impl SegmentFile {
    /// Opens an existing segment or creates a new one.
    async fn open(dir: &Path, id: u64, create: bool) -> Result<Self, SegmentError> {
        let path = segment_path(dir, id);

        let file = if create {
            OpenOptions::new()
                .create(true)
                .truncate(false) // Don't truncate - append to existing segments
                .write(true)
                .read(true)
                .open(&path)
                .await?
        } else {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await?
        };

        let metadata = file.metadata().await?;
        let size = metadata.len();

        Ok(Self {
            id,
            file,
            size,
            path,
        })
    }

    /// Appends a record to the segment.
    async fn append(&mut self, record: &Record) -> Result<u64, SegmentError> {
        let encoded = record.encode();
        let offset = self.size;

        self.file.write_all(&encoded).await?;
        self.size += encoded.len() as u64;

        Ok(offset)
    }

    /// Returns true if appending this record would exceed the size limit.
    fn would_exceed(&self, record_size: usize, max_size: u64) -> bool {
        self.size + record_size as u64 > max_size
    }

    /// Flushes data to disk.
    async fn flush(&mut self) -> Result<(), SegmentError> {
        self.file.flush().await?;
        Ok(())
    }

    /// Syncs data to disk (fsync).
    async fn sync(&mut self) -> Result<(), SegmentError> {
        self.file.sync_data().await?;
        Ok(())
    }
}

/// Manages WAL segments with automatic rotation.
pub struct SegmentManager {
    config: SegmentConfig,
    current: Arc<Mutex<SegmentFile>>,
    current_id: Arc<Mutex<u64>>,
    meter: Arc<dyn Meter>,
    node_id: u32,
    last_fsync: Arc<Mutex<Option<Instant>>>,
}

impl SegmentManager {
    /// Creates a new segment manager.
    pub async fn new(
        config: SegmentConfig,
        meter: Arc<dyn Meter>,
        node_id: u32,
    ) -> Result<Self, SegmentError> {
        // Create directory if it doesn't exist
        tokio::fs::create_dir_all(&config.dir).await?;

        // Find the latest segment ID
        let latest_id = find_latest_segment_id(&config.dir).await?;

        // Open or create the current segment
        let segment = SegmentFile::open(&config.dir, latest_id, true).await?;

        Ok(Self {
            config,
            current: Arc::new(Mutex::new(segment)),
            current_id: Arc::new(Mutex::new(latest_id)),
            meter,
            node_id,
            last_fsync: Arc::new(Mutex::new(None)),
        })
    }

    /// Appends a record to the WAL, rotating if necessary.
    /// Applies the configured fsync policy.
    pub async fn append(&self, record: &Record) -> Result<Position, SegmentError> {
        let encoded_size = record.encode().len();

        let mut current = self.current.lock().await;

        // Check if we need to rotate
        if current.would_exceed(encoded_size, self.config.max_segment_size) {
            drop(current); // Release lock before rotating
            self.rotate().await?;
            current = self.current.lock().await;
        }

        let offset = current.append(record).await?;
        let segment_id = current.id;

        // Apply fsync policy
        match self.config.fsync_policy {
            FsyncPolicy::Always => {
                // Always fsync immediately after write
                let start = Instant::now();
                current.sync().await?;
                let elapsed_ms = start.elapsed().as_millis() as u32;

                self.meter.emit(VizEvent::Wal(WalEvt {
                    node: self.node_id,
                    seg: current.id,
                    kind: WalKind::Fsync { ms: elapsed_ms },
                }));
            }
            FsyncPolicy::Batch(window) => {
                // Check if we need to fsync based on time window
                let mut last_sync = self.last_fsync.lock().await;
                let should_sync = match *last_sync {
                    None => true,
                    Some(last) => last.elapsed() >= window,
                };

                if should_sync {
                    let start = Instant::now();
                    current.sync().await?;
                    let elapsed_ms = start.elapsed().as_millis() as u32;
                    *last_sync = Some(Instant::now());

                    self.meter.emit(VizEvent::Wal(WalEvt {
                        node: self.node_id,
                        seg: current.id,
                        kind: WalKind::Fsync { ms: elapsed_ms },
                    }));
                }
            }
            FsyncPolicy::Os => {
                // No fsync - let OS handle it
            }
        }

        Ok(Position { segment_id, offset })
    }

    /// Flushes the current segment to disk.
    pub async fn flush(&self) -> Result<(), SegmentError> {
        let mut current = self.current.lock().await;
        current.flush().await
    }

    /// Syncs the current segment to disk (fsync).
    pub async fn sync(&self) -> Result<(), SegmentError> {
        let start = std::time::Instant::now();
        let mut current = self.current.lock().await;
        current.sync().await?;
        let elapsed_ms = start.elapsed().as_millis() as u32;

        // Emit fsync observability event
        self.meter.emit(VizEvent::Wal(WalEvt {
            node: self.node_id,
            seg: current.id,
            kind: WalKind::Fsync { ms: elapsed_ms },
        }));

        Ok(())
    }

    /// Rotates to a new segment file.
    async fn rotate(&self) -> Result<(), SegmentError> {
        let mut current_id = self.current_id.lock().await;
        let new_id = *current_id + 1;

        // Emit rotation event with old segment size
        let old_segment = self.current.lock().await;
        let old_size = old_segment.size;
        let old_id = old_segment.id;
        drop(old_segment);

        self.meter.emit(VizEvent::Wal(WalEvt {
            node: self.node_id,
            seg: old_id,
            kind: WalKind::SegmentRoll { bytes: old_size },
        }));

        // Create new segment
        let new_segment = SegmentFile::open(&self.config.dir, new_id, true).await?;

        // Swap in the new segment
        let mut current = self.current.lock().await;
        *current = new_segment;
        *current_id = new_id;

        Ok(())
    }

    /// Reads records from a segment starting at the given position.
    pub async fn read_from(&self, position: Position) -> Result<SegmentReader, SegmentError> {
        let path = segment_path(&self.config.dir, position.segment_id);
        let file = File::open(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SegmentError::NotFound(position.segment_id)
            } else {
                SegmentError::Io(e)
            }
        })?;

        Ok(SegmentReader {
            reader: BufReader::new(file),
            position: position.offset,
            segment_id: position.segment_id,
        })
    }

    /// Returns the current write position.
    pub async fn current_position(&self) -> Position {
        let current = self.current.lock().await;
        Position {
            segment_id: current.id,
            offset: current.size,
        }
    }
}

/// Iterator for reading records from a segment.
pub struct SegmentReader {
    reader: BufReader<File>,
    position: u64,
    segment_id: u64,
}

impl SegmentReader {
    /// Reads the next record from the segment.
    pub async fn next_record(&mut self) -> Result<Option<(Record, Position)>, SegmentError> {
        // Seek to the current position if needed
        self.reader
            .seek(std::io::SeekFrom::Start(self.position))
            .await?;

        // Try to read some data
        let mut buffer = vec![0u8; 4096]; // Start with 4KB buffer
        let n = self.reader.read(&mut buffer).await?;

        if n == 0 {
            return Ok(None); // EOF
        }

        buffer.truncate(n);

        // Try to decode a record
        match Record::decode(&buffer) {
            Ok((record, size)) => {
                let pos = Position {
                    segment_id: self.segment_id,
                    offset: self.position,
                };
                self.position += size as u64;
                Ok(Some((record, pos)))
            }
            Err(crate::record::RecordError::Incomplete) if n == 4096 => {
                // Need more data, read more
                let mut more_data = vec![0u8; 4096];
                let additional = self.reader.read(&mut more_data).await?;
                buffer.extend_from_slice(&more_data[..additional]);

                match Record::decode(&buffer) {
                    Ok((record, size)) => {
                        let pos = Position {
                            segment_id: self.segment_id,
                            offset: self.position,
                        };
                        self.position += size as u64;
                        Ok(Some((record, pos)))
                    }
                    Err(e) => Err(SegmentError::Record(e)),
                }
            }
            Err(crate::record::RecordError::Incomplete) => {
                // Incomplete at EOF - this is fine during recovery
                Ok(None)
            }
            Err(e) => Err(SegmentError::Record(e)),
        }
    }
}

/// Generates the path for a segment file.
fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{:06}.wal", id))
}

/// Finds the latest segment ID in a directory.
async fn find_latest_segment_id(dir: &Path) -> Result<u64, SegmentError> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut max_id = 0u64;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "wal" {
                if let Some(stem) = path.file_stem() {
                    if let Some(stem_str) = stem.to_str() {
                        if let Ok(id) = stem_str.parse::<u64>() {
                            max_id = max_id.max(id);
                        }
                    }
                }
            }
        }
    }

    Ok(max_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nori_observe::NoopMeter;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_segment_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Os, // Fast for tests
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        let record = Record::put(b"key1".as_slice(), b"value1".as_slice());
        let pos = manager.append(&record).await.unwrap();

        assert_eq!(pos.segment_id, 0);
        assert_eq!(pos.offset, 0);
    }

    #[tokio::test]
    async fn test_segment_rotation() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: 100, // Small size to trigger rotation
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Os,
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        // Write small records that accumulate to trigger rotation
        let record = Record::put(b"key".as_slice(), b"value".as_slice());

        let pos1 = manager.append(&record).await.unwrap();
        assert_eq!(pos1.segment_id, 0);

        // Keep appending until we rotate
        let mut rotated = false;
        for _ in 0..20 {
            let pos = manager.append(&record).await.unwrap();
            if pos.segment_id == 1 {
                rotated = true;
                break;
            }
        }

        assert!(rotated, "Should have rotated to segment 1");
    }

    #[tokio::test]
    async fn test_read_records() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Os,
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        // Write some records
        let records = vec![
            Record::put(b"key1".as_slice(), b"value1".as_slice()),
            Record::put(b"key2".as_slice(), b"value2".as_slice()),
            Record::delete(b"key1".as_slice()),
        ];

        for record in &records {
            manager.append(record).await.unwrap();
        }

        manager.sync().await.unwrap();

        // Read them back
        let mut reader = manager
            .read_from(Position {
                segment_id: 0,
                offset: 0,
            })
            .await
            .unwrap();

        let mut read_records = Vec::new();
        while let Some((record, _pos)) = reader.next_record().await.unwrap() {
            read_records.push(record);
        }

        assert_eq!(read_records.len(), 3);
        assert_eq!(read_records[0], records[0]);
        assert_eq!(read_records[1], records[1]);
        assert_eq!(read_records[2], records[2]);
        assert!(read_records[2].tombstone);
    }

    #[tokio::test]
    async fn test_concurrent_reads() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Os,
        };

        let manager = Arc::new(
            SegmentManager::new(config, Arc::new(NoopMeter), 1)
                .await
                .unwrap(),
        );

        // Write records
        for i in 0..10 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            manager.append(&record).await.unwrap();
        }

        manager.sync().await.unwrap();

        // Spawn multiple readers
        let mut handles = vec![];
        for _ in 0..3 {
            let mgr = manager.clone();
            let handle = tokio::spawn(async move {
                let mut reader = mgr
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
                count
            });
            handles.push(handle);
        }

        // All readers should see all records
        for handle in handles {
            let count = handle.await.unwrap();
            assert_eq!(count, 10);
        }
    }

    #[tokio::test]
    async fn test_fsync_policy_always() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Always,
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        // With Always policy, each append should fsync
        let record = Record::put(b"key".as_slice(), b"value".as_slice());
        manager.append(&record).await.unwrap();
        manager.append(&record).await.unwrap();

        // Verify data is persisted (implicit by successful append with Always policy)
        let mut reader = manager
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
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_fsync_policy_batch() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        let record = Record::put(b"key".as_slice(), b"value".as_slice());

        // First append should fsync
        manager.append(&record).await.unwrap();

        // Immediate second append should not fsync (within window)
        manager.append(&record).await.unwrap();

        // Wait for window to pass
        tokio::time::sleep(Duration::from_millis(15)).await;

        // This append should fsync again
        manager.append(&record).await.unwrap();

        // Verify all records persisted
        manager.sync().await.unwrap();
        let mut reader = manager
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
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_fsync_policy_os() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            max_segment_size: DEFAULT_SEGMENT_SIZE,
            dir: temp_dir.path().to_path_buf(),
            fsync_policy: FsyncPolicy::Os,
        };

        let manager = SegmentManager::new(config, Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        // With OS policy, appends don't fsync automatically
        let record = Record::put(b"key".as_slice(), b"value".as_slice());
        manager.append(&record).await.unwrap();
        manager.append(&record).await.unwrap();

        // Manual sync to ensure persistence for test verification
        manager.sync().await.unwrap();

        let mut reader = manager
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
        assert_eq!(count, 2);
    }
}
