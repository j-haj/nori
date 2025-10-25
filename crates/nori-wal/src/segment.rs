//! WAL segment file management with automatic rotation at 128MB.
//!
//! Segments are numbered sequentially (e.g., 000000.wal, 000001.wal) and rotated
//! when they reach the configured size limit (default 128MB).

use crate::record::Record;
use nori_observe::{Meter, VizEvent, WalEvt, WalKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
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
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
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
        // Default to batch with 5ms window
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
    ///
    /// If `preallocate_size` is Some and this is a new file, it will be pre-allocated
    /// to the given size to prevent "no space left" errors and improve filesystem locality.
    async fn open(dir: &Path, id: u64, create: bool, preallocate_size: Option<u64>) -> Result<Self, SegmentError> {
        let path = segment_path(dir, id);

        let mut file = if create {
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
        let actual_data_size = metadata.len();

        // Pre-allocate space for new files (but track actual data written separately)
        let logical_size = if actual_data_size == 0 && create {
            if let Some(target_size) = preallocate_size {
                // Use set_len to pre-allocate disk space
                file.set_len(target_size).await?;
                file.sync_all().await?;
                // Seek back to beginning since we'll be writing from offset 0
                file.seek(std::io::SeekFrom::Start(0)).await?;
            }
            0 // Logical size is still 0, we've just reserved space
        } else {
            // Existing file: seek to end and use actual size
            if actual_data_size > 0 {
                file.seek(std::io::SeekFrom::End(0)).await?;
            }
            actual_data_size
        };

        Ok(Self {
            id,
            file,
            size: logical_size,
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

    /// Finalizes the segment by truncating it to actual written size.
    /// This is important when pre-allocation is used.
    async fn finalize(&mut self) -> Result<(), SegmentError> {
        self.file.set_len(self.size).await?;
        self.file.sync_all().await?;
        Ok(())
    }
}

/// Simple LRU cache for segment file descriptors.
struct FdCache {
    cache: HashMap<u64, Arc<Mutex<File>>>,
    max_size: usize,
    access_order: Vec<u64>,
}

impl FdCache {
    fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_size,
            access_order: Vec::new(),
        }
    }

    async fn get_or_open(&mut self, segment_id: u64, dir: &Path) -> Result<Arc<Mutex<File>>, SegmentError> {
        // Check if already in cache
        if let Some(file) = self.cache.get(&segment_id) {
            // Update access order
            self.access_order.retain(|&id| id != segment_id);
            self.access_order.push(segment_id);
            return Ok(file.clone());
        }

        // Open new file
        let path = segment_path(dir, segment_id);
        let file = File::open(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SegmentError::NotFound(segment_id)
            } else {
                SegmentError::Io(e)
            }
        })?;

        let file_arc = Arc::new(Mutex::new(file));

        // Evict LRU if at capacity
        if self.cache.len() >= self.max_size {
            if let Some(&lru_id) = self.access_order.first() {
                self.cache.remove(&lru_id);
                self.access_order.remove(0);
            }
        }

        // Insert into cache
        self.cache.insert(segment_id, file_arc.clone());
        self.access_order.push(segment_id);

        Ok(file_arc)
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
    fd_cache: Arc<Mutex<FdCache>>,
}

impl Drop for SegmentManager {
    fn drop(&mut self) {
        // Best-effort finalization on drop
        // We can't do async work here, so we use blocking operations
        if let Ok(current) = self.current.try_lock() {
            // Use std::fs to do synchronous truncation
            if let Ok(metadata) = std::fs::metadata(&current.path) {
                if metadata.len() != current.size {
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&current.path)
                        .and_then(|f| {
                            f.set_len(current.size)?;
                            f.sync_all()
                        });
                }
            }
        }
    }
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
        // TODO: Enable pre-allocation once we have platform-specific fallocate support
        let segment = SegmentFile::open(&config.dir, latest_id, true, None).await?;

        Ok(Self {
            config,
            current: Arc::new(Mutex::new(segment)),
            current_id: Arc::new(Mutex::new(latest_id)),
            meter,
            node_id,
            last_fsync: Arc::new(Mutex::new(None)),
            fd_cache: Arc::new(Mutex::new(FdCache::new(32))), // Cache up to 32 segment FDs
        })
    }

    /// Deletes all segments before the given position.
    ///
    /// This is used for garbage collection after data has been compacted or
    /// replicated. Any segment with ID < position.segment_id will be deleted.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the data in these segments is no longer needed
    /// (e.g., it has been compacted into SSTables or safely replicated).
    pub async fn delete_segments_before(&self, position: Position) -> Result<u64, SegmentError> {
        let current_id = *self.current_id.lock().await;

        // Don't delete the current segment
        if position.segment_id >= current_id {
            return Ok(0);
        }

        let mut deleted_count = 0u64;
        let mut entries = tokio::fs::read_dir(&self.config.dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Parse segment ID from filename
            if let Some(id) = parse_segment_id_from_path(&path) {
                // Delete if this segment is before the cutoff position
                if id < position.segment_id {
                    tokio::fs::remove_file(&path).await?;
                    deleted_count += 1;

                    self.meter.emit(VizEvent::Wal(WalEvt {
                        node: self.node_id,
                        seg: id,
                        kind: WalKind::SegmentGc,
                    }));
                }
            }
        }

        Ok(deleted_count)
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
        self.apply_fsync_policy(&mut current, segment_id).await?;

        Ok(Position { segment_id, offset })
    }

    /// Appends a batch of records to the WAL.
    ///
    /// More efficient than individual appends because:
    /// - Lock held only once for entire batch
    /// - Single fsync for entire batch (if policy is Always)
    /// - No interleaving with other writers
    pub async fn append_batch(&self, records: &[Record]) -> Result<Vec<Position>, SegmentError> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        // Calculate total size needed
        let total_size: usize = records.iter().map(|r| r.encode().len()).sum();

        let mut current = self.current.lock().await;
        let mut positions = Vec::with_capacity(records.len());

        // Check if we need to rotate before starting batch
        if current.would_exceed(total_size, self.config.max_segment_size) {
            drop(current);
            self.rotate().await?;
            current = self.current.lock().await;
        }

        // Append all records
        for record in records {
            let offset = current.append(record).await?;
            positions.push(Position {
                segment_id: current.id,
                offset,
            });
        }

        let segment_id = current.id;

        // Apply fsync policy once for entire batch
        self.apply_fsync_policy(&mut current, segment_id).await?;

        Ok(positions)
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

        // Finalize and get metadata from old segment
        let mut old_segment = self.current.lock().await;
        let old_size = old_segment.size;
        let old_id = old_segment.id;

        // Truncate old segment to actual written size (important for pre-allocated files)
        old_segment.finalize().await?;
        drop(old_segment);

        self.meter.emit(VizEvent::Wal(WalEvt {
            node: self.node_id,
            seg: old_id,
            kind: WalKind::SegmentRoll { bytes: old_size },
        }));

        // Create new segment
        // TODO: Enable pre-allocation once we have platform-specific fallocate support
        let new_segment = SegmentFile::open(&self.config.dir, new_id, true, None).await?;

        // Swap in the new segment
        let mut current = self.current.lock().await;
        *current = new_segment;
        *current_id = new_id;

        Ok(())
    }

    /// Applies the configured fsync policy to the current segment.
    ///
    /// Handles all three fsync policies (Always, Batch, Os) and emits
    /// appropriate observability events.
    async fn apply_fsync_policy(
        &self,
        current: &mut SegmentFile,
        segment_id: u64,
    ) -> Result<(), SegmentError> {
        match self.config.fsync_policy {
            FsyncPolicy::Always => {
                self.fsync_with_timing(current, segment_id).await
            }
            FsyncPolicy::Batch(window) => {
                self.fsync_if_window_elapsed(current, segment_id, window).await
            }
            FsyncPolicy::Os => {
                // No fsync - let OS handle it
                Ok(())
            }
        }
    }

    /// Performs fsync and emits timing event.
    async fn fsync_with_timing(
        &self,
        current: &mut SegmentFile,
        segment_id: u64,
    ) -> Result<(), SegmentError> {
        let start = Instant::now();
        current.sync().await?;
        let elapsed_ms = start.elapsed().as_millis() as u32;

        self.meter.emit(VizEvent::Wal(WalEvt {
            node: self.node_id,
            seg: segment_id,
            kind: WalKind::Fsync { ms: elapsed_ms },
        }));

        Ok(())
    }

    /// Performs fsync if the time window has elapsed since last sync.
    async fn fsync_if_window_elapsed(
        &self,
        current: &mut SegmentFile,
        segment_id: u64,
        window: Duration,
    ) -> Result<(), SegmentError> {
        let mut last_sync = self.last_fsync.lock().await;
        let should_sync = match *last_sync {
            None => true,
            Some(last) => last.elapsed() >= window,
        };

        if should_sync {
            // Update timestamp BEFORE fsync to prevent multiple concurrent fsyncs
            let fsync_start = Instant::now();
            *last_sync = Some(fsync_start);
            drop(last_sync); // Release lock before expensive fsync

            current.sync().await?;
            let elapsed_ms = fsync_start.elapsed().as_millis() as u32;

            self.meter.emit(VizEvent::Wal(WalEvt {
                node: self.node_id,
                seg: segment_id,
                kind: WalKind::Fsync { ms: elapsed_ms },
            }));
        }

        Ok(())
    }

    /// Reads records from a segment starting at the given position.
    pub async fn read_from(&self, position: Position) -> Result<SegmentReader, SegmentError> {
        // Get file from cache (or open if not cached)
        let mut cache = self.fd_cache.lock().await;
        let file_arc = cache.get_or_open(position.segment_id, &self.config.dir).await?;
        drop(cache); // Release cache lock

        Ok(SegmentReader {
            file: file_arc,
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

    /// Finalizes the current segment by truncating to actual written size.
    /// Should be called before closing the WAL.
    pub async fn finalize_current(&self) -> Result<(), SegmentError> {
        let mut current = self.current.lock().await;
        current.finalize().await
    }
}

/// Iterator for reading records from a segment.
pub struct SegmentReader {
    file: Arc<Mutex<File>>,
    position: u64,
    segment_id: u64,
}

impl SegmentReader {
    /// Reads the next record from the segment.
    pub async fn next_record(&mut self) -> Result<Option<(Record, Position)>, SegmentError> {
        const READ_BUFFER_SIZE: usize = 65536; // 64KB buffer for better performance

        let mut file = self.file.lock().await;

        // Seek to the current position
        file.seek(std::io::SeekFrom::Start(self.position)).await?;

        // Read initial buffer
        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        let n = file.read(&mut buffer).await?;

        if n == 0 {
            return Ok(None); // EOF
        }

        buffer.truncate(n);

        // Try to decode the record, reading more data if needed
        match self.try_decode_with_retry(&mut buffer, &mut file, n, READ_BUFFER_SIZE).await? {
            Some((record, size)) => {
                let pos = Position {
                    segment_id: self.segment_id,
                    offset: self.position,
                };
                self.position += size as u64;
                Ok(Some((record, pos)))
            }
            None => Ok(None),
        }
    }

    /// Attempts to decode a record, reading more data if the initial buffer is incomplete.
    async fn try_decode_with_retry(
        &self,
        buffer: &mut Vec<u8>,
        file: &mut File,
        bytes_read: usize,
        buffer_size: usize,
    ) -> Result<Option<(Record, usize)>, SegmentError> {
        match Record::decode(buffer) {
            Ok((record, size)) => Ok(Some((record, size))),
            Err(crate::record::RecordError::Incomplete) if bytes_read == buffer_size => {
                // Buffer was full but record is incomplete - read more data
                self.read_more_and_decode(buffer, file, buffer_size).await
            }
            Err(crate::record::RecordError::Incomplete) => {
                // Incomplete at EOF - this is fine during recovery
                Ok(None)
            }
            Err(e) => Err(SegmentError::Record(e)),
        }
    }

    /// Reads additional data and attempts to decode again.
    async fn read_more_and_decode(
        &self,
        buffer: &mut Vec<u8>,
        file: &mut File,
        buffer_size: usize,
    ) -> Result<Option<(Record, usize)>, SegmentError> {
        let mut more_data = vec![0u8; buffer_size];
        let additional = file.read(&mut more_data).await?;
        buffer.extend_from_slice(&more_data[..additional]);

        match Record::decode(buffer) {
            Ok((record, size)) => Ok(Some((record, size))),
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
        if let Some(id) = parse_segment_id_from_path(&entry.path()) {
            max_id = max_id.max(id);
        }
    }

    Ok(max_id)
}

/// Parses a segment ID from a .wal file path.
///
/// Returns None if the path is not a valid .wal file or cannot be parsed.
fn parse_segment_id_from_path(path: &Path) -> Option<u64> {
    // Check extension is "wal"
    if path.extension()?.to_str()? != "wal" {
        return None;
    }

    // Parse the filename stem as a u64
    let stem_str = path.file_stem()?.to_str()?;
    stem_str.parse::<u64>().ok()
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
