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
┌─────────────────────────────────────────┐
│  Wal API                                 │
│  - open / append / sync / read           │
├─────────────────────────────────────────┤
│  SegmentManager                          │
│  - Rotation at 128MB                     │
│  - Fsync policy enforcement              │
├─────────────────────────────────────────┤
│  Recovery                                │
│  - CRC validation                        │
│  - Prefix-valid truncation               │
├─────────────────────────────────────────┤
│  Record Format                           │
│  - Varint encoding (klen, vlen)          │
│  - Flags (tombstone, TTL, compression)   │
│  - CRC32C checksum                       │
└─────────────────────────────────────────┘
```

Segments are stored as sequential files:
```
wal/
  000000.wal  (128 MB)
  000001.wal  (128 MB)
  000002.wal  (active)
```

## Performance Targets

Per `context/30_storage.yaml`:

- p95 GET latency: 10ms
- p95 PUT latency: 20ms
- Write amplification: ≤ 12x (with LSM compaction)

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

MIT OR Apache-2.0
