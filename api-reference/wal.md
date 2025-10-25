---
layout: default
title: Wal
parent: API Reference
nav_order: 0
---

# Wal
{: .no_toc }

Main WAL interface for append-only logging with automatic recovery and rotation.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Type Definition

```rust
pub struct Wal: Send + Sync
```

The main write-ahead log type. Provides methods for:
- Appending records
- Syncing to disk
- Reading records
- Garbage collection

**Thread Safety**: `Wal` is `Send + Sync` and can be safely shared across threads using `Arc<Wal>`.

---

## Constructor Methods

### `Wal::open`

```rust
pub async fn open(config: WalConfig) -> Result<(Self, RecoveryInfo), SegmentError>
```

Opens a WAL, performing automatic recovery if needed.

**Parameters**:
- `config`: Configuration for the WAL

**Returns**:
- `Ok((Wal, RecoveryInfo))`: Opened WAL and recovery statistics
- `Err(SegmentError)`: If opening failed

**What it does**:
1. Validates configuration
2. Creates WAL directory if it doesn't exist
3. Scans existing segments and recovers valid records
4. Truncates any corrupted data
5. Resumes writing from the last valid position

**Example**:
```rust
use nori_wal::{Wal, WalConfig};

let config = WalConfig::default();
let (wal, recovery_info) = Wal::open(config).await?;

println!("Recovered {} records", recovery_info.valid_records);

if recovery_info.corruption_detected {
    log::warn!("Truncated {} bytes", recovery_info.bytes_truncated);
}
```

**Errors**:
- `SegmentError::InvalidConfig`: Invalid configuration
- `SegmentError::Io`: I/O error (disk full, permissions, etc.)

---

### `Wal::open_with_meter`

```rust
pub async fn open_with_meter(
    config: WalConfig,
    meter: Arc<dyn Meter>,
) -> Result<(Self, RecoveryInfo), SegmentError>
```

Opens a WAL with custom observability.

**Parameters**:
- `config`: Configuration for the WAL
- `meter`: Custom metrics collector (implements `nori_observe::Meter`)

**Returns**: Same as `Wal::open`

**Use when**:
- Integrating with custom metrics systems
- Emitting events to dashboards
- Advanced monitoring/debugging

**Example**:
```rust
use nori_wal::Wal;
use std::sync::Arc;

let meter = Arc::new(MyCustomMeter::new());
let (wal, info) = Wal::open_with_meter(config, meter).await?;
```

---

## Write Methods

### `append`

```rust
pub async fn append(&self, record: &Record) -> Result<Position, SegmentError>
```

Appends a single record to the WAL.

**Parameters**:
- `record`: The record to append

**Returns**:
- `Ok(Position)`: Where the record was written (segment ID + offset)
- `Err(SegmentError)`: If append failed

**Behavior**:
- Record is encoded and written to the active segment
- Segment rotates automatically if size limit reached
- Fsync behavior depends on `FsyncPolicy`
- Thread-safe: multiple threads can call concurrently (serialized internally)

**Example**:
```rust
use nori_wal::{Record, Position};

let record = Record::put(b"user:123", b"alice@example.com");
let position = wal.append(&record).await?;

println!("Wrote record at {:?}", position);
// Output: Wrote record at Position { segment_id: 0, offset: 1024 }
```

**Performance**:
- Without fsync: ~10-50μs
- With fsync (Always policy): ~1-5ms
- With fsync (Batch policy): ~10-50μs (most writes), ~1-5ms (periodic)

---

### `append_batch`

```rust
pub async fn append_batch(&self, records: &[Record]) -> Result<Vec<Position>, SegmentError>
```

Appends multiple records in a batch.

**Parameters**:
- `records`: Slice of records to append

**Returns**:
- `Ok(Vec<Position>)`: Positions where each record was written
- `Err(SegmentError)`: If batch append failed

**Benefits over repeated `append()`**:
- Acquires lock only once (not once per record)
- Single fsync for entire batch (with `Always` policy)
- Sequential writes without interleaving from other threads

**Example**:
```rust
let records = vec![
    Record::put(b"key1", b"value1"),
    Record::put(b"key2", b"value2"),
    Record::put(b"key3", b"value3"),
];

let positions = wal.append_batch(&records).await?;

for (i, pos) in positions.iter().enumerate() {
    println!("Record {} at {:?}", i, pos);
}
```

**Performance**:
```
Single append:      1000 records = 1000 lock acquisitions, 1000 fsyncs (Always)
Batch append:       1000 records = 1 lock acquisition, 1 fsync (Always)
Speedup:            ~100-1000x for large batches with Always policy
```

---

## Sync Methods

### `flush`

```rust
pub async fn flush(&self) -> Result<(), SegmentError>
```

Flushes buffered data to the OS (but doesn't call fsync).

**Use when**:
- You want data in OS cache but don't need disk persistence yet
- Rarely needed (OS handles buffering efficiently)

**Example**:
```rust
wal.append(&record).await?;
wal.flush().await?;  // Data in OS cache, not on disk yet
```

**Warning**: `flush()` does NOT guarantee durability! Use `sync()` for that.

---

### `sync`

```rust
pub async fn sync(&self) -> Result<(), SegmentError>
```

Syncs all data to physical disk (fsync).

**Guarantees**:
- All previously appended records are durable
- Survives power failure after this returns

**Use when**:
- Using `FsyncPolicy::Os` and need manual durability
- End of a logical transaction
- Before shutting down

**Example**:
```rust
// Write a batch
for record in records {
    wal.append(&record).await?;
}

// Ensure all durable
wal.sync().await?;
```

**Performance**: ~1-5ms on SSD

---

## Read Methods

### `read_from`

```rust
pub async fn read_from(
    &self,
    position: Position,
) -> Result<SegmentReader, SegmentError>
```

Creates a reader starting at the given position.

**Parameters**:
- `position`: Where to start reading (segment ID + offset)

**Returns**:
- `Ok(SegmentReader)`: Iterator over records
- `Err(SegmentError)`: If position is invalid or segment missing

**Example**:
```rust
use nori_wal::Position;

// Read from beginning
let mut reader = wal.read_from(Position { segment_id: 0, offset: 0 }).await?;

while let Some((record, position)) = reader.next_record().await? {
    println!("Record at {:?}: key={:?}", position, record.key);
}
```

**Use cases**:
- Replaying the entire log (position = `Position::start()`)
- Resuming from last processed position
- Replication (followers read from leader's position)

**Performance**: ~200K records/sec sequential read

---

### `current_position`

```rust
pub async fn current_position(&self) -> Position
```

Returns the current write position (where next record will be written).

**Returns**: `Position` of the next write

**Example**:
```rust
let pos_before = wal.current_position().await;
wal.append(&record).await?;
let pos_after = wal.current_position().await;

assert!(pos_after > pos_before);
```

**Use when**:
- Tracking progress for replication
- Checkpointing
- Testing

---

## Management Methods

### `delete_segments_before`

```rust
pub async fn delete_segments_before(&self, position: Position) -> Result<u64, SegmentError>
```

Deletes all segments before the given position.

**Parameters**:
- `position`: Delete all segments before this position

**Returns**:
- `Ok(count)`: Number of segments deleted
- `Err(SegmentError)`: If deletion failed

**Safety Requirements**:
**IMPORTANT**: Caller must ensure data is no longer needed before deleting!

Typical workflow:
1. Compact old segments into new format (e.g., SSTables)
2. Verify compaction succeeded
3. Delete old segments

**Example**:
```rust
// Compact segments 0-9 into SSTable
compact_to_sstable(0..10).await?;

// Safe to delete old segments
let cutoff = Position { segment_id: 10, offset: 0 };
let deleted = wal.delete_segments_before(cutoff).await?;

println!("Deleted {} old segments", deleted);
```

**What gets deleted**:
```
Before:
  000000.wal  ← Delete
  000001.wal  ← Delete
  ...
  000009.wal  ← Delete
  000010.wal  ← Keep (cutoff segment)
  000011.wal  ← Keep (active segment)

After:
  000010.wal
  000011.wal
```

**Cannot delete**:
- Active segment (currently being written to)
- Segments at or after the cutoff position

---

### `close`

```rust
pub async fn close(self) -> Result<(), SegmentError>
```

Gracefully closes the WAL, ensuring all data is durable.

**What it does**:
1. Syncs any pending data to disk
2. Finalizes the current segment (truncates to actual size)
3. Consumes the `Wal` (can't be used after)

**Use when**:
- Shutting down application
- Want to ensure clean shutdown

**Example**:
```rust
// Write data
wal.append(&record).await?;

// Graceful shutdown
wal.close().await?;

// wal is consumed, can't use anymore
```

**Alternative**: Just drop the `Wal`. The `Drop` impl will do best-effort finalization.

---

## Accessor Methods

### `config`

```rust
pub fn config(&self) -> &WalConfig
```

Returns a reference to the WAL configuration.

**Returns**: `&WalConfig`

**Example**:
```rust
let cfg = wal.config();
println!("Max segment size: {} MB", cfg.max_segment_size / (1024 * 1024));
println!("Fsync policy: {:?}", cfg.fsync_policy);
```

---

## Complete Example

```rust
use nori_wal::{Wal, WalConfig, Record, Position, FsyncPolicy};
use std::time::Duration;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Configure WAL
    let config = WalConfig {
        dir: PathBuf::from("/var/lib/myapp/wal"),
        max_segment_size: 256 * 1024 * 1024,  // 256 MB
        fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
        preallocate: true,
        node_id: 1,
    };

    // 2. Open with recovery
    let (wal, recovery_info) = Wal::open(config).await?;

    println!("Recovered {} records", recovery_info.valid_records);

    // 3. Write records
    let records = vec![
        Record::put(b"user:1", b"alice@example.com"),
        Record::put(b"user:2", b"bob@example.com"),
        Record::delete(b"user:1"),  // Tombstone
    ];

    let positions = wal.append_batch(&records).await?;

    // 4. Explicitly sync (if using Os policy)
    wal.sync().await?;

    // 5. Read back
    let mut reader = wal.read_from(Position { segment_id: 0, offset: 0 }).await?;

    while let Some((record, position)) = reader.next_record().await? {
        if record.tombstone {
            println!("DELETE {:?} at {:?}", record.key, position);
        } else {
            println!("PUT {:?}={:?} at {:?}", record.key, record.value, position);
        }
    }

    // 6. Garbage collection (after compaction)
    // let deleted = wal.delete_segments_before(cutoff).await?;

    // 7. Graceful shutdown
    wal.close().await?;

    Ok(())
}
```

---

## Error Handling

All methods return `Result<T, SegmentError>`. Common errors:

| Error | Cause | How to Handle |
|-------|-------|---------------|
| `SegmentError::Io(e)` | Disk full, permissions, etc. | Check disk space, permissions |
| `SegmentError::InvalidConfig(msg)` | Bad configuration | Fix config values |
| `SegmentError::NotFound(id)` | Segment doesn't exist | Normal after deletion |
| `SegmentError::Record(e)` | Record decode error | Check for corruption |

**Example**:
```rust
match wal.append(&record).await {
    Ok(pos) => println!("Success: {:?}", pos),
    Err(SegmentError::Io(e)) if e.kind() == io::ErrorKind::OutOfMemory => {
        panic!("Out of disk space!");
    }
    Err(e) => {
        log::error!("Append failed: {}", e);
        return Err(e);
    }
}
```

---

## Thread Safety

`Wal` is `Send + Sync`, so you can share it across threads:

```rust
use std::sync::Arc;

let wal = Arc::new(wal);

// Spawn multiple writers
for i in 0..4 {
    let wal = wal.clone();
    tokio::spawn(async move {
        let record = Record::put(format!("key{}", i).as_bytes(), b"value");
        wal.append(&record).await.unwrap();
    });
}

// Spawn a reader
let wal_reader = wal.clone();
tokio::spawn(async move {
    let mut reader = wal_reader.read_from(Position::start()).await.unwrap();
    while let Some((record, _)) = reader.next_record().await.unwrap() {
        println!("Read: {:?}", record.key);
    }
});
```

**Concurrency behavior**:
- Writes are serialized (only one thread writes at a time)
- Reads don't block writes (and vice versa)
- Multiple readers can run concurrently

See [Concurrency Model](../how-it-works/concurrency) for details.

---

## Performance Characteristics

| Operation | Latency | Throughput |
|-----------|---------|------------|
| `append()` (no fsync) | 10-50μs | ~110K ops/sec |
| `append()` (batch fsync) | 10-50μs (avg) | ~86K ops/sec |
| `append()` (always fsync) | 1-5ms | ~420 ops/sec |
| `append_batch(100)` (always fsync) | ~5ms total | ~20K records/sec |
| `sync()` | 1-5ms | - |
| `read_from() + iterate` | ~5μs per record | ~200K records/sec |

See [Performance Tuning](../performance/tuning) for optimization tips.

---

## See Also

- [WalConfig](config) - Configuration options
- [Record](record) - Record types
- [Position](position) - Position in the log
- [RecoveryInfo](recovery-info) - Recovery statistics
- [Errors](errors) - Error types
