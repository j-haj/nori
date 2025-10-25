---
layout: default
title: 5-Minute Quickstart
parent: Getting Started
nav_order: 1
---

# 5-Minute Quickstart
{: .no_toc }

Get up and running with nori-wal in under 5 minutes.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Installation

Add nori-wal to your `Cargo.toml`:

```toml
[dependencies]
nori-wal = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "fs"] }
```

{: .note }
nori-wal requires tokio for async I/O. Make sure you have the `fs` feature enabled.

---

## Your First WAL

Let's write a complete example that demonstrates the key concepts. This example shows:
- Opening a WAL (with automatic recovery)
- Writing records
- Reading records back
- Handling crashes gracefully

Create a new file `src/main.rs`:

```rust
use nori_wal::{Wal, WalConfig, Record};
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Step 1: Open the WAL
    // This automatically recovers from any previous session
    let config = WalConfig::default();
    let (wal, recovery_info) = Wal::open(config).await?;

    // Tell us what happened during recovery
    println!("=== Recovery Stats ===");
    println!("  Valid records recovered: {}", recovery_info.valid_records);
    println!("  Segments scanned: {}", recovery_info.segments_scanned);
    println!("  Corruption detected: {}", recovery_info.corruption_detected);
    println!("  Bytes truncated: {}", recovery_info.bytes_truncated);

    // Step 2: Write some records
    println!("\n=== Writing Records ===");

    // PUT: Write a key-value pair
    let record = Record::put(b"user:1", b"alice@example.com");
    let pos1 = wal.append(&record).await?;
    println!("Wrote user:1 at position {:?}", pos1);

    let record = Record::put(b"user:2", b"bob@example.com");
    let pos2 = wal.append(&record).await?;
    println!("Wrote user:2 at position {:?}", pos2);

    // DELETE: Mark a key as deleted (tombstone)
    let record = Record::delete(b"user:1");
    let pos3 = wal.append(&record).await?;
    println!("Deleted user:1 at position {:?}", pos3);

    // Step 3: Ensure durability
    // sync() forces all data to disk
    wal.sync().await?;
    println!("\nAll records synced to disk!");

    // Step 4: Read records back
    println!("\n=== Reading Records ===");

    // Create a reader starting from the beginning
    let mut reader = wal.read_from(
        nori_wal::Position { segment_id: 0, offset: 0 }
    ).await?;

    // Scan through all records
    let mut count = 0;
    while let Some((record, position)) = reader.next_record().await? {
        count += 1;
        println!("Record #{} at {:?}:", count, position);
        println!("  Key: {}", String::from_utf8_lossy(&record.key));

        if record.tombstone {
            println!("  Type: DELETE (tombstone)");
        } else {
            println!("  Type: PUT");
            println!("  Value: {}", String::from_utf8_lossy(&record.value));
        }
    }

    println!("\nRead {} records total", count);

    Ok(())
}
```

---

## Run It!

```bash
cargo run
```

You should see output like this:

```
=== Recovery Stats ===
  Valid records recovered: 0
  Segments scanned: 0
  Corruption detected: false
  Bytes truncated: 0

=== Writing Records ===
Wrote user:1 at position Position { segment_id: 0, offset: 0 }
Wrote user:2 at position Position { segment_id: 0, offset: 27 }
Deleted user:1 at position Position { segment_id: 0, offset: 54 }

All records synced to disk!

=== Reading Records ===
Record #1 at Position { segment_id: 0, offset: 0 }:
  Key: user:1
  Type: PUT
  Value: alice@example.com

Record #2 at Position { segment_id: 0, offset: 27 }:
  Key: user:2
  Type: PUT
  Value: bob@example.com

Record #3 at Position { segment_id: 0, offset: 54 }:
  Key: user:1
  Type: DELETE (tombstone)

Read 3 records total
```

---

## Run It Again!

Now run the same program again:

```bash
cargo run
```

Notice something different?

```
=== Recovery Stats ===
  Valid records recovered: 3    <--- Now we recovered data!
  Segments scanned: 1
  Corruption detected: false
  Bytes truncated: 0

=== Writing Records ===
Wrote user:1 at position Position { segment_id: 0, offset: 71 }
Wrote user:2 at position Position { segment_id: 0, offset: 98 }
Deleted user:1 at position Position { segment_id: 0, offset: 125 }
```

The WAL automatically recovered the 3 records from the previous run! This is the power of a WAL - **your data survives across restarts**.

---

## Understanding the Output

### Recovery Stats

```rust
let (wal, recovery_info) = Wal::open(config).await?;
```

Every time you open a WAL, it scans existing segment files and recovers valid records. The `RecoveryInfo` tells you:

| Field | Meaning |
|-------|---------|
| `valid_records` | How many records were successfully recovered |
| `segments_scanned` | How many segment files were checked |
| `corruption_detected` | Whether any corruption was found (and truncated) |
| `bytes_truncated` | How much data was removed due to corruption |

{: .highlight }
If `corruption_detected` is `true`, don't panic! The WAL uses a "prefix-valid" recovery strategy: it keeps all valid records and only discards incomplete or corrupted data at the tail.

### Positions

```rust
let pos = wal.append(&record).await?;
// Position { segment_id: 0, offset: 27 }
```

Every record has a position in the log:
- `segment_id`: Which segment file (starts at 0)
- `offset`: Byte offset within that segment

You can use positions to:
- Read from a specific point
- Track your progress through the log
- Implement checkpointing

---

## What's Happening Under the Hood?

When you run this example:

1. **First Run**
   - `Wal::open()` creates a new directory `wal/`
   - Creates segment file `wal/000000.wal`
   - Pre-allocates it to 128MB (on supported platforms)
   - Writes 3 records (total ~71 bytes)
   - `sync()` calls `fsync()` to ensure durability

2. **Second Run**
   - `Wal::open()` finds existing `wal/000000.wal`
   - Scans it and validates CRC32C for each record
   - Recovers all 3 valid records
   - Continues appending new records after them

3. **On Disk**
   ```
   wal/
     000000.wal  (128MB pre-allocated, ~142 bytes used)
   ```

---

## Try Simulating a Crash

Let's see recovery in action! Modify your program to crash mid-write:

```rust
// Write a few records
for i in 1..=5 {
    let key = format!("key:{}", i);
    let value = format!("value:{}", i);
    let record = Record::put(key.as_bytes(), value.as_bytes());
    wal.append(&record).await?;

    // Crash after record 3 (before sync!)
    if i == 3 {
        println!("CRASH!");
        std::process::exit(1);
    }
}

wal.sync().await?;  // Never reached!
```

Run it:

```bash
$ cargo run
Wrote record 1
Wrote record 2
Wrote record 3
CRASH!

$ cargo run
=== Recovery Stats ===
  Valid records recovered: 3    <--- Only 3 recovered!
```

Why only 3? Because we didn't call `sync()` after records 4 and 5. They were in the OS buffer but never made it to disk.

{: .important }
**Key Takeaway**: If you want durability, you must call `sync()`. Or use `FsyncPolicy::Always` to sync after every write (slower but maximally safe).

---

## Next Steps

Now that you've written your first WAL program, you can:

- **[Understand configuration options](configuration)** - Tune for your workload
- **[Learn about fsync policies](../core-concepts/fsync-policies)** - Balance durability vs performance
- **[Explore record types](../core-concepts/records)** - TTL, compression, tombstones
- **[Dive into recovery](../how-it-works/recovery)** - How crash recovery really works

Or jump straight into building something real with our [Recipes](../recipes/) section!

---

## Common Questions

### Where does the data go?

By default, the WAL directory is `wal/` in your current working directory. You can change this:

```rust
let config = WalConfig {
    dir: PathBuf::from("/var/lib/myapp/wal"),
    ..Default::default()
};
```

### How much disk space do I need?

Each segment is pre-allocated to 128MB by default. The WAL creates a new segment when the current one fills up. So you need:
- **Minimum**: 128MB (one active segment)
- **Typical**: 256-512MB (active segment + a few for history)
- **Maximum**: Unlimited (unless you call `delete_segments_before()` to garbage collect)

### Can I use this in production?

Yes! nori-wal is designed for production use. It includes:
- Comprehensive error handling
- Detailed observability
- Extensive test coverage (37 tests including property tests)
- No unsafe code in the public API
- Battle-tested recovery logic

### What happens if the disk fills up?

With file pre-allocation (default), you'll get an error **when creating a new segment**, not when writing. This is good - you can handle the error gracefully instead of discovering you're out of space mid-write.

---

## Troubleshooting

**Q: I get `No such file or directory` error**
```
Error: IO error: No such file or directory (os error 2)
```

**A:** Make sure your WAL directory's parent exists. For example, if you set `dir: "/var/lib/myapp/wal"`, make sure `/var/lib/myapp/` exists first.

---

**Q: Recovery says I have corruption**
```
Recovery Stats:
  Corruption detected: true
  Bytes truncated: 1234
```

**A:** This is usually harmless! It just means the last write was incomplete (e.g., you ctrl-C'd mid-write). The WAL truncates the partial data and keeps all complete records.

---

**Q: Performance is slow**
```
// Takes 2ms per write!
wal.append(&record).await?
```

**A:** You're probably using `FsyncPolicy::Always` (the default is `Batch`). Check your config:

```rust
let config = WalConfig {
    fsync_policy: FsyncPolicy::Batch(Duration::from_millis(5)),  // Much faster!
    ..Default::default()
};
```

---

Congrats! You now understand the basics of nori-wal. Ready to dive deeper? Check out [Core Concepts](../core-concepts/what-is-wal) next.
