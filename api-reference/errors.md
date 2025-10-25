---
layout: default
title: Errors & Types
parent: API Reference
nav_order: 4
---

# Errors & Types API
{: .no_toc }

Complete reference for error types and supporting structures in nori-wal.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Overview

nori-wal uses typed errors and result types for comprehensive error handling. All errors implement `std::error::Error` and can be converted to other error types using the `?` operator.

**Error Types:**
- `SegmentError` - Segment-level operations (WAL, file I/O)
- `RecordError` - Record encoding/decoding errors

**Supporting Types:**
- `Position` - Location in the WAL
- `RecoveryInfo` - Recovery statistics

---

## SegmentError

Main error type for WAL operations.

### Type Definition

[View source in `crates/nori-wal/src/segment.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/segment.rs#L20-L30)

### Variants

| Variant | Description | Source |
|---------|-------------|--------|
| `Io(io::Error)` | I/O error from filesystem operations | File open, read, write, fsync failures |
| `Record(RecordError)` | Record encoding/decoding error | Invalid record format, CRC mismatch |
| `NotFound(u64)` | Segment file not found | Segment ID doesn't exist |
| `InvalidConfig(String)` | Configuration validation error | Invalid `WalConfig` parameters |

### Error Conversions

`SegmentError` automatically converts from:
- `std::io::Error` (via `#[from]`)
- `RecordError` (via `#[from]`)

**Examples:**

```rust
use nori_wal::{Wal, WalConfig, SegmentError};

match Wal::open(config).await {
    Ok((wal, info)) => println!("Opened WAL with {} records", info.valid_records),
    Err(SegmentError::Io(e)) => {
        eprintln!("I/O error: {}", e);
        if e.kind() == std::io::ErrorKind::NotFound {
            eprintln!("Directory doesn't exist");
        }
    }
    Err(SegmentError::Record(e)) => {
        eprintln!("Record error during recovery: {}", e);
    }
    Err(SegmentError::NotFound(id)) => {
        eprintln!("Segment {} not found", id);
    }
    Err(SegmentError::InvalidConfig(msg)) => {
        eprintln!("Invalid configuration: {}", msg);
    }
}
```

---

### SegmentError::Io

I/O errors from filesystem operations.

**Common Causes:**

| Error Kind | Cause | Solution |
|------------|-------|----------|
| `NotFound` | Directory or segment doesn't exist | Create directory or check path |
| `PermissionDenied` | No write/read permissions | Fix file permissions |
| `NoSpaceLeft` | Disk full | Free up space or reduce segment size |
| `WriteZero` | Disk write failed | Check disk health |
| `UnexpectedEof` | File truncated mid-read | Corruption or crash during write |

**Examples:**

```rust
use std::io::ErrorKind;

match wal.append(&record).await {
    Ok(pos) => println!("Written at {:?}", pos),
    Err(SegmentError::Io(e)) => match e.kind() {
        ErrorKind::NoSpaceLeft => {
            eprintln!("Disk full! Cannot write more data.");
            // Trigger cleanup or alert
        }
        ErrorKind::PermissionDenied => {
            eprintln!("Permission denied. Check file permissions.");
        }
        _ => {
            eprintln!("I/O error: {}", e);
        }
    },
    Err(e) => eprintln!("Other error: {}", e),
}
```

---

### SegmentError::Record

Record-level errors during encoding or decoding.

[See RecordError documentation](#recorderror) for details.

**When it occurs:**
- During recovery: corrupt or incomplete records
- During append: encoding failures (rare)
- During read: decoding failures

**Examples:**

```rust
match wal.append(&record).await {
    Ok(_) => {}
    Err(SegmentError::Record(RecordError::CrcMismatch { expected, actual })) => {
        eprintln!("Data corruption: CRC expected {:#x}, got {:#x}", expected, actual);
    }
    Err(SegmentError::Record(RecordError::Incomplete)) => {
        eprintln!("Incomplete record at end of segment");
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

### SegmentError::NotFound

Segment file not found.

**When it occurs:**
- Reading from a segment ID that doesn't exist
- Deleted segment files while WAL is running
- Incorrect segment directory

**Examples:**

```rust
let position = Position { segment_id: 999, offset: 0 };

match wal.read_from(position).await {
    Ok(reader) => { /* ... */ }
    Err(SegmentError::NotFound(id)) => {
        eprintln!("Segment {} not found. Was it deleted?", id);
        // Either the segment was garbage collected or position is invalid
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

### SegmentError::InvalidConfig

Configuration validation failed.

**Common Validation Errors:**

| Configuration | Invalid Value | Error Message |
|---------------|---------------|---------------|
| `max_segment_size` | `0` | "max_segment_size must be greater than 0" |
| `max_segment_size` | `< 1MB` | "max_segment_size should be at least 1MB for reasonable performance" |
| `fsync_policy` | `Batch(Duration::ZERO)` | "fsync batch window cannot be zero - use FsyncPolicy::Always instead" |
| `fsync_policy` | `Batch(> 1 second)` | "fsync batch window should be less than 1 second to avoid excessive data loss risk" |

**Examples:**

```rust
use nori_wal::{WalConfig, FsyncPolicy, SegmentError};
use std::time::Duration;

let config = WalConfig {
    max_segment_size: 0, // Invalid!
    ..Default::default()
};

match Wal::open(config).await {
    Ok(_) => {}
    Err(SegmentError::InvalidConfig(msg)) => {
        eprintln!("Configuration error: {}", msg);
        // Fix configuration based on error message
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

## RecordError

Errors related to record encoding and decoding.

### Type Definition

[View source in `crates/nori-wal/src/record.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L17-L31)

### Variants

| Variant | Description | When It Occurs |
|---------|-------------|----------------|
| `Io(io::Error)` | I/O error (e.g., varint overflow) | Malformed varint, buffer issues |
| `CrcMismatch { expected, actual }` | Checksum validation failed | Data corruption |
| `InvalidCompression(u8)` | Unknown compression type | Corrupt flags byte |
| `CompressionFailed(String)` | Compression error | Should never happen |
| `DecompressionFailed(String)` | Decompression error | Corrupt compressed data |
| `Incomplete` | Not enough bytes to decode | Truncated record |

---

### RecordError::Io

I/O errors during encoding/decoding.

**Common Causes:**
- Varint overflow (varint too long)
- Buffer underflow during decode

**Examples:**

```rust
use nori_wal::{Record, RecordError};

let corrupted_data = &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

match Record::decode(corrupted_data) {
    Ok(_) => {}
    Err(RecordError::Io(e)) => {
        eprintln!("I/O error: {}", e);
        // Likely a varint overflow from malformed data
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

---

### RecordError::CrcMismatch

CRC32C checksum validation failed - data corruption detected.

**Fields:**
- `expected: u32` - CRC stored in the record
- `actual: u32` - CRC calculated from data

**When it occurs:**
- Disk corruption
- Memory corruption
- Incomplete write (crash mid-write)
- Bit flips (hardware issues)

**Examples:**

```rust
match Record::decode(data) {
    Ok((record, size)) => {
        // CRC validated automatically
        println!("Valid record: {:?}", record);
    }
    Err(RecordError::CrcMismatch { expected, actual }) => {
        eprintln!("CORRUPTION DETECTED!");
        eprintln!("  Expected CRC: {:#010x}", expected);
        eprintln!("  Actual CRC:   {:#010x}", actual);
        eprintln!("  XOR diff:     {:#010x}", expected ^ actual);

        // This record is corrupt and should be discarded
        // Recovery will truncate here
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

**What to do:**
1. Log the error with offset/position
2. Skip this record during recovery
3. Truncate segment at this point
4. Alert operators (possible hardware issue)

---

### RecordError::InvalidCompression

Unknown compression type in flags byte.

**When it occurs:**
- Corrupt flags byte (bits 2-3)
- Reading data written by incompatible version

**Valid compression types:**
- `0` = None
- `1` = LZ4
- `2` = Zstd
- `3` = Reserved (future use)

**Examples:**

```rust
match Record::decode(data) {
    Ok(_) => {}
    Err(RecordError::InvalidCompression(typ)) => {
        eprintln!("Unknown compression type: {}", typ);
        eprintln!("Valid types: 0 (None), 1 (LZ4), 2 (Zstd)");
        // Data is corrupt or from incompatible version
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

### RecordError::CompressionFailed

Compression failed during encoding.

**When it occurs:**
- Should never happen in practice
- LZ4/Zstd libraries return error

**Examples:**

```rust
let record = Record::put(b"key", large_value)
    .with_compression(Compression::Lz4);

match wal.append(&record).await {
    Ok(_) => {}
    Err(SegmentError::Record(RecordError::CompressionFailed(msg))) => {
        eprintln!("Compression failed: {}", msg);
        // This is very rare - may indicate memory issues
        // Fall back to uncompressed write
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

### RecordError::DecompressionFailed

Decompression failed during decoding.

**When it occurs:**
- Corrupt compressed data
- Truncated compressed value
- Memory allocation failure

**Examples:**

```rust
match Record::decode(data) {
    Ok(_) => {}
    Err(RecordError::DecompressionFailed(msg)) => {
        eprintln!("Decompression failed: {}", msg);
        // Compressed data is corrupt
        // Recovery should truncate here
    }
    Err(e) => eprintln!("Error: {}", e),
}
```

---

### RecordError::Incomplete

Not enough bytes to decode a complete record.

**When it occurs:**
- Reading from end of segment
- Truncated write (crash mid-append)
- Streaming decode with partial data

**Examples:**

```rust
let mut offset = 0;
let buffer = read_segment_file()?;

loop {
    match Record::decode(&buffer[offset..]) {
        Ok((record, size)) => {
            println!("Record: {:?}", record);
            offset += size;
        }
        Err(RecordError::Incomplete) => {
            // Reached end of valid data
            // This is normal at end of segment
            println!("End of valid records at offset {}", offset);
            break;
        }
        Err(RecordError::CrcMismatch { .. }) => {
            // Corruption - stop here
            eprintln!("Corruption at offset {}", offset);
            break;
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            break;
        }
    }
}
```

---

## Position

Location within the WAL (segment ID + byte offset).

### Type Definition

[View source in `crates/nori-wal/src/segment.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/segment.rs#L32-L37)

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `segment_id` | `u64` | Segment file number (0, 1, 2, ...) |
| `offset` | `u64` | Byte offset within the segment |

### Ordering

`Position` implements `Ord` and can be compared:

```rust
use nori_wal::Position;

let pos1 = Position { segment_id: 0, offset: 100 };
let pos2 = Position { segment_id: 0, offset: 200 };
let pos3 = Position { segment_id: 1, offset: 0 };

assert!(pos1 < pos2);  // Same segment, earlier offset
assert!(pos2 < pos3);  // Earlier segment
```

### Usage

**Starting position for reading:**

```rust
// Read from beginning
let start = Position { segment_id: 0, offset: 0 };
let mut reader = wal.read_from(start).await?;

// Read from specific position
let checkpoint = Position { segment_id: 5, offset: 1024 };
let mut reader = wal.read_from(checkpoint).await?;
```

**Saving checkpoints:**

```rust
// Save position for resume
let pos = wal.append(&record).await?;
save_checkpoint(pos);

// Later, resume from checkpoint
let checkpoint = load_checkpoint();
let mut reader = wal.read_from(checkpoint).await?;
```

---

## RecoveryInfo

Statistics about WAL recovery operation.

### Type Definition

[View source in `crates/nori-wal/src/recovery.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/recovery.rs#L17-L30)

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `valid_records` | `u64` | Total valid records recovered |
| `segments_scanned` | `u64` | Number of segment files scanned |
| `bytes_truncated` | `u64` | Bytes removed due to corruption |
| `last_valid_position` | `Option<Position>` | Position of last valid record |
| `corruption_detected` | `bool` | Whether any corruption was found |

### Usage

**After opening WAL:**

```rust
let (wal, info) = Wal::open(config).await?;

println!("Recovery complete:");
println!("  Records recovered: {}", info.valid_records);
println!("  Segments scanned: {}", info.segments_scanned);
println!("  Bytes truncated: {}", info.bytes_truncated);

if info.corruption_detected {
    eprintln!("WARNING: Corruption detected and truncated");
    eprintln!("  Lost {} bytes of data", info.bytes_truncated);

    // Alert operators
    send_alert("WAL corruption detected");
}

if let Some(pos) = info.last_valid_position {
    println!("Last valid record at segment {}, offset {}",
        pos.segment_id, pos.offset);
}
```

**Monitoring recovery metrics:**

```rust
let (wal, info) = Wal::open(config).await?;

// Log to metrics system
metrics.gauge("wal.recovery.valid_records", info.valid_records);
metrics.gauge("wal.recovery.bytes_truncated", info.bytes_truncated);
metrics.gauge("wal.recovery.corruption_detected", info.corruption_detected as u64);

// Alert if significant data loss
if info.bytes_truncated > 1024 * 1024 {  // > 1 MB lost
    alert_ops("Significant WAL data loss during recovery");
}
```

---

## Error Handling Patterns

### Pattern 1: Graceful Degradation

```rust
async fn write_with_fallback(wal: &Wal, record: &Record) -> Result<Position, AppError> {
    match wal.append(record).await {
        Ok(pos) => Ok(pos),
        Err(SegmentError::Io(e)) if e.kind() == ErrorKind::NoSpaceLeft => {
            // Trigger emergency cleanup
            wal.delete_segments_before(old_checkpoint).await?;

            // Retry
            wal.append(record).await.map_err(Into::into)
        }
        Err(e) => Err(e.into()),
    }
}
```

### Pattern 2: Retry with Backoff

```rust
async fn append_with_retry(
    wal: &Wal,
    record: &Record,
    max_retries: u32
) -> Result<Position, SegmentError> {
    let mut retries = 0;
    let mut delay = Duration::from_millis(10);

    loop {
        match wal.append(record).await {
            Ok(pos) => return Ok(pos),
            Err(SegmentError::Io(e)) if e.kind() == ErrorKind::Interrupted => {
                // Transient error, retry
                if retries >= max_retries {
                    return Err(SegmentError::Io(e));
                }
                retries += 1;
                tokio::time::sleep(delay).await;
                delay *= 2;  // Exponential backoff
            }
            Err(e) => return Err(e),  // Don't retry other errors
        }
    }
}
```

### Pattern 3: Error Classification

```rust
#[derive(Debug)]
enum ErrorSeverity {
    Transient,  // Retry
    Fatal,      // Abort
    Corruption, // Alert + truncate
}

fn classify_error(err: &SegmentError) -> ErrorSeverity {
    match err {
        SegmentError::Io(e) if e.kind() == ErrorKind::Interrupted => {
            ErrorSeverity::Transient
        }
        SegmentError::Record(RecordError::CrcMismatch { .. }) => {
            ErrorSeverity::Corruption
        }
        SegmentError::InvalidConfig(_) => {
            ErrorSeverity::Fatal
        }
        _ => ErrorSeverity::Fatal,
    }
}
```

### Pattern 4: Detailed Error Context

```rust
use thiserror::Error;

#[derive(Debug, Error)]
enum AppError {
    #[error("WAL operation failed at position {position:?}: {source}")]
    WalError {
        position: Position,
        #[source]
        source: SegmentError,
    },

    #[error("Recovery failed after {attempts} attempts: {source}")]
    RecoveryError {
        attempts: u32,
        #[source]
        source: SegmentError,
    },
}

// Usage
let pos = wal.current_position().await;
wal.append(&record).await.map_err(|e| AppError::WalError {
    position: pos,
    source: e,
})?;
```

---

## See Also

- [Wal API](wal) - Main WAL interface
- [Record API](record) - Record operations
- [Configuration](config) - Configuration options
- [Recovery Guarantees](../core-concepts/recovery-guarantees) - Recovery behavior
- [Troubleshooting](../troubleshooting/) - Common issues and solutions
