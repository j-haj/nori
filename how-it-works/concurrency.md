---
layout: default
title: Concurrency Model
parent: How It Works
nav_order: 3
---

# Concurrency Model
{: .no_toc }

How nori-wal handles concurrent reads and writes safely and efficiently.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Overview

nori-wal is designed to be **safe for concurrent use** across multiple threads. The type signature reflects this:

```rust
pub struct Wal: Send + Sync
```

This means:
- **Send**: Can be moved between threads
- **Sync**: Can be shared between threads (via `Arc<Wal>`)

However, the concurrency model has **specific characteristics** you need to understand.

---

## Thread Safety Guarantees

### What is Safe

```rust
// Safe: Multiple threads can share a WAL
let wal = Arc::new(wal);

let wal1 = wal.clone();
tokio::spawn(async move {
    wal1.append(&record).await?;  // Safe
});

let wal2 = wal.clone();
tokio::spawn(async move {
    wal2.append(&record).await?;  // Safe
});
```

### What is Serialized

**All writes are serialized internally**. Even if multiple threads call `append()` concurrently, they execute sequentially:

```
Thread 1: append(R1) ─┐
Thread 2: append(R2) ──┼─→ Mutex → [R1][R2] (sequential on disk)
Thread 3: append(R3) ─┘
```

**Why?**
- Append-only log must maintain order
- Only one thread can write to a file at a time
- Serialization ensures consistency

---

## Internal Locking Structure

```rust
pub struct SegmentManager {
    config: SegmentConfig,

    // Protected by Mutex - one writer at a time
    current: Arc<Mutex<SegmentFile>>,
    current_id: Arc<Mutex<u64>>,
    last_fsync: Arc<Mutex<Option<Instant>>>,

    // FD cache for readers
    fd_cache: Arc<Mutex<FdCache>>,

    // Immutable (no lock needed)
    meter: Arc<dyn Meter>,
    node_id: u32,
}
```

### Locks Explained

| Field | Lock Type | Purpose | Contention |
|-------|-----------|---------|------------|
| `current` | `Mutex<SegmentFile>` | Active segment for writing | High on writes |
| `current_id` | `Mutex<u64>` | Current segment ID | Low (rarely changes) |
| `last_fsync` | `Mutex<Option<Instant>>` | Last fsync time (for batching) | High on writes |
| `fd_cache` | `Mutex<FdCache>` | File descriptor cache | Medium on reads |

---

## Write Path Concurrency

### Append Operation

```rust
pub async fn append(&self, record: &Record) -> Result<Position> {
    // 1. Acquire lock (blocks if another thread is writing)
    let mut current = self.current.lock().await;

    // 2. Encode record (CPU-bound, lock held)
    let encoded = record.encode();

    // 3. Check if rotation needed
    if current.would_exceed(encoded.len(), self.config.max_segment_size) {
        // Rotation requires multiple locks - deadlock potential?
        // Answer: No, because we hold current lock exclusively
        self.rotate(&mut current).await?;
    }

    // 4. Append to segment (I/O-bound, lock held)
    let offset = current.append(record).await?;

    // 5. Fsync based on policy (I/O-bound, lock held)
    self.maybe_fsync(&mut current).await?;

    // 6. Release lock (implicit on drop)
    Ok(Position {
        segment_id: current.id,
        offset,
    })
}
```

**Critical section duration**:
- Without fsync: ~10-50μs (encoding + write)
- With fsync: ~1-5ms (encoding + write + fsync)

---

### Write Contention

When multiple threads write concurrently:

```
Time →
Thread 1: [acquire lock] [encode] [write] [fsync] [release]
Thread 2:                 [blocked................] [acquire lock] [encode] [write] [fsync] [release]
Thread 3:                                          [blocked...................] [acquire lock] ...
```

**Performance impact**:

| Fsync Policy | Single Thread | 4 Threads | Contention Overhead |
|--------------|---------------|-----------|---------------------|
| Always | ~420 writes/sec | ~420 writes/sec | None (disk-bound) |
| Batch(5ms) | ~86K writes/sec | ~70K writes/sec | ~18% (lock overhead) |
| Os | ~110K writes/sec | ~85K writes/sec | ~23% (lock overhead) |

**Why contention?**
- Lock is held during encode, write, and fsync
- Multiple threads can't make progress in parallel
- CPU encoding happens while lock is held (could be optimized)

---

### Optimization: Pre-Encode Before Locking

**Current**:
```rust
async fn append(&self, record: &Record) -> Result<Position> {
    let mut current = self.current.lock().await;  // Lock
    let encoded = record.encode();                 // Encode (CPU)
    current.write_all(&encoded).await?;            // Write
    // ...
}
```

**Potential optimization** (not implemented):
```rust
async fn append(&self, record: &Record) -> Result<Position> {
    let encoded = record.encode();                 // Encode BEFORE lock
    let mut current = self.current.lock().await;  // Lock
    current.write_all(&encoded).await?;            // Write
    // ...
}
```

**Benefit**: Reduce lock hold time by ~5-10μs (encoding time)

**Not implemented because**:
- Encoding is fast (~2-10μs)
- Optimization adds complexity
- Lock contention is not the bottleneck for most workloads

---

## Read Path Concurrency

### Sequential Reading

```rust
pub async fn read_from(&self, position: Position) -> Result<SegmentReader> {
    // 1. Acquire FD cache lock
    let fd = self.fd_cache.lock().await.get_or_open(position.segment_id).await?;

    // 2. Create reader (does not hold lock)
    Ok(SegmentReader {
        file: fd,  // Arc<Mutex<File>>
        position: position.offset,
        buffer: Vec::new(),
    })
}
```

### Reading with Concurrent Writes

**Readers do NOT block writers** (and vice versa):

```
Writer Thread: [append R1] [append R2] [append R3]
Reader Thread:               [read R1] [read R2] [block until R3 written]
```

**Why?**
- Readers have separate file descriptors (via FD cache)
- Reads and writes use different syscalls (no OS-level conflicts)
- append-only means no in-place modifications

---

### Multiple Concurrent Readers

Multiple readers can read simultaneously:

```rust
let wal = Arc::new(wal);

// Reader 1: Read from beginning
let wal1 = wal.clone();
tokio::spawn(async move {
    let mut reader = wal1.read_from(Position::start()).await?;
    while let Some((record, _)) = reader.next_record().await? {
        // Process
    }
});

// Reader 2: Read from position 1000
let wal2 = wal.clone();
tokio::spawn(async move {
    let mut reader = wal2.read_from(Position { segment_id: 0, offset: 1000 }).await?;
    while let Some((record, _)) = reader.next_record().await? {
        // Process
    }
});
```

**Performance**: No contention between readers (each has independent FD)

---

## File Descriptor Caching

### Why Cache FDs?

Opening files is expensive:
- ~100μs per `File::open()` syscall
- ~1μs for cache hit

**Without caching**:
```rust
// Reader 1 opens segment 0: 100μs
// Reader 2 opens segment 0: 100μs (redundant!)
// Reader 3 opens segment 0: 100μs (redundant!)
```

**With caching**:
```rust
// Reader 1 opens segment 0: 100μs (cache miss, opens FD)
// Reader 2 opens segment 0: 1μs (cache hit, reuses FD)
// Reader 3 opens segment 0: 1μs (cache hit, reuses FD)
```

---

### FD Cache Structure

```rust
struct FdCache {
    cache: HashMap<u64, Arc<Mutex<File>>>,  // segment_id → File
    max_size: usize,                         // LRU limit (default: 16)
    access_order: Vec<u64>,                  // LRU tracking
}
```

### LRU Eviction

When cache reaches capacity, evict least-recently-used FD:

```rust
fn insert(&mut self, segment_id: u64, file: File) {
    if self.cache.len() >= self.max_size {
        // Evict LRU
        let lru_id = self.access_order.remove(0);
        self.cache.remove(&lru_id);
        // FD automatically closed when Arc<File> is dropped
    }

    self.cache.insert(segment_id, Arc::new(Mutex::new(file)));
    self.access_order.push(segment_id);
}
```

**Example**:
```
Cache size: 3
Access pattern: [0, 1, 2, 3, 0, 1]

0: Cache miss → Open FD0, cache = [0]
1: Cache miss → Open FD1, cache = [0, 1]
2: Cache miss → Open FD2, cache = [0, 1, 2]
3: Cache miss → Open FD3, evict 0, cache = [1, 2, 3]
0: Cache miss → Open FD0, evict 1, cache = [2, 3, 0]
1: Cache miss → Open FD1, evict 2, cache = [3, 0, 1]
```

---

## Rotation and Concurrency

### Rotation Algorithm

```rust
async fn rotate(&self, current: &mut SegmentFile) -> Result<()> {
    // 1. Finalize current segment (truncate to actual size)
    current.finalize().await?;

    // 2. Increment segment ID
    let mut current_id = self.current_id.lock().await;
    *current_id += 1;
    let new_id = *current_id;
    drop(current_id);

    // 3. Create new segment
    let new_segment = SegmentFile::open(
        &self.config.dir,
        new_id,
        true,  // create
        if self.config.preallocate {
            Some(self.config.max_segment_size)
        } else {
            None
        },
    ).await?;

    // 4. Swap in new segment (still holding current lock)
    *current = new_segment;

    Ok(())
}
```

**Key point**: Rotation happens while holding the `current` lock, so no other thread can write during rotation.

---

### Rotation Latency

Rotation adds latency to the write that triggers it:

| Step | Latency |
|------|---------|
| Finalize old segment (truncate + sync) | ~2-5 ms |
| Open new segment file | ~100 μs |
| Preallocate 128MB | ~1-2 ms |
| **Total** | **~3-7 ms** |

**Impact on concurrent writers**:
```
Thread 1: append(triggers rotation) [3-7ms rotation] [return]
Thread 2: append(blocked)            [wait...........] [acquire lock] [append] [return]
Thread 3: append(blocked)                               [wait......] [acquire lock] [append] [return]
```

**Mitigation**: Rotation is rare (once per 128MB by default), so amortized cost is negligible.

---

## Sync and Concurrency

### Explicit Sync

```rust
wal.sync().await?;
```

Acquires the lock and forces fsync:

```rust
async fn sync(&self) -> Result<()> {
    let mut current = self.current.lock().await;
    current.sync().await?;

    // Update last_fsync time
    let mut last_fsync = self.last_fsync.lock().await;
    *last_fsync = Some(Instant::now());

    Ok(())
}
```

**Blocks all writers** until fsync completes (~1-5ms).

---

### Batched Sync (FsyncPolicy::Batch)

```rust
async fn maybe_fsync(&self, current: &mut SegmentFile) -> Result<()> {
    if let FsyncPolicy::Batch(window) = self.config.fsync_policy {
        let mut last_fsync = self.last_fsync.lock().await;
        let now = Instant::now();

        if last_fsync.map_or(true, |t| now.duration_since(t) >= window) {
            current.sync().await?;
            *last_fsync = Some(now);
        }
    }

    Ok(())
}
```

**First write in window**: Pays fsync cost (~1-5ms)
**Subsequent writes in window**: No fsync (fast)

---

## Drop Handler and Finalization

### Best-Effort Finalization

When `Wal` is dropped, we try to finalize the current segment:

```rust
impl Drop for SegmentManager {
    fn drop(&mut self) {
        // Try to acquire lock (non-blocking)
        if let Ok(current) = self.current.try_lock() {
            // Synchronous truncation (can't do async in Drop)
            if current.size != actual_file_size {
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&current.path)
                    .and_then(|f| {
                        f.set_len(current.size)?;
                        f.sync_all()
                    });
            }
        }
        // If lock is held, skip finalization (segment will be recovered on next open)
    }
}
```

**Why best-effort?**
- Can't do async work in Drop
- Can't block indefinitely (may be in async runtime shutdown)
- Recovery will fix any unfinalizedSegments

---

## Deadlock Analysis

### Potential Deadlock Scenario?

```rust
// Thread 1: Holds current lock, tries to acquire current_id lock
async fn rotate(&self, current: &mut SegmentFile) {
    // current lock held
    let mut current_id = self.current_id.lock().await;  // Acquire current_id
    // ...
}

// Thread 2: Holds current_id lock, tries to acquire current lock?
// ... No such code path exists!
```

**Conclusion**: **No deadlock possible**
- Lock acquisition order is always: `current` → `current_id` → `last_fsync`
- Never acquire locks in reverse order
- Locks are released quickly (no long-held locks)

---

## Performance Benchmarks

### Single-Threaded Performance

| Fsync Policy | Throughput | Latency (p50) | Latency (p99) |
|--------------|------------|---------------|---------------|
| Always | 420 writes/sec | 2.1 ms | 9.8 ms |
| Batch(5ms) | 86,000 writes/sec | 38 μs | 5.3 ms |
| Os | 110,000 writes/sec | 28 μs | 95 μs |

---

### Multi-Threaded Performance (4 threads)

| Fsync Policy | Single Thread | 4 Threads | Scalability |
|--------------|---------------|-----------|-------------|
| Always | 420/sec | 420/sec | 1.0x (disk-bound) |
| Batch(5ms) | 86K/sec | 70K/sec | 0.82x (lock contention) |
| Os | 110K/sec | 85K/sec | 0.77x (lock contention) |

**Observation**: Write-heavy workloads don't scale linearly due to serialized append.

---

### Read Performance (Concurrent)

| Scenario | Single Reader | 4 Readers | Scalability |
|----------|---------------|-----------|-------------|
| Sequential read (cached FDs) | 200K records/sec | 750K records/sec | 3.75x |
| Sequential read (uncached FDs) | 180K records/sec | 600K records/sec | 3.33x |

**Observation**: Reads scale well because they don't share locks with writers.

---

## Best Practices

### 1. Share WAL Across Threads

```rust
// Good: Share a single WAL instance
let wal = Arc::new(wal);

for _ in 0..num_threads {
    let wal = wal.clone();
    tokio::spawn(async move {
        wal.append(&record).await?;
    });
}
```

```rust
// Bad: Open multiple WALs (file corruption risk!)
// DON'T DO THIS:
for _ in 0..num_threads {
    tokio::spawn(async move {
        let (wal, _) = Wal::open(config.clone()).await?;  // Multiple WALs!
        wal.append(&record).await?;
    });
}
```

---

### 2. Batch Writes in Application Code

If you have high write contention, batch at the application level:

```rust
// Instead of:
for record in records {
    wal.append(&record).await?;  // Lock per record
}

// Do this:
let mut batch = Vec::new();
for record in records {
    batch.push(record);
}

// Append batch with single lock acquisition
for record in batch {
    wal.append(&record).await?;
}
```

Or use `append_batch()` if available (future feature).

---

### 3. Use Separate WALs for Independent Data

If you have independent streams of data, use separate WALs:

```rust
// Good: Separate WALs for different purposes
let user_wal = Wal::open(WalConfig { dir: "wal/users".into(), .. }).await?;
let event_wal = Wal::open(WalConfig { dir: "wal/events".into(), .. }).await?;

// No lock contention between user and event writes
tokio::join!(
    user_wal.append(&user_record),
    event_wal.append(&event_record),
);
```

---

### 4. Don't Hold Locks Across Async Points

```rust
// Bad: Don't do this (not possible with current API, but illustrative)
let current = wal.manager.current.lock().await;  // Acquire lock
tokio::time::sleep(Duration::from_secs(1)).await;  // Hold lock across sleep!
current.append(&record).await?;

// Good: Lock is acquired and released within a single operation
wal.append(&record).await?;  // Lock acquired and released internally
```

---

## Key Takeaways

1. **WAL is thread-safe (Send + Sync)**
   - Can be shared across threads via `Arc<Wal>`
   - Multiple readers and writers are safe

2. **Writes are serialized internally**
   - Only one thread can append at a time
   - Lock held during encode + write + fsync
   - Contention reduces throughput by ~20-30%

3. **Reads don't block writes (and vice versa)**
   - Separate file descriptors via FD cache
   - Concurrent readers scale linearly (~3.75x with 4 threads)

4. **FD cache reduces open() overhead**
   - LRU cache with default size 16
   - Cache hit: ~1μs, cache miss: ~100μs

5. **Rotation happens while holding lock**
   - Adds ~3-7ms latency to triggering write
   - Rare event (once per 128MB)
   - No deadlock possible

6. **Best practice: Share one WAL per directory**
   - Use `Arc<Wal>` for sharing
   - Don't open multiple WALs for same directory

---

## What's Next?

You've completed the "How It Works" section! Now explore:

- **[API Reference](../api-reference/)** - Documentation for all public types
- **[Recipes](../recipes/)** - Build real applications with nori-wal
- **[Performance Tuning](../performance/tuning)** - Optimize for your workload

Or dive into the implementation in `crates/nori-wal/src/` on GitHub.
