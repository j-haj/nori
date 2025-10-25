---
layout: default
title: Configuration
parent: API Reference
nav_order: 3
---

# Configuration API
{: .no_toc }

Complete API reference for configuring nori-wal behavior.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Overview

nori-wal provides two main configuration types:

- **`WalConfig`** - High-level WAL configuration
- **`FsyncPolicy`** - Durability vs performance trade-offs

These configurations allow you to tune nori-wal for your specific use case, balancing durability, performance, and resource usage.

---

## WalConfig

Configuration for the Write-Ahead Log.

### Type Definition

[View source in `crates/nori-wal/src/wal.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/wal.rs#L14-L30)

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dir` | `PathBuf` | `"wal"` | Directory to store WAL segments |
| `max_segment_size` | `u64` | `128 * 1024 * 1024` (128 MiB) | Maximum size of a segment before rotation |
| `fsync_policy` | `FsyncPolicy` | `Batch(5ms)` | Fsync policy for durability |
| `preallocate` | `bool` | `true` | Enable file pre-allocation for new segments |
| `node_id` | `u32` | `0` | Node ID for observability events |

---

### WalConfig::default

Creates a default configuration with sensible defaults for most use cases.

```rust
impl Default for WalConfig
```

**Returns:** A `WalConfig` with:
- Directory: `"wal"`
- Segment size: 128 MiB
- Fsync policy: Batch with 5ms window
- Pre-allocation: enabled
- Node ID: 0

**Examples:**

```rust
use nori_wal::WalConfig;

// Use default configuration
let config = WalConfig::default();

assert_eq!(config.dir, PathBuf::from("wal"));
assert_eq!(config.max_segment_size, 128 * 1024 * 1024);
assert_eq!(config.preallocate, true);
```

---

### Configuration Validation

`WalConfig` automatically validates configuration when opening a WAL.

**Validation Rules:**

1. **`max_segment_size` must be > 0**
   - Error: `"max_segment_size must be greater than 0"`

2. **`max_segment_size` should be ≥ 1 MiB**
   - Error: `"max_segment_size should be at least 1MB for reasonable performance"`

3. **Batch fsync window must be < 1 second**
   - Error: `"fsync batch window should be less than 1 second to avoid excessive data loss risk"`

4. **Batch fsync window cannot be zero**
   - Error: `"fsync batch window cannot be zero - use FsyncPolicy::Always instead"`

**Examples:**

```rust
use nori_wal::{Wal, WalConfig, FsyncPolicy};
use std::time::Duration;

// Invalid: zero segment size
let config = WalConfig {
    max_segment_size: 0,
    ..Default::default()
};
assert!(Wal::open(config).await.is_err());

// Invalid: segment too small
let config = WalConfig {
    max_segment_size: 512, // Less than 1MB
    ..Default::default()
};
assert!(Wal::open(config).await.is_err());

// Invalid: batch window too large
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_secs(2)),
    ..Default::default()
};
assert!(Wal::open(config).await.is_err());

// Invalid: zero batch window
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::ZERO),
    ..Default::default()
};
assert!(Wal::open(config).await.is_err());

// Valid configurations
let config = WalConfig {
    max_segment_size: 64 * 1024 * 1024, // 64 MiB
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(10)),
    ..Default::default()
};
let (wal, _) = Wal::open(config).await?;
```

---

### Field Details

#### `dir: PathBuf`

Directory where WAL segment files are stored.

**Default:** `"wal"`

**Behavior:**
- Created automatically if it doesn't exist (via `tokio::fs::create_dir_all`)
- Segments are named `000000.wal`, `000001.wal`, etc.
- Must be writable by the process

**Examples:**

```rust
use std::path::PathBuf;

// Use default directory
let config = WalConfig::default();
assert_eq!(config.dir, PathBuf::from("wal"));

// Custom directory
let config = WalConfig {
    dir: PathBuf::from("/var/lib/myapp/wal"),
    ..Default::default()
};

// Relative path
let config = WalConfig {
    dir: PathBuf::from("data/wal"),
    ..Default::default()
};

// Temporary directory for testing
let temp_dir = tempfile::TempDir::new()?;
let config = WalConfig {
    dir: temp_dir.path().to_path_buf(),
    ..Default::default()
};
```

---

#### `max_segment_size: u64`

Maximum size (in bytes) of a single segment file before rotation.

**Default:** `134,217,728` (128 MiB)

**Valid range:** `≥ 1,048,576` (1 MiB)

**Behavior:**
- When a segment reaches this size, a new segment is created
- Smaller values → more segment files → more overhead
- Larger values → fewer rotations → larger recovery time

**Choosing a Value:**

| Segment Size | Pros | Cons | Best For |
|--------------|------|------|----------|
| 16-32 MiB | Fast recovery, easier GC | More files, more overhead | Low-latency apps |
| 64-128 MiB | Balanced | Default choice | Most applications |
| 256-512 MiB | Fewer files | Slower recovery, more memory | High-throughput apps |
| 1 GiB+ | Minimal overhead | Very slow recovery | Archival/batch systems |

**Examples:**

```rust
// Small segments for fast recovery
let config = WalConfig {
    max_segment_size: 16 * 1024 * 1024, // 16 MiB
    ..Default::default()
};

// Default (balanced)
let config = WalConfig {
    max_segment_size: 128 * 1024 * 1024, // 128 MiB
    ..Default::default()
};

// Large segments for high throughput
let config = WalConfig {
    max_segment_size: 512 * 1024 * 1024, // 512 MiB
    ..Default::default()
};
```

**Performance Impact:**

```rust
// Recovery time increases with segment size
// For 100,000 records of ~100 bytes each (~10 MB data):

// 16 MiB segments: 1 segment to scan, ~10ms recovery
// 128 MiB segments: 1 segment to scan, ~10ms recovery
// 512 MiB segments: 1 segment to scan, ~10ms recovery

// For 10 million records (~1 GB data):

// 16 MiB segments: ~64 segments to scan, ~100ms recovery
// 128 MiB segments: ~8 segments to scan, ~80ms recovery
// 512 MiB segments: ~2 segments to scan, ~70ms recovery
```

---

#### `fsync_policy: FsyncPolicy`

Controls when data is synced to disk (durability vs performance trade-off).

**Default:** `FsyncPolicy::Batch(Duration::from_millis(5))`

**See:** [FsyncPolicy](#fsyncpolicy) for detailed documentation.

**Examples:**

```rust
use std::time::Duration;

// Maximum durability (every write synced)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Always,
    ..Default::default()
};

// Balanced (batch within 5ms)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),
    ..Default::default()
};

// Maximum performance (OS handles syncing)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Os,
    ..Default::default()
};
```

---

#### `preallocate: bool`

Enable file pre-allocation for new segment files.

**Default:** `true`

**Behavior:**
- When `true`, new segments are pre-allocated to `max_segment_size` using platform-specific APIs
  - Linux: `fallocate()`
  - macOS: `fcntl(F_PREALLOCATE)`
- When `false`, segments grow as data is written

**Benefits of Pre-allocation:**

**Early error detection** - "No space left on device" errors happen at segment creation, not during critical writes

**Better filesystem locality** - Reduces fragmentation, keeps data contiguous

**Improved performance** - Some filesystems (ext4, XFS) perform better with pre-allocated files

**Predictable space usage** - Easier to monitor disk usage

**Drawbacks:**

**Disk space** - Reserves full segment size immediately (even if not used)

**Slower startup** - Pre-allocation takes time (usually <100ms per segment)

**When to Disable:**

- Running on filesystems that don't support pre-allocation
- Very tight disk space constraints
- Want to minimize initial disk usage

**Examples:**

```rust
// Enable pre-allocation (default)
let config = WalConfig {
    preallocate: true,
    max_segment_size: 128 * 1024 * 1024,
    ..Default::default()
};
// → Immediately reserves 128 MiB on disk

// Disable pre-allocation
let config = WalConfig {
    preallocate: false,
    max_segment_size: 128 * 1024 * 1024,
    ..Default::default()
};
// → Only uses disk space as data is written
```

**Disk Usage Comparison:**

```rust
// With preallocate = true:
// 3 segments created → 3 × 128 MiB = 384 MiB disk space used
// (even if segments only contain 10 MiB of actual data)

// With preallocate = false:
// 3 segments with 10 MiB data each → 30 MiB disk space used
```

---

#### `node_id: u32`

Identifier for this WAL instance, used in observability events.

**Default:** `0`

**Purpose:**
- Distinguishes WAL instances in distributed systems
- Included in `VizEvent` metrics for dashboards
- Useful for debugging multi-node deployments

**Examples:**

```rust
// Single-node deployment (default)
let config = WalConfig {
    node_id: 0,
    ..Default::default()
};

// Multi-node deployment
let config_node1 = WalConfig {
    node_id: 1,
    ..Default::default()
};

let config_node2 = WalConfig {
    node_id: 2,
    ..Default::default()
};

// Use server ID from environment
let node_id = std::env::var("SERVER_ID")?.parse()?;
let config = WalConfig {
    node_id,
    ..Default::default()
};
```

---

## FsyncPolicy

Fsync policy controlling durability vs performance trade-offs.

### Type Definition

[View source in `crates/nori-wal/src/segment.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/segment.rs#L39-L49)

### Variants

#### `FsyncPolicy::Always`

**Fsync after every write** - Maximum durability, lowest performance.

**Guarantees:**
- Every `append()` call syncs to disk before returning
- Zero data loss on crash (except in-flight operations)
- Writes are durable immediately

**Performance:**
- ~5,000-10,000 writes/sec on SSD
- ~100-500 writes/sec on HDD
- Each write waits for disk fsync (~0.1-1ms on SSD)

**Use When:**
- Absolutely cannot lose any committed data
- Financial transactions, audit logs
- Low write volume (<10k writes/sec)

**Examples:**

```rust
let config = WalConfig {
    fsync_policy: FsyncPolicy::Always,
    ..Default::default()
};

let (wal, _) = Wal::open(config).await?;

// Every append is immediately durable
let record = Record::put(b"account:123", b"balance:1000");
wal.append(&record).await?; // Blocks until synced to disk
// Data is now durable, even if power fails right after
```

**Benchmark:**

```
FsyncPolicy::Always on NVMe SSD:
  - Throughput: ~8,000 writes/sec
  - Latency p50: 0.1ms
  - Latency p99: 0.3ms
```

---

#### `FsyncPolicy::Batch(Duration)`

**Batch fsyncs within a time window** - Balanced durability and performance.

**Guarantees:**
- Fsyncs happen at most once per `Duration`
- Writes are buffered and synced together
- May lose up to `Duration` of data on crash

**Parameters:**
- `Duration`: Maximum time between fsyncs
- Valid range: `1ms` to `999ms` (enforced by validation)

**Performance:**
- ~50,000-200,000 writes/sec (depending on window size)
- Amortizes fsync cost across many writes
- Throughput scales with batch window

**Use When:**
- Can tolerate small data loss window (milliseconds)
- High write volume (>10k writes/sec)
- Most database and cache workloads

**Examples:**

```rust
use std::time::Duration;

// Aggressive batching (default: 5ms window)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),
    ..Default::default()
};

// Conservative batching (1ms window)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(1)),
    ..Default::default()
};

// Relaxed batching (100ms window)
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(100)),
    ..Default::default()
};

let (wal, _) = Wal::open(config).await?;

// Writes are buffered
wal.append(&record1).await?; // Returns immediately (buffered)
wal.append(&record2).await?; // Returns immediately (buffered)
wal.append(&record3).await?; // Returns immediately (buffered)

// After 5ms (or manual sync), all 3 records are synced together
tokio::time::sleep(Duration::from_millis(6)).await;
// All 3 records now durable

// Or manually sync before the window expires
wal.append(&record4).await?;
wal.sync().await?; // Force immediate fsync
// record4 is now durable
```

**Choosing a Window Size:**

| Window | Data Loss Risk | Throughput | Best For |
|--------|----------------|------------|----------|
| 1ms | Minimal (~1ms of data) | ~50k writes/sec | Financial systems |
| 5ms | Low (~5ms of data) | ~110k writes/sec | Databases (default) |
| 10ms | Moderate (~10ms of data) | ~150k writes/sec | High-throughput caches |
| 100ms | High (~100ms of data) | ~200k writes/sec | Analytics, logs |

**Benchmark:**

```
FsyncPolicy::Batch on NVMe SSD:
  Window     Throughput      p50 Latency    p99 Latency
  1ms        55,000/sec      0.02ms         1.2ms
  5ms        110,000/sec     0.01ms         5.5ms
  10ms       145,000/sec     0.01ms         10.5ms
  100ms      190,000/sec     0.01ms         102ms
```

---

#### `FsyncPolicy::Os`

**Let the OS handle fsyncing** - Best performance, least durability.

**Guarantees:**
- Writes are buffered in OS page cache
- No explicit fsync calls (except on `sync()` or `close()`)
- May lose data on crash or power failure
- Data is eventually written to disk (OS dependent)

**Performance:**
- ~500,000-1,000,000 writes/sec
- Submicrosecond latency
- Limited only by memcpy speed

**Use When:**
- Durability is not critical
- Can reconstruct data from other sources
- Ephemeral caches, development/testing
- Replication from another durable source

**Examples:**

```rust
let config = WalConfig {
    fsync_policy: FsyncPolicy::Os,
    ..Default::default()
};

let (wal, _) = Wal::open(config).await?;

// Writes are buffered in OS page cache (very fast)
for i in 0..1_000_000 {
    let record = Record::put(format!("key{}", i).as_bytes(), b"value");
    wal.append(&record).await?; // Returns in microseconds
}

// Manually sync when you want durability
wal.sync().await?; // All 1M records now durable

// Or close WAL (automatically syncs)
wal.close().await?;
```

**Risk:**

```rust
let config = WalConfig {
    fsync_policy: FsyncPolicy::Os,
    ..Default::default()
};
let (wal, _) = Wal::open(config).await?;

wal.append(&record1).await?;
wal.append(&record2).await?;
wal.append(&record3).await?;

// Power failure here → All 3 records likely lost
// (unless OS flushed the page cache)

// To guarantee durability with FsyncPolicy::Os:
wal.append(&record4).await?;
wal.sync().await?; // Explicit sync
// Now record4 is durable
```

**Benchmark:**

```
FsyncPolicy::Os on NVMe SSD:
  - Throughput: ~750,000 writes/sec
  - Latency p50: 0.001ms (1 microsecond)
  - Latency p99: 0.003ms
```

---

### Fsync Policy Comparison

| Policy | Throughput | Latency p99 | Data Loss Risk | Use Case |
|--------|------------|-------------|----------------|----------|
| `Always` | ~8k/sec | 0.3ms | None | Financial, audit logs |
| `Batch(1ms)` | ~55k/sec | 1.2ms | ~1ms of data | Conservative DBs |
| `Batch(5ms)` | ~110k/sec | 5.5ms | ~5ms of data | **Default** - most apps |
| `Batch(10ms)` | ~145k/sec | 10.5ms | ~10ms of data | High-throughput DBs |
| `Batch(100ms)` | ~190k/sec | 102ms | ~100ms of data | Analytics, logs |
| `Os` | ~750k/sec | 0.003ms | High (until manual sync) | Caches, testing |

**Trade-off Visualization:**

```
Durability ←─────────────────────────────────────────→ Performance

Always    Batch(1ms)  Batch(5ms)  Batch(100ms)    Os
  |          |           |             |            |
  ↓          ↓           ↓             ↓            ↓
8k/sec   55k/sec    110k/sec      190k/sec     750k/sec
```

---

## Usage Patterns

### Production Database

```rust
use nori_wal::{WalConfig, FsyncPolicy};
use std::time::Duration;

let config = WalConfig {
    dir: PathBuf::from("/var/lib/mydb/wal"),
    max_segment_size: 128 * 1024 * 1024, // 128 MiB
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),
    preallocate: true,
    node_id: std::env::var("NODE_ID")?.parse()?,
};

let (wal, recovery_info) = Wal::open(config).await?;
log::info!("Recovered {} records", recovery_info.valid_records);
```

---

### High-Durability System (Financial)

```rust
let config = WalConfig {
    dir: PathBuf::from("/mnt/raid1/wal"),
    max_segment_size: 64 * 1024 * 1024, // Smaller segments for faster recovery
    fsync_policy: FsyncPolicy::Always, // Never lose data
    preallocate: true,
    node_id: 1,
};

let (wal, _) = Wal::open(config).await?;

// Every write is immediately durable
wal.append(&Record::put(b"txn:123", b"transfer:$1000")).await?;
// Guaranteed on disk before returning
```

---

### High-Throughput Cache

```rust
let config = WalConfig {
    dir: PathBuf::from("cache/wal"),
    max_segment_size: 256 * 1024 * 1024, // Larger segments
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(100)), // Relaxed
    preallocate: false, // Save disk space
    node_id: 0,
};

let (wal, _) = Wal::open(config).await?;

// High throughput, can tolerate 100ms data loss
for i in 0..1_000_000 {
    wal.append(&Record::put(format!("cache:{}", i).as_bytes(), b"data")).await?;
}
```

---

### Testing / Development

```rust
use tempfile::TempDir;

let temp_dir = TempDir::new()?;
let config = WalConfig {
    dir: temp_dir.path().to_path_buf(),
    max_segment_size: 16 * 1024 * 1024, // Small segments
    fsync_policy: FsyncPolicy::Os, // Fast, no durability needed
    preallocate: false,
    node_id: 0,
};

let (wal, _) = Wal::open(config).await?;

// Fast writes for testing
wal.append(&Record::put(b"test", b"data")).await?;
```

---

### Multi-Node Deployment

```rust
// Node 1
let config_node1 = WalConfig {
    dir: PathBuf::from("/data/node1/wal"),
    node_id: 1,
    ..Default::default()
};

// Node 2
let config_node2 = WalConfig {
    dir: PathBuf::from("/data/node2/wal"),
    node_id: 2,
    ..Default::default()
};

// Node 3
let config_node3 = WalConfig {
    dir: PathBuf::from("/data/node3/wal"),
    node_id: 3,
    ..Default::default()
};

// Observability events will include node_id to distinguish nodes
```

---

## Best Practices

### 1. Choose Appropriate Segment Size

```rust
// GOOD: Balance based on write volume
let config = if high_write_volume {
    WalConfig {
        max_segment_size: 256 * 1024 * 1024, // 256 MiB
        ..Default::default()
    }
} else {
    WalConfig {
        max_segment_size: 64 * 1024 * 1024, // 64 MiB
        ..Default::default()
    }
};

// BAD: Too small (excessive overhead)
let config = WalConfig {
    max_segment_size: 1024 * 1024, // 1 MiB - will rotate constantly
    ..Default::default()
};
```

### 2. Match Fsync Policy to Durability Needs

```rust
// GOOD: Match policy to use case
let cache_config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(50)),
    ..Default::default()
};

let txn_config = WalConfig {
    fsync_policy: FsyncPolicy::Always,
    ..Default::default()
};

// BAD: Using Os policy for critical data
let config = WalConfig {
    fsync_policy: FsyncPolicy::Os, // Can lose data!
    ..Default::default()
};
// Then storing financial transactions → WRONG
```

### 3. Use Pre-allocation on Production Systems

```rust
// GOOD: Enable pre-allocation for production
let config = WalConfig {
    preallocate: true, // Fail fast on disk space issues
    ..Default::default()
};

//  RISKY: Disable only if necessary
let config = WalConfig {
    preallocate: false, // Might fail mid-write!
    ..Default::default()
};
```

### 4. Set Meaningful Node IDs

```rust
// GOOD: Unique node IDs in distributed systems
let node_id = hostname::get()?.to_str().unwrap().parse()?;
let config = WalConfig {
    node_id,
    ..Default::default()
};

// BAD: All nodes use default node_id = 0
// (can't distinguish them in metrics)
```

---

## See Also

- [Wal API](wal) - Main WAL interface
- [Record API](record) - Record format
- [Errors](errors) - Error handling
- [Core Concepts: Fsync Policies](../core-concepts/fsync-policies) - Deep dive into durability trade-offs
- [Performance Benchmarks](../performance/benchmarks) - Performance comparison of fsync policies
