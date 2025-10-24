# nori-wal

Append-only write-ahead log with automatic recovery, rotation, and configurable durability guarantees.

## Features

- **Varint-encoded records** with CRC32C checksumming for data integrity
- **Automatic segment rotation** at 128MB (configurable)
- **Crash recovery** with prefix-valid strategy and partial-tail truncation
- **Configurable fsync policies**: Always, Batch (time-windowed), or OS-managed
- **Multi-segment support** with concurrent readers
- **First-class observability** via `nori-observe` (vendor-neutral metrics and events)
- **Zero-copy reads** where possible

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
nori-wal = "0.1"
```

### Basic Usage

```rust
use nori_wal::{Wal, WalConfig, Record};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open WAL (performs recovery automatically)
    let config = WalConfig::default();
    let (wal, recovery_info) = Wal::open(config).await?;

    println!("Recovered {} records", recovery_info.valid_records);

    // Append records
    let record = Record::put(b"key", b"value");
    let position = wal.append(&record).await?;

    // Delete a key (tombstone)
    let delete = Record::delete(b"old_key");
    wal.append(&delete).await?;

    // Sync to disk
    wal.sync().await?;

    Ok(())
}
```

### Custom Configuration

```rust
use nori_wal::{WalConfig, FsyncPolicy};
use std::time::Duration;
use std::path::PathBuf;

let config = WalConfig {
    dir: PathBuf::from("/var/lib/myapp/wal"),
    max_segment_size: 256 * 1024 * 1024, // 256 MB
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
    node_id: 42,
};

let (wal, _info) = Wal::open(config).await?;
```

### Reading Records

```rust
use nori_wal::Position;

// Read from beginning
let mut reader = wal.read_from(Position { segment_id: 0, offset: 0 }).await?;

while let Some((record, position)) = reader.next_record().await? {
    println!("Record at {:?}: key={:?}", position, record.key);
    if record.tombstone {
        println!("  -> Tombstone (deleted)");
    } else {
        println!("  -> Value: {:?}", record.value);
    }
}
```

### With Observability

```rust
use nori_wal::Wal;
use nori_observe_prom::PrometheusMeter; // Example backend
use std::sync::Arc;

let meter = Arc::new(PrometheusMeter::new());
let config = WalConfig::default();

let (wal, _info) = Wal::open_with_meter(config, meter).await?;

// Now all WAL operations emit metrics:
// - wal_records_total
// - wal_fsync_ms
// - segment rotations
// - corruption events
```

## Fsync Policies

Choose your durability vs. performance tradeoff:

| Policy | Durability | Performance | Use Case |
|--------|-----------|-------------|----------|
| `Always` | Maximum | Lowest | Critical data, infrequent writes |
| `Batch(window)` | High | Balanced | Default (5ms window) |
| `Os` | OS-dependent | Highest | High-throughput, acceptable data loss |

```rust
// Maximum durability - fsync after every write
FsyncPolicy::Always

// Balanced - fsync at most once per 5ms
FsyncPolicy::Batch(Duration::from_millis(5))

// Best performance - let OS decide when to fsync
FsyncPolicy::Os
```

## Recovery

The WAL automatically recovers on open:

- Scans all segment files sequentially
- Validates CRC32C for each record
- Truncates partial or corrupt records at tail
- Emits `CorruptionTruncated` events when corruption is detected

```rust
let (wal, recovery_info) = Wal::open(config).await?;

println!("Recovery stats:");
println!("  Valid records: {}", recovery_info.valid_records);
println!("  Segments scanned: {}", recovery_info.segments_scanned);
println!("  Bytes truncated: {}", recovery_info.bytes_truncated);
println!("  Corruption detected: {}", recovery_info.corruption_detected);
```

## Record Types

### PUT Records

```rust
// Simple PUT
let record = Record::put(b"key", b"value");

// PUT with TTL
use std::time::Duration;
let record = Record::put_with_ttl(
    b"session_key",
    b"session_data",
    Duration::from_secs(3600)
);

// PUT with compression (note: compression not yet implemented)
let record = Record::put(b"large_key", b"large_value")
    .with_compression(Compression::Lz4);
```

### DELETE Records (Tombstones)

```rust
let record = Record::delete(b"key_to_remove");
assert!(record.tombstone);
```

## Architecture

```
┌──────────────────────────────────────────┐
│  Wal API                                 │
│  - open / append / sync / read           │
├──────────────────────────────────────────┤
│  SegmentManager                          │
│  - Rotation at 128MB                     │
│  - Fsync policy enforcement              │
├──────────────────────────────────────────┤
│  Recovery                                │
│  - CRC validation                        │
│  - Prefix-valid truncation               │
├──────────────────────────────────────────┤
│  Record Format                           │
│  - Varint encoding (klen, vlen)          │
│  - Flags (tombstone, TTL, compression)   │
│  - CRC32C checksum                       │
└──────────────────────────────────────────┘
```

Segments are stored as sequential files:
```
wal/
  000000.wal  (128 MB)
  000001.wal  (128 MB)
  000002.wal  (active)
```

## Performance

**TL;DR - What performance can you expect?**

**Good fit for these scenarios:**

- **High-throughput event logging**: 110K writes/sec (1KB records) with OS fsync
- **Web application state**: 86K writes/sec with 5ms durability guarantee (Batch fsync)
- **Message queues & event streams**: 102 MiB/s when batching 1000+ messages
- **Fast crash recovery**: 50MB WAL recovers in 15ms, multi-GB in under 1 second
- **Sequential replay**: 52 MiB/s scan throughput for rebuilding state

**Not ideal for these scenarios:**

- **Ultra-low latency systems**: ~9µs per write may be too slow (consider lockless queues)
- **Strict synchronous writes**: Always fsync drops to ~420 writes/sec (disk-bound)
- **Random read access**: No indexing, only sequential scans (use LSM or B-tree on top)
- **Extremely high concurrency**: 8+ concurrent writers see lock contention (~15 MiB/s)
- **Very large records (>10KB)**: Throughput drops to ~44K writes/sec with batching

**Recommendation:** Use Batch (5ms) fsync policy for most applications—it provides 80% of OS performance with good durability guarantees.

---

*Benchmarks run on Apple M2 Pro (10 cores, 16GB RAM) using [Criterion](https://github.com/bheisler/criterion.rs).*

### Write Performance

**Single-threaded sequential writes** show how fast individual append operations are:

| Record Size | OS Fsync | Batch (5ms) Fsync | Always Fsync |
|------------|----------|-------------------|--------------|
| 100 bytes  | ~8.4µs/write (119,000 writes/sec) | ~9.9µs/write (101,000 writes/sec) | ~1.3ms/write (755 writes/sec) |
| 1 KB       | ~9.1µs/write (110,000 writes/sec) | ~11.6µs/write (86,000 writes/sec) | ~2.4ms/write (420 writes/sec) |
| 10 KB      | ~14.5µs/write (69,000 writes/sec) | ~22.5µs/write (44,000 writes/sec) | ~3.1ms/write (323 writes/sec) |

**Throughput:**
- OS fsync: 11-673 MiB/s depending on record size
- Batch fsync: 9-434 MiB/s (5ms batching window)
- Always fsync: 74 KiB/s - 3.2 MiB/s (syncs after every write)

**Batch writes** (appending multiple records then syncing once):

| Batch Size | Throughput | Time |
|-----------|-----------|------|
| 10 records | ~2.8 MiB/s | ~3.5ms |
| 100 records | ~22.5 MiB/s | ~4.3ms |
| 1000 records | ~102 MiB/s | ~9.5ms |

### Concurrent Write Performance

Multiple async tasks writing concurrently (100 writes per task, 1KB records, OS fsync):

| Threads | Throughput | Time |
|---------|-----------|------|
| 1       | ~19.2 MiB/s | ~5.1ms |
| 2       | ~19.8 MiB/s | ~9.9ms |
| 4       | ~14.7 MiB/s | ~26.7ms |
| 8       | ~17.5 MiB/s | ~44.6ms |

### Read Performance

Sequential scan throughput (reading all records from the beginning):

| Record Count | Time | Throughput |
|-------------|------|------------|
| 100 (100 KB) | ~1.9ms | ~52 MiB/s |
| 1,000 (1 MB) | ~18.6ms | ~52 MiB/s |
| 10,000 (10 MB) | ~182ms | ~54 MiB/s |

### Recovery Performance

Time to recover and validate records on WAL restart (simulates crash recovery):

| Record Count | Time | Throughput |
|-------------|------|------------|
| 1,000 (1 MB) | ~458µs | ~2.1 GiB/s |
| 10,000 (10 MB) | ~2.9ms | ~3.3 GiB/s |
| 50,000 (50 MB) | ~14.6ms | ~3.3 GiB/s |

**Multi-segment recovery** (spanning multiple 1MB segments):
- 2 segments (5,000 records): ~1.3ms (~3.6 GiB/s)
- 5 segments (5,000 records): ~1.5ms (~3.2 GiB/s)

## Observability Events

The WAL emits typed events via `nori-observe::Meter`:

- `WalEvt::SegmentRoll { bytes }` - Segment rotated
- `WalEvt::Fsync { ms }` - Fsync completed with timing
- `WalEvt::CorruptionTruncated` - Corruption detected and truncated

## Thread Safety

- `Wal` is `Send + Sync` and can be shared across threads
- Concurrent appends are serialized internally
- Multiple readers can operate concurrently

## License

MIT
