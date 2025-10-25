---
layout: default
title: API Reference
nav_order: 5
has_children: true
---

# API Reference
{: .no_toc }

Complete API documentation for all public types and methods in nori-wal.
{: .fs-6 .fw-300 }

---

## Quick Reference

### Core Types

| Type | Purpose | Page |
|------|---------|------|
| [`Wal`](wal) | Main WAL interface | [API →](wal) |
| [`Record`](record) | Key-value record with metadata | [API →](record) |
| [`WalConfig`](config) | Configuration builder | [API →](config) |
| [`Position`](position) | Location in the log | [API →](position) |
| [`RecoveryInfo`](recovery-info) | Recovery statistics | [API →](recovery-info) |

### Enums

| Type | Purpose | Page |
|------|---------|------|
| [`FsyncPolicy`](fsync-policy) | Durability policy | [API →](fsync-policy) |
| [`Compression`](compression) | Compression algorithm | [API →](compression) |

### Error Types

| Type | Purpose | Page |
|------|---------|------|
| [`SegmentError`](errors#segmenterror) | Segment-level errors | [API →](errors#segmenterror) |
| [`RecordError`](errors#recorderror) | Record-level errors | [API →](errors#recorderror) |

---

## Import Paths

```rust
// Main types
use nori_wal::{Wal, WalConfig, Record, Position, RecoveryInfo};

// Enums
use nori_wal::{FsyncPolicy, Compression};

// Errors
use nori_wal::{SegmentError, RecordError};
```

---

## Usage Patterns

### Basic Write

```rust
use nori_wal::{Wal, WalConfig, Record};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = WalConfig::default();
    let (wal, _) = Wal::open(config).await?;

    let record = Record::put(b"key", b"value");
    let position = wal.append(&record).await?;
    wal.sync().await?;

    Ok(())
}
```

### Configuration

```rust
use nori_wal::{WalConfig, FsyncPolicy};
use std::time::Duration;
use std::path::PathBuf;

let config = WalConfig {
    dir: PathBuf::from("/var/lib/myapp/wal"),
    max_segment_size: 256 * 1024 * 1024,  // 256 MB
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
    preallocate: true,
    node_id: 1,
};

let (wal, recovery_info) = Wal::open(config).await?;
```

### Reading

```rust
use nori_wal::Position;

let mut reader = wal.read_from(Position { segment_id: 0, offset: 0 }).await?;

while let Some((record, position)) = reader.next_record().await? {
    println!("Record at {:?}: key={:?}", position, record.key);
}
```

### Error Handling

```rust
use nori_wal::{Wal, SegmentError};

match Wal::open(config).await {
    Ok((wal, info)) => {
        if info.corruption_detected {
            log::warn!("Corruption detected: {} bytes truncated",
                       info.bytes_truncated);
        }
    }
    Err(SegmentError::Io(e)) => {
        eprintln!("I/O error: {}", e);
    }
    Err(SegmentError::InvalidConfig(msg)) => {
        eprintln!("Invalid config: {}", msg);
    }
    Err(e) => {
        eprintln!("Other error: {}", e);
    }
}
```

---

## API Conventions

### Async Methods

All I/O methods are async and require a Tokio runtime:

```rust
// ✅ Correct: Use with async runtime
#[tokio::main]
async fn main() {
    wal.append(&record).await?;
}

// ❌ Wrong: Can't call async methods in sync context
fn main() {
    wal.append(&record).await?;  // Compile error!
}
```

### Error Handling

All fallible operations return `Result<T, E>`:

```rust
// Always handle errors
let position = wal.append(&record).await?;  // Propagate with ?

// Or match explicitly
match wal.append(&record).await {
    Ok(pos) => println!("Wrote at {:?}", pos),
    Err(e) => eprintln!("Failed: {}", e),
}
```

### Thread Safety

All types are `Send + Sync` and can be shared:

```rust
let wal = Arc::new(wal);

for _ in 0..4 {
    let wal = wal.clone();
    tokio::spawn(async move {
        wal.append(&record).await?;
    });
}
```

---

## Type Categories

### Primary API

Start here for most use cases:

- [`Wal`](wal) - Open, append, sync, read
- [`Record`](record) - Create PUT/DELETE records
- [`WalConfig`](config) - Configure behavior

### Advanced API

For fine-grained control:

- [`Position`](position) - Seek to specific locations
- [`SegmentReader`](segment-reader) - Manual reading
- [`FsyncPolicy`](fsync-policy) - Custom durability

### Observability

For monitoring and debugging:

- [`RecoveryInfo`](recovery-info) - Recovery statistics
- [`Meter` trait](meter) - Custom metrics (advanced)

---

## Examples by Use Case

### Event Sourcing

```rust
// Append events
let event = serde_json::to_vec(&MyEvent { id: 1, data: "..." })?;
let record = Record::put(b"aggregate:1", event);
wal.append(&record).await?;

// Replay events
let mut reader = wal.read_from(Position::start()).await?;
while let Some((record, _)) = reader.next_record().await? {
    let event: MyEvent = serde_json::from_slice(&record.value)?;
    apply_event(event);
}
```

### Key-Value Store

```rust
// PUT
let record = Record::put(b"user:123", b"alice@example.com");
wal.append(&record).await?;

// DELETE
let record = Record::delete(b"user:123");
wal.append(&record).await?;

// Rebuild state from log
let mut kv = HashMap::new();
let mut reader = wal.read_from(Position::start()).await?;
while let Some((record, _)) = reader.next_record().await? {
    if record.tombstone {
        kv.remove(&record.key);
    } else {
        kv.insert(record.key, record.value);
    }
}
```

### Message Queue

```rust
// Publish
let record = Record::put(b"topic:events", b"message payload");
let position = wal.append(&record).await?;

// Consumer tracks position
let mut consumer_position = load_consumer_position()?;
let mut reader = wal.read_from(consumer_position).await?;

while let Some((record, position)) = reader.next_record().await? {
    process_message(&record);
    consumer_position = position;
    save_consumer_position(position)?;
}
```

---

## Performance Tips

### Batching

```rust
// Batch multiple records before syncing
for record in records {
    wal.append(&record).await?;
}
wal.sync().await?;  // Single fsync for all
```

### Compression

```rust
// For large values, use compression
let record = Record::put(b"key", large_value)
    .with_compression(Compression::Lz4);
wal.append(&record).await?;
```

### Fsync Policy

```rust
// Choose policy based on durability needs
WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),  // Balanced
    // or FsyncPolicy::Always  // Maximum durability
    // or FsyncPolicy::Os      // Maximum performance
    ..Default::default()
}
```

---

## Detailed API Pages

Click through to detailed documentation for each type:

- **[Wal](wal)** - Main WAL API
- **[Record](record)** - Record creation and metadata
- **[WalConfig](config)** - Configuration options
- **[Position](position)** - Position in the log
- **[RecoveryInfo](recovery-info)** - Recovery results
- **[FsyncPolicy](fsync-policy)** - Durability policies
- **[Compression](compression)** - Compression algorithms
- **[Errors](errors)** - Error types and handling
