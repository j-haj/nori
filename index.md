---
layout: default
title: Home
nav_order: 1
description: "NoriKV is a sharded, Raft-replicated, log-structured key-value store with portable SDKs and first-class observability."
permalink: /
---

# NoriKV
{: .fs-9 }

A sharded, Raft-replicated, log-structured key-value store with portable SDKs and first-class observability.
{: .fs-6 .fw-300 }

[Get Started](getting-started/quickstart){: .btn .btn-primary .fs-5 .mb-4 .mb-md-0 .mr-2 }
[View on GitHub](https://github.com/j-haj/nori){: .btn .fs-5 .mb-4 .mb-md-0 }

---

## What is NoriKV?

**NoriKV** is a distributed key-value store built from composable, production-ready components. It combines battle-tested algorithms (Raft, SWIM, LSM) with modern observability and a clean architecture.

### Key Features

- **Log-Structured Storage**: LSM engine with WAL, SSTables, and automatic compaction
- **Raft Consensus**: Replicated logs with read-index optimization and lease-based reads
- **Automatic Sharding**: Jump Consistent Hash with configurable shards and replica placement
- **SWIM Membership**: Gossip-based failure detection and cluster discovery
- **First-Class Observability**: Vendor-neutral telemetry with Prometheus and OTLP exporters
- **Portable SDKs**: TypeScript, Python, Go, and Java clients with consistent APIs
- **100% Rust**: Safe, fast, and designed for production

---

## Architecture Overview

NoriKV is built from six core crates, each solving a specific problem:

```
┌─────────────────────────────────────────────────┐
│  NoriKV Server (DI composition)                 │
├─────────────────────────────────────────────────┤
│  Adapters: LSM, Raft, SWIM, gRPC, HTTP         │
├─────────────────────────────────────────────────┤
│  Ports: Storage, ReplicatedLog, Membership,    │
│         Transport, Router traits                │
├─────────────────────────────────────────────────┤
│  Domain: Types, IDs, Versions, Errors          │
└─────────────────────────────────────────────────┘
```

### Published Crates

| Crate | Purpose | Status |
|-------|---------|--------|
| **[nori-observe](https://github.com/j-haj/nori/tree/main/crates/nori-observe)** | Vendor-neutral observability ABI | Ready |
| **[nori-wal](https://github.com/j-haj/nori/tree/main/crates/nori-wal)** | Write-ahead log with recovery | Ready |
| **[nori-sstable](https://github.com/j-haj/nori/tree/main/crates/nori-sstable)** | Immutable sorted string tables | Ready |
| **[nori-lsm](https://github.com/j-haj/nori/tree/main/crates/nori-lsm)** | LSM storage engine | Ready |
| **[nori-swim](https://github.com/j-haj/nori/tree/main/crates/nori-swim)** | SWIM membership protocol | Ready |
| **[nori-raft](https://github.com/j-haj/nori/tree/main/crates/nori-raft)** | Raft consensus algorithm | Ready |

---

## Core Components

### nori-wal: Write-Ahead Log

Production-ready WAL with automatic recovery, rotation, and configurable durability.

```rust
use nori_wal::{Wal, WalConfig, Record};

let (wal, recovery_info) = Wal::open(WalConfig::default()).await?;
let record = Record::put(b"user:42", b"alice@example.com");
wal.append(&record).await?;
```

**Features:**
- 110K writes/sec with batch fsync
- CRC32C checksumming for corruption detection
- Automatic crash recovery with prefix-valid truncation
- LZ4/Zstd compression support

[WAL Documentation →](core-concepts/what-is-wal)

---

### nori-sstable: Sorted String Tables

Immutable, sorted key-value tables with bloom filters and compression.

**Features:**
- Block-based format with configurable block size
- Bloom filters for negative lookups
- Snappy/LZ4/Zstd compression
- Range queries and iterators

---

### nori-lsm: LSM Storage Engine

Embeddable LSM engine combining WAL, memtable, and SSTables.

**Features:**
- Leveled compaction strategy
- Automatic background compaction
- Point reads and range scans
- Snapshot isolation

---

### nori-raft: Raft Consensus

Production Raft implementation with modern optimizations.

**Features:**
- Leader election and log replication
- Read-index optimization for consistent reads
- Lease-based reads (linearizable without log appends)
- Joint consensus for configuration changes
- Snapshot support for log compaction

---

### nori-swim: SWIM Membership

Gossip-based failure detection and cluster membership.

**Features:**
- Scalable failure detection
- Eventual consistency for membership changes
- Configurable timeouts and failure detectors
- Integration with Raft for reconfiguration

---

### nori-observe: Observability ABI

Vendor-neutral observability layer with zero dependencies.

**Features:**
- `Meter` trait for metrics and events
- Prometheus/OpenMetrics exporter
- OTLP exporter with trace exemplars
- Typed `VizEvent` enums for dashboards
- Zero-allocation hot paths

---

## Use Cases

### Distributed Database

Use all components together for a full distributed KV store:

```
Client → gRPC → Router → Raft → LSM → WAL/SSTables
                            ↓
                          SWIM (membership)
```

### Embedded Storage

Use just the storage layer (LSM + WAL + SSTables):

```rust
use nori_lsm::{LsmEngine, LsmConfig};

let engine = LsmEngine::open(LsmConfig::default()).await?;
engine.put(b"key", b"value").await?;
let value = engine.get(b"key").await?;
```

### Custom Consensus

Use Raft with your own storage implementation:

```rust
use nori_raft::{Raft, RaftConfig, Storage};

struct MyStorage { /* ... */ }
impl Storage for MyStorage { /* ... */ }

let raft = Raft::new(RaftConfig::default(), MyStorage::new());
```

---

## Performance

{: .important }
> Benchmarks from Apple M2 Pro (10 cores, 16GB RAM). Production numbers will vary.

| Component | Operation | Performance |
|-----------|-----------|-------------|
| **nori-wal** | Sequential writes (batch fsync) | 110K/sec |
| **nori-wal** | Recovery | 3.3 GiB/s |
| **nori-lsm** | Point reads (memtable hit) | <1µs |
| **nori-lsm** | Point reads (SSTable L0) | ~10µs |
| **nori-sstable** | Sequential scan | 52 MiB/s |

[Detailed Benchmarks →](performance/benchmarks)

---

## SDKs

NoriKV provides official SDKs for multiple languages:

| Language | Package | Status |
|----------|---------|--------|
| TypeScript | `@norikv/client` | Ready |
| Python | `norikv` | Ready |
| Go | `github.com/j-haj/nori-go` | Ready |
| Java | `com.norikv:norikv-client` | Ready |

All SDKs share:
- Consistent API design
- Automatic retry and failover
- Connection pooling
- Type-safe key-value operations

---

## Architecture Highlights

### Hexagonal Architecture

NoriKV uses ports & adapters for clean separation:

**Ports (traits):**
- `Storage` - Key-value operations
- `ReplicatedLog` - Consensus interface
- `Membership` - Cluster state
- `Transport` - Network communication

**Adapters (implementations):**
- LSM adapter for `Storage`
- Raft adapter for `ReplicatedLog`
- SWIM adapter for `Membership`
- gRPC/HTTP adapters for `Transport`

This design allows:
- Testing with mock implementations
- Swapping components (e.g., different storage engines)
- Clear dependency boundaries

---

### Observability-First Design

Every component emits typed events via `nori-observe`:

```rust
pub trait Meter: Send + Sync {
    fn emit(&self, event: VizEvent);
}

pub enum VizEvent {
    Wal(WalEvt),
    Lsm(LsmEvt),
    Raft(RaftEvt),
    Swim(SwimEvt),
}
```

These events power:
- Prometheus metrics
- Live dashboards (via WebSocket)
- Distributed tracing (OTLP with exemplars)
- Debug logs

---

### Consistent Hashing & Placement

NoriKV uses Jump Consistent Hash for deterministic shard assignment:

```rust
fn key_to_shard(key: &[u8], num_shards: u32) -> u32 {
    let hash = xxhash64(key, seed: 0);
    jump_consistent_hash(hash, num_shards)
}
```

**Benefits:**
- Deterministic (same key → same shard)
- Minimal movement on resize (only K/N keys move)
- No routing table needed
- Lock-free lookups

Default: 1024 virtual shards, RF=3

---

## Documentation Structure

This documentation covers the entire NoriKV project:

### Core Concepts
Learn the fundamentals of WAL, LSM, Raft, and SWIM.

[Core Concepts →](core-concepts/what-is-wal)

### Getting Started
Quick tutorials to get up and running.

[Quickstart →](getting-started/quickstart)

### How It Works
Deep dives into internals and algorithms.

[How It Works →](how-it-works/record-format)

### API Reference
Complete API documentation for all crates.

[API Reference →](api-reference/)

### Recipes
Common patterns and use cases.

[Recipes →](recipes/)

### Performance
Benchmarks and optimization guides.

[Performance →](performance/benchmarks)

---

## Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
# Full stack
norikv-server = "0.1"

# Individual crates
nori-wal = "0.1"
nori-lsm = "0.1"
nori-raft = "0.1"
nori-swim = "0.1"
```

### Basic Example

```rust
use nori_lsm::{LsmEngine, LsmConfig};
use nori_wal::Record;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open LSM engine (includes WAL and SSTables)
    let config = LsmConfig::default();
    let engine = LsmEngine::open(config).await?;

    // Write data
    engine.put(b"user:123", b"alice@example.com").await?;

    // Read data
    if let Some(value) = engine.get(b"user:123").await? {
        println!("Value: {:?}", value);
    }

    // Range scan
    let range = engine.scan(b"user:", b"user:~").await?;
    for (key, value) in range {
        println!("{:?} → {:?}", key, value);
    }

    Ok(())
}
```

---

## When to Use NoriKV

### Great Fit

- Need a **distributed key-value store** with strong consistency
- Building **multi-tenant systems** with sharding
- Want **embeddable storage** components (use crates individually)
- Need **observability** out of the box
- Building in **Rust** and want safe, fast libraries
- Care about **operational simplicity** (no complex configuration)

### Not the Right Tool

- Need **SQL** or complex queries (use PostgreSQL, MySQL)
- Ultra-low latency **< 10µs** required (use in-memory stores)
- **Read-heavy** workloads with no writes (use caching layer)
- Need **document storage** with flexible schemas (use MongoDB)

---

## Project Status

NoriKV is under active development. Current status:

| Component | Status |
|-----------|--------|
| nori-wal | Production-ready |
| nori-sstable | Production-ready |
| nori-lsm | Production-ready |
| nori-raft | In development |
| nori-swim | In development |
| Server | In development |
| SDKs | Planned |

---

## Contributing

NoriKV is open source (MIT license) and welcomes contributions!

- **Found a bug?** [Open an issue](https://github.com/j-haj/nori/issues)
- **Have an idea?** [Start a discussion](https://github.com/j-haj/nori/discussions)
- **Want to contribute?** Check our [Contributing Guide](https://github.com/j-haj/nori/blob/main/CONTRIBUTING.md)

---

## License

MIT License - see [LICENSE](https://github.com/j-haj/nori/blob/main/LICENSE) for details.

---

## Next Steps

<div class="code-example" markdown="1">

**New to distributed systems?**
Start with [What is a Write-Ahead Log?](core-concepts/what-is-wal) to understand the fundamentals.

**Ready to build?**
Jump into the [5-Minute Quickstart](getting-started/quickstart) to get hands-on.

**Want deep dives?**
Check out [How It Works](how-it-works/record-format) for implementation details.

**Need API docs?**
See the [API Reference](api-reference/) for complete API documentation.

</div>
