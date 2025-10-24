//! WAL recovery with corruption detection and partial-tail truncation.
//!
//! Implements prefix-valid recovery strategy:
//! - Scans all segment files in order
//! - Validates CRC32C for each record
//! - Truncates partial/corrupt records at tail
//! - Emits CorruptionTruncated events when data is lost

use crate::record::{Record, RecordError};
use crate::segment::{Position, SegmentError};
use nori_observe::{Meter, VizEvent, WalEvt, WalKind};
use std::path::Path;
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Result of WAL recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInfo {
    /// Total number of valid records recovered.
    pub valid_records: u64,
    /// Number of segments scanned.
    pub segments_scanned: u64,
    /// Total bytes truncated due to corruption.
    pub bytes_truncated: u64,
    /// Position of the last valid record.
    pub last_valid_position: Option<Position>,
    /// Whether any corruption was detected and truncated.
    pub corruption_detected: bool,
}

/// Recovers WAL segments from a directory.
///
/// Scans all .wal files in order, validates each record's CRC32C,
/// and truncates any partial or corrupt records at the end of segments.
pub async fn recover(
    wal_dir: &Path,
    meter: Arc<dyn Meter>,
    node_id: u32,
) -> Result<RecoveryInfo, SegmentError> {
    let mut segments = find_all_segments(wal_dir).await?;
    segments.sort_unstable(); // Process in order

    let mut info = RecoveryInfo {
        valid_records: 0,
        segments_scanned: 0,
        bytes_truncated: 0,
        last_valid_position: None,
        corruption_detected: false,
    };

    for segment_id in segments {
        let segment_info = recover_segment(wal_dir, segment_id, meter.clone(), node_id).await?;

        info.valid_records += segment_info.valid_records;
        info.segments_scanned += 1;
        info.bytes_truncated += segment_info.bytes_truncated;

        if segment_info.bytes_truncated > 0 {
            info.corruption_detected = true;
        }

        if let Some(pos) = segment_info.last_valid_position {
            info.last_valid_position = Some(pos);
        }
    }

    Ok(info)
}

/// Information about a single segment's recovery.
struct SegmentRecoveryInfo {
    valid_records: u64,
    bytes_truncated: u64,
    last_valid_position: Option<Position>,
}

/// Recovers a single segment file.
async fn recover_segment(
    wal_dir: &Path,
    segment_id: u64,
    meter: Arc<dyn Meter>,
    node_id: u32,
) -> Result<SegmentRecoveryInfo, SegmentError> {
    let path = segment_path(wal_dir, segment_id);
    let mut file = File::open(&path).await?;

    let file_size = file.metadata().await?.len();
    let mut buffer = vec![0u8; file_size as usize];
    file.read_exact(&mut buffer).await?;

    let mut offset = 0u64;
    let mut valid_records = 0u64;
    let mut last_valid_offset = 0u64;

    // Scan records until we hit corruption or EOF
    while offset < file_size {
        let remaining = &buffer[offset as usize..];

        match Record::decode(remaining) {
            Ok((_record, size)) => {
                // Valid record
                valid_records += 1;
                offset += size as u64;
                last_valid_offset = offset;
            }
            Err(RecordError::Incomplete) => {
                // Partial record at tail - this is expected during recovery
                break;
            }
            Err(RecordError::CrcMismatch { .. }) => {
                // Corruption detected - truncate here
                break;
            }
            Err(_) => {
                // Other error (invalid compression, etc.) - truncate here
                break;
            }
        }
    }

    let bytes_truncated = file_size - last_valid_offset;

    // If we need to truncate, use atomic temp file + rename pattern
    if bytes_truncated > 0 {
        let temp_path = path.with_extension("wal.tmp");

        // Write valid data to temp file
        let mut temp_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .await?;

        temp_file.write_all(&buffer[..last_valid_offset as usize]).await?;
        temp_file.sync_all().await?;
        drop(temp_file);

        // Atomic rename: if this succeeds, the old file is replaced atomically
        // If we crash before this, the original file is unchanged
        tokio::fs::rename(&temp_path, &path).await?;

        // Emit corruption event
        meter.emit(VizEvent::Wal(WalEvt {
            node: node_id,
            seg: segment_id,
            kind: WalKind::CorruptionTruncated,
        }));
    }

    let last_valid_position = if valid_records > 0 {
        Some(Position {
            segment_id,
            offset: last_valid_offset,
        })
    } else {
        None
    };

    Ok(SegmentRecoveryInfo {
        valid_records,
        bytes_truncated,
        last_valid_position,
    })
}

/// Finds all segment files in a directory.
async fn find_all_segments(dir: &Path) -> Result<Vec<u64>, SegmentError> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut segment_ids = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "wal" {
                if let Some(stem) = path.file_stem() {
                    if let Some(stem_str) = stem.to_str() {
                        if let Ok(id) = stem_str.parse::<u64>() {
                            segment_ids.push(id);
                        }
                    }
                }
            }
        }
    }

    Ok(segment_ids)
}

/// Generates the path for a segment file.
fn segment_path(dir: &Path, id: u64) -> std::path::PathBuf {
    dir.join(format!("{:06}.wal", id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Record;
    use crate::segment::{SegmentConfig, SegmentManager};
    use nori_observe::NoopMeter;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_recovery_clean_segments() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Write some clean records
        let manager = SegmentManager::new(config.clone(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        for i in 0..10 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            manager.append(&record).await.unwrap();
        }

        manager.sync().await.unwrap();
        drop(manager);

        // Recover
        let info = recover(temp_dir.path(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        assert_eq!(info.valid_records, 10);
        assert_eq!(info.segments_scanned, 1);
        assert_eq!(info.bytes_truncated, 0);
        assert!(!info.corruption_detected);
        assert!(info.last_valid_position.is_some());
    }

    #[tokio::test]
    async fn test_recovery_with_partial_record() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Write some records
        let manager = SegmentManager::new(config.clone(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        for i in 0..5 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            manager.append(&record).await.unwrap();
        }

        manager.sync().await.unwrap();
        drop(manager);

        // Append garbage to simulate partial write
        let seg_path = temp_dir.path().join("000000.wal");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&seg_path)
            .await
            .unwrap();
        file.write_all(b"PARTIAL_GARBAGE_DATA").await.unwrap();
        file.sync_all().await.unwrap();

        // Recover - should truncate the garbage
        let info = recover(temp_dir.path(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        assert_eq!(info.valid_records, 5);
        assert_eq!(info.segments_scanned, 1);
        assert!(info.bytes_truncated > 0);
        assert!(info.corruption_detected);
    }

    #[tokio::test]
    async fn test_recovery_with_corrupted_crc() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Write some records
        let manager = SegmentManager::new(config.clone(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        for i in 0..5 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            manager.append(&record).await.unwrap();
        }

        manager.sync().await.unwrap();
        drop(manager);

        // Read the file and corrupt the last record's data (not CRC)
        let seg_path = temp_dir.path().join("000000.wal");
        let mut file_data = tokio::fs::read(&seg_path).await.unwrap();

        // Flip some bits in the last 20 bytes (likely in the last record's value)
        if file_data.len() > 20 {
            let corrupt_pos = file_data.len() - 15;
            file_data[corrupt_pos] ^= 0xFF;
            file_data[corrupt_pos + 1] ^= 0xFF;
        }

        // Write back the corrupted data
        tokio::fs::write(&seg_path, &file_data).await.unwrap();

        // Recover - should detect CRC mismatch and truncate
        let info = recover(temp_dir.path(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        // Should recover fewer than 5 records due to corruption
        assert!(info.valid_records < 5);
        assert!(info.bytes_truncated > 0);
        assert!(info.corruption_detected);
    }

    #[tokio::test]
    async fn test_recovery_empty_segment() {
        let temp_dir = TempDir::new().unwrap();

        // Create an empty segment file
        let seg_path = temp_dir.path().join("000000.wal");
        File::create(&seg_path).await.unwrap();

        let info = recover(temp_dir.path(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        assert_eq!(info.valid_records, 0);
        assert_eq!(info.segments_scanned, 1);
        assert_eq!(info.bytes_truncated, 0);
        assert!(!info.corruption_detected);
        assert!(info.last_valid_position.is_none());
    }

    #[tokio::test]
    async fn test_recovery_multiple_segments() {
        let temp_dir = TempDir::new().unwrap();
        let config = SegmentConfig {
            dir: temp_dir.path().to_path_buf(),
            max_segment_size: 100, // Force rotation
            ..Default::default()
        };

        // Write records across multiple segments
        let manager = SegmentManager::new(config.clone(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        for i in 0..20 {
            let key = format!("key{}", i);
            let record = Record::put(bytes::Bytes::from(key), b"value".as_slice());
            manager.append(&record).await.unwrap();
        }

        manager.sync().await.unwrap();
        drop(manager);

        // Recover all segments
        let info = recover(temp_dir.path(), Arc::new(NoopMeter), 1)
            .await
            .unwrap();

        assert_eq!(info.valid_records, 20);
        assert!(info.segments_scanned >= 2); // Should have rotated
        assert_eq!(info.bytes_truncated, 0);
        assert!(!info.corruption_detected);
    }
}
