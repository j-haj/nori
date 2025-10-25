---
layout: default
title: Home
nav_order: 1
description: "nori-wal is a production-ready Write-Ahead Log implementation in Rust with automatic recovery, configurable durability, and excellent performance."
permalink: /
---

# nori-wal
{: .fs-9 }

Production-ready Write-Ahead Log for Rust with automatic recovery, rotation, and configurable durability guarantees.
{: .fs-6 .fw-300 }

[Get Started](getting-started/quickstart){: .btn .btn-primary .fs-5 .mb-4 .mb-md-0 .mr-2 }
[View on GitHub](https://github.com/j-haj/nori){: .btn .fs-5 .mb-4 .mb-md-0 }

---

## What is nori-wal?

**nori-wal** is an append-only write-ahead log (WAL) designed for building reliable, high-performance storage systems. Think of it as a durable, sequential log where you can write records and be confident they'll survive crashes, power failures, and other disasters.

### Key Features

- 🚀 **Fast**: 110K writes/sec with batch fsync, 3.3 GiB/s recovery speed
- 💪 **Reliable**: CRC32C checksumming, automatic crash recovery, prefix-valid truncation
- 🎯 **Configurable**: Choose your durability vs performance tradeoff
- 🔧 **Production-ready**: Battle-tested recovery, proper error handling, comprehensive observability
- 📦 **Zero dependencies** (except tokio): No complex dependency trees
- 🦀 **100% Safe Rust**: No unsafe code in the public API

---

## Quick Example

```rust
use nori_wal::{Wal, WalConfig, Record};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open a WAL (automatically recovers from previous sessions)
    let config = WalConfig::default();
    let (wal, recovery_info) = Wal::open(config).await?;

    println!("Recovered {} records from previous session",
        recovery_info.valid_records);

    // Append a record - it's crash-safe!
    let record = Record::put(b"user:42", b"alice@example.com");
    let position = wal.append(&record).await?;

    // Read it back
    let mut reader = wal.read_from(position).await?;
    if let Some((record, _pos)) = reader.next_record().await? {
        println!("Key: {:?}, Value: {:?}", record.key, record.value);
    }

    Ok(())
}
```

---

## Why Use a WAL?

A Write-Ahead Log is fundamental to building reliable storage systems. Here's why:

### Durability Without Complexity

Instead of worrying about atomically updating complex data structures on disk, you append to a log. If you crash mid-operation, the log tells you exactly what happened.

### Recovery Made Simple

After a crash, just scan the log from the beginning. Valid records are kept, partial writes at the tail are discarded. No complex transaction recovery logic needed.

### Foundation for Higher-Level Systems

WALs are the building blocks for:
- **Key-value stores** (like RocksDB, LevelDB)
- **Databases** (PostgreSQL, MySQL, etc.)
- **Message queues** (Kafka, Pulsar)
- **Event sourcing** systems
- **Replication** protocols

---

## Performance at a Glance

{: .important }
> These numbers are from an Apple M2 Pro with 10 cores and 16GB RAM. Your mileage may vary, but they show what's possible.

| Operation | Performance | Notes |
|-----------|-------------|-------|
| **Sequential Writes** | 110K writes/sec | 1KB records, OS fsync |
| **Batch Writes** | 102 MiB/s | 1000-record batches |
| **Recovery** | 3.3 GiB/s | Validates CRC on every record |
| **Sequential Reads** | 52 MiB/s | Full scan with decode |

Want details? Check out the [Performance Guide](performance/benchmarks).

---

## Choose Your Durability Level

nori-wal lets you pick the right tradeoff for your use case:

```rust
use nori_wal::FsyncPolicy;
use std::time::Duration;

// Maximum durability: fsync after every write
//   Good for: Financial data, critical user data
//   Performance: ~420 writes/sec
let policy = FsyncPolicy::Always;

// Balanced: batch fsyncs within 5ms window
//   Good for: Most applications
//   Performance: ~86K writes/sec
let policy = FsyncPolicy::Batch(Duration::from_millis(5));

// Maximum performance: let OS decide when to flush
//   Good for: Event logs, metrics, caches
//   Performance: ~110K writes/sec
let policy = FsyncPolicy::Os;
```

---

## What Makes nori-wal Different?

### Designed for Real-World Production

- **Automatic recovery** with detailed metrics
- **Segment rotation** at 128MB (configurable) prevents unbounded growth
- **Garbage collection** API for safe cleanup after compaction
- **File pre-allocation** (platform-specific) for better filesystem behavior
- **Observability hooks** for metrics and events

### Optimized for Modern Hardware

- **64KB read buffers** reduce syscall overhead
- **Batch append API** amortizes lock and fsync costs
- **LZ4/Zstd compression** for compressible data
- **Zero-copy reads** where possible
- **Concurrent readers** don't block writers

### Built on Solid Foundations

- **CRC32C checksums** catch corruption early
- **Varint encoding** for compact records
- **Prefix-valid recovery** strategy (industry standard)
- **Atomic truncation** using temp file + rename
- **Platform-specific optimizations** (fallocate on Linux, F_PREALLOCATE on macOS)

---

## When to Use nori-wal

### ✅ Great Fit

- Building a **database** or **key-value store**
- Implementing **event sourcing** or **CQRS**
- Creating a **message queue** or **pub-sub system**
- Need **crash-safe** append-only storage
- Want **simple recovery** after failures
- Building **replication** systems

### ❌ Not the Right Tool

- Need **random access** (use an index on top)
- Ultra-low latency (< 10µs) required
- **Read-heavy** workloads (WALs are write-optimized)
- Don't need durability (use in-memory structures)

---

## Next Steps

<div class="code-example" markdown="1">

**New to WALs?**
Start with [What is a Write-Ahead Log?](core-concepts/what-is-wal) to understand the fundamentals.

**Ready to build?**
Jump into the [5-Minute Quickstart](getting-started/quickstart) to get hands-on.

**Want to understand the internals?**
Check out [How It Works](how-it-works/record-format) for deep dives.

**Need API docs?**
See the [API Reference](api-reference/) for complete details.

</div>

---

## Contributing

nori-wal is open source (MIT license) and welcomes contributions! Found a bug? Have an idea? Check out our [Contributing Guide](https://github.com/j-haj/nori/blob/main/CONTRIBUTING.md).

---

## License

MIT License - see [LICENSE](https://github.com/j-haj/nori/blob/main/LICENSE) for details.
