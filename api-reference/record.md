---
layout: default
title: Record
parent: API Reference
nav_order: 2
---

# Record API
{: .no_toc }

Complete API reference for the `Record` type - the fundamental unit of data in nori-wal.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Overview

A `Record` represents a single key-value operation in the WAL. Records support:

- **PUT operations** - Store key-value pairs
- **DELETE operations** - Tombstone markers for deletions
- **TTL support** - Optional time-to-live for records
- **Compression** - LZ4 and Zstd compression for values
- **Integrity** - CRC32C checksums for corruption detection

### Type Definition

[View source in `crates/nori-wal/src/record.rs`](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L64-L72)

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `key` | `Bytes` | The record's key (immutable byte buffer) |
| `value` | `Bytes` | The record's value (immutable byte buffer) |
| `tombstone` | `bool` | `true` for DELETE operations, `false` for PUT |
| `ttl` | `Option<Duration>` | Optional time-to-live for the record |
| `compression` | `Compression` | Compression algorithm used for the value |

---

## Creating Records

### `Record::put`

Creates a new PUT record.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L76-L84)

**Parameters:**
- `key` - The record's key (accepts `&[u8]`, `Vec<u8>`, `String`, etc.)
- `value` - The record's value (accepts `&[u8]`, `Vec<u8>`, `String`, etc.)

**Returns:** A new `Record` with `tombstone = false` and no TTL or compression.

**Examples:**

```rust
use nori_wal::Record;

// From byte slices
let record = Record::put(b"user:123", b"Alice");

// From Strings
let key = String::from("session:abc");
let value = String::from("active");
let record = Record::put(key, value);

// From Vecs
let key = vec![1, 2, 3];
let value = vec![4, 5, 6];
let record = Record::put(key, value);
```

---

### `Record::put_with_ttl`

Creates a new PUT record with a time-to-live.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L87-L95)

**Parameters:**
- `key` - The record's key
- `value` - The record's value
- `ttl` - Time-to-live duration

**Returns:** A new `Record` with the specified TTL.

**Examples:**

```rust
use nori_wal::Record;
use std::time::Duration;

// Cache entry that expires in 5 minutes
let record = Record::put_with_ttl(
    b"cache:result:123",
    b"computed value",
    Duration::from_secs(300),
);

// Session that expires in 1 hour
let record = Record::put_with_ttl(
    b"session:abc",
    b"user_data",
    Duration::from_secs(3600),
);
```

**Notes:**
- TTL is stored as milliseconds internally
- TTL enforcement is the application's responsibility (nori-wal doesn't auto-expire records)
- Maximum TTL is `u64::MAX` milliseconds (~584 million years)

---

### `Record::delete`

Creates a DELETE record (tombstone).

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L98-L106)

**Parameters:**
- `key` - The key to delete

**Returns:** A new `Record` with `tombstone = true` and empty value.

**Examples:**

```rust
use nori_wal::Record;

// Mark a key as deleted
let record = Record::delete(b"user:123");

assert!(record.tombstone);
assert_eq!(record.value.len(), 0);
```

**Tombstone Semantics:**
- Tombstones shadow earlier PUT records for the same key
- Used in LSM compaction to remove deleted keys
- Empty value (storage optimization)

---

## Compression

### `Record::with_compression`

Sets the compression algorithm for the record's value.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L109-L112)

**Parameters:**
- `compression` - Compression algorithm (`Compression::None`, `Compression::Lz4`, or `Compression::Zstd`)

**Returns:** The modified record (builder pattern).

**Examples:**

```rust
use nori_wal::{Record, Compression};

// LZ4 compression (fast)
let record = Record::put(b"key", b"large value".repeat(1000))
    .with_compression(Compression::Lz4);

// Zstd compression (better ratio)
let record = Record::put(b"key", b"large value".repeat(1000))
    .with_compression(Compression::Zstd);

// No compression (default)
let record = Record::put(b"key", b"small value")
    .with_compression(Compression::None);
```

**Compression Algorithms:**

| Algorithm | Speed | Ratio | Best For |
|-----------|-------|-------|----------|
| `Compression::None` | N/A | 1:1 | Small values, already compressed data |
| `Compression::Lz4` | Very fast | ~2-3x | Hot path, large text/JSON |
| `Compression::Zstd` | Moderate | ~3-5x | Cold storage, maximum space savings |

**Performance Tips:**
- Use LZ4 for hot path writes (microseconds overhead)
- Use Zstd for cold storage or archival
- Skip compression for small values (<100 bytes) - overhead not worth it
- Skip compression for already compressed data (JPEG, PNG, etc.)

**Compression Test Results:**

```rust
// Highly compressible data (repeated text)
let value = "hello world ".repeat(100); // 1200 bytes

let none = Record::put(b"k", value.as_bytes()).encode();
let lz4 = Record::put(b"k", value.as_bytes())
    .with_compression(Compression::Lz4)
    .encode();
let zstd = Record::put(b"k", value.as_bytes())
    .with_compression(Compression::Zstd)
    .encode();

// none.len() = 1215 bytes
// lz4.len()  = ~50 bytes (24x compression)
// zstd.len() = ~30 bytes (40x compression)
```

---

## Encoding and Decoding

### `Record::encode`

Encodes the record into bytes with CRC32C checksum.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L115-L163)

**Returns:** Encoded record as immutable `Bytes`.

**Format:**
```
[klen: varint]
[vlen: varint]
[flags: u8]
[ttl_ms?: varint] (if TTL present)
[key: bytes]
[value: bytes]
[crc32c: u32 LE]
```

**Flags byte:**
- Bit 0: `tombstone` (1 = DELETE, 0 = PUT)
- Bit 1: `ttl_present` (1 = TTL follows, 0 = no TTL)
- Bits 2-3: `compression` (0 = None, 1 = LZ4, 2 = Zstd)
- Bits 4-7: Reserved (must be 0)

**Examples:**

```rust
use nori_wal::Record;

let record = Record::put(b"user:123", b"Alice");
let encoded = record.encode();

// Write to WAL
wal.append(&record).await?;

// Or write to arbitrary storage
file.write_all(&encoded).await?;
```

**Varint Encoding:**
- Key and value lengths use LEB128 variable-length encoding
- 1 byte for lengths < 128
- 2 bytes for lengths < 16,384
- Etc.

**CRC32C Checksum:**
- Computed over entire record (excluding the CRC itself)
- Detects corruption during recovery
- Hardware-accelerated on modern CPUs (SSE 4.2)

---

### `Record::decode`

Decodes a record from bytes, validating the CRC32C checksum.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L166-L199)

**Parameters:**
- `data` - Byte slice containing the encoded record

**Returns:**
- `Ok((record, bytes_consumed))` - Successfully decoded record and number of bytes read
- `Err(RecordError)` - Decoding failed

**Errors:**

| Error | Cause |
|-------|-------|
| `RecordError::Incomplete` | Not enough bytes to decode |
| `RecordError::CrcMismatch` | Checksum validation failed |
| `RecordError::InvalidCompression` | Unknown compression type |
| `RecordError::DecompressionFailed` | Decompression error |
| `RecordError::Io` | I/O error (e.g., varint overflow) |

**Examples:**

```rust
use nori_wal::Record;

// Decode a record
let (record, bytes_consumed) = Record::decode(&encoded)?;

// Handle errors
match Record::decode(&corrupted_data) {
    Ok((record, _)) => println!("Decoded: {:?}", record),
    Err(RecordError::CrcMismatch { expected, actual }) => {
        eprintln!("Corruption detected: expected {:#x}, got {:#x}", expected, actual);
    }
    Err(RecordError::Incomplete) => {
        eprintln!("Incomplete record, need more data");
    }
    Err(e) => eprintln!("Decode error: {}", e),
}
```

**Decoding from Streams:**

```rust
use nori_wal::Record;

let mut buffer = Vec::new();
file.read_to_end(&mut buffer).await?;

let mut offset = 0;
while offset < buffer.len() {
    match Record::decode(&buffer[offset..]) {
        Ok((record, size)) => {
            println!("Record: {:?}", record);
            offset += size;
        }
        Err(RecordError::Incomplete) => {
            // Need more data
            break;
        }
        Err(e) => {
            eprintln!("Decode error at offset {}: {}", offset, e);
            break;
        }
    }
}
```

---

## Compression Enum

### `Compression`

Compression algorithm for record values.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L34-L39)

**Variants:**

| Variant | Value | Description |
|---------|-------|-------------|
| `None` | 0 | No compression |
| `Lz4` | 1 | LZ4 block compression (fast) |
| `Zstd` | 2 | Zstandard compression (high ratio) |

**Examples:**

```rust
use nori_wal::Compression;

let comp = Compression::Lz4;
assert_eq!(comp as u8, 1);
```

---

## Error Handling

### `RecordError`

Errors that can occur during record encoding/decoding.

[View source](https://github.com/j-haj/nori/blob/main/crates/nori-wal/src/record.rs#L17-L31)

**Variants:**

| Variant | Description |
|---------|-------------|
| `Io(io::Error)` | I/O error (e.g., varint overflow) |
| `CrcMismatch { expected, actual }` | Checksum validation failed |
| `InvalidCompression(u8)` | Unknown compression type byte |
| `CompressionFailed(String)` | Compression error (shouldn't happen) |
| `DecompressionFailed(String)` | Decompression error (data corruption) |
| `Incomplete` | Not enough bytes to decode record |

**Examples:**

```rust
use nori_wal::{Record, RecordError};

match Record::decode(data) {
    Ok((record, _)) => { /* success */ }
    Err(RecordError::CrcMismatch { expected, actual }) => {
        // Corruption detected
        log::error!("CRC mismatch: expected {:#x}, got {:#x}", expected, actual);
    }
    Err(RecordError::Incomplete) => {
        // Need more data (normal for streaming)
    }
    Err(RecordError::DecompressionFailed(msg)) => {
        // Compressed data is corrupt
        log::error!("Decompression failed: {}", msg);
    }
    Err(e) => {
        log::error!("Decode error: {}", e);
    }
}
```

---

## Wire Format Details

### Record Format

```
┌─────────────────────────────────────────────────┐
│ klen (varint)          │ 1-10 bytes             │
├─────────────────────────────────────────────────┤
│ vlen (varint)          │ 1-10 bytes             │
├─────────────────────────────────────────────────┤
│ flags (u8)             │ 1 byte                 │
├─────────────────────────────────────────────────┤
│ ttl_ms (varint)        │ 0 or 1-10 bytes        │ (if TTL_PRESENT bit set)
├─────────────────────────────────────────────────┤
│ key                    │ klen bytes             │
├─────────────────────────────────────────────────┤
│ value                  │ vlen bytes             │
├─────────────────────────────────────────────────┤
│ crc32c (u32 LE)        │ 4 bytes                │
└─────────────────────────────────────────────────┘
```

### Flags Byte

```
 7  6  5  4  3  2  1  0
┌──┬──┬──┬──┬──┬──┬──┬──┐
│ Reserved │Cmp│TL│TS│
└──┴──┴──┴──┴──┴──┴──┴──┘
            │ │  │  └─ Tombstone (0=PUT, 1=DELETE)
            │ │  └──── TTL present (0=no, 1=yes)
            │ └─────── Compression (00=None, 01=LZ4, 10=Zstd, 11=reserved)
            └────────── Reserved (must be 0)
```

### Compression Details

**LZ4 Compressed Value:**
```
[original_size: varint][compressed_data]
```

**Zstd Compressed Value:**
```
[compressed_data]  (zstd frame includes original size)
```

### Size Examples

**Minimal record** (empty key/value, no TTL):
```
1 + 1 + 1 + 4 = 7 bytes
```

**Typical record** (`key="user:123"`, `value="Alice"`):
```
1 (klen) + 1 (vlen) + 1 (flags) + 8 (key) + 5 (value) + 4 (crc) = 20 bytes
```

**Record with TTL** (add ~2-5 bytes):
```
1 (klen) + 1 (vlen) + 1 (flags) + 3 (ttl) + key + value + 4 (crc)
```

---

## Usage Patterns

### Basic PUT/DELETE

```rust
use nori_wal::{Wal, Record, WalConfig};

let wal = Wal::open(WalConfig::default()).await?;

// Write a key-value pair
let record = Record::put(b"user:123", b"Alice");
wal.append(&record).await?;

// Delete a key
let record = Record::delete(b"user:456");
wal.append(&record).await?;
```

### TTL for Caching

```rust
use std::time::Duration;

// Cache entry expires in 5 minutes
let record = Record::put_with_ttl(
    b"cache:expensive_query",
    b"cached result",
    Duration::from_secs(300),
);

wal.append(&record).await?;

// Later, check if expired
if let Some(ttl) = record.ttl {
    let elapsed = SystemTime::now().duration_since(creation_time)?;
    if elapsed > ttl {
        println!("Record expired!");
    }
}
```

### Compression for Large Values

```rust
use nori_wal::Compression;

// Large JSON document
let json = serde_json::to_vec(&large_object)?;

let record = Record::put(b"document:123", json)
    .with_compression(Compression::Zstd);

wal.append(&record).await?;
```

### Batched Writes

```rust
let records = vec![
    Record::put(b"k1", b"v1"),
    Record::put(b"k2", b"v2"),
    Record::put(b"k3", b"v3"),
];

for record in records {
    wal.append(&record).await?;
}

// Sync once for all records
wal.sync().await?;
```

### Recovery from WAL

```rust
use nori_wal::{Wal, WalConfig, Position};
use std::collections::HashMap;

// Replay WAL to reconstruct state
let wal = Wal::open(WalConfig::default()).await?;
let mut state: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();

let mut reader = wal.read_from(Position::start()).await?;
while let Some((record, _pos)) = reader.next_record().await? {
    if record.tombstone {
        state.remove(&record.key.to_vec());
    } else {
        state.insert(record.key.to_vec(), record.value.to_vec());
    }
}

println!("Recovered {} keys", state.len());
```

---

## Best Practices

### 1. Choose Appropriate Compression

```rust
// GOOD: Compress large, repetitive data
let large_json = serde_json::to_vec(&data)?;
if large_json.len() > 1000 {
    Record::put(key, large_json).with_compression(Compression::Lz4)
} else {
    Record::put(key, large_json)
}

// BAD: Compress already compressed data
let jpeg = fs::read("photo.jpg")?;
Record::put(b"photo", jpeg).with_compression(Compression::Lz4) // Wastes CPU
```

### 2. Handle TTL Properly

```rust
// GOOD: Application enforces TTL
struct CacheEntry {
    record: Record,
    inserted_at: SystemTime,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        if let Some(ttl) = self.record.ttl {
            self.inserted_at.elapsed().unwrap() > ttl
        } else {
            false
        }
    }
}

// BAD: Assuming WAL auto-expires records
// (it doesn't - TTL is just metadata)
```

### 3. Use Tombstones Correctly

```rust
// GOOD: Tombstone to mark deletion
memtable.insert(key, value);
wal.append(&Record::put(key, value)).await?;

// Later, to delete:
memtable.remove(key);
wal.append(&Record::delete(key)).await?;

// BAD: Overwriting with empty value
wal.append(&Record::put(key, b"")).await? // Not a deletion!
```

### 4. Validate After Decode

```rust
// GOOD: Check CRC on recovery
match Record::decode(data) {
    Ok((record, size)) => {
        // CRC validated automatically
        process(record);
    }
    Err(RecordError::CrcMismatch { .. }) => {
        log::warn!("Skipping corrupt record at offset {}", offset);
        // Continue or abort recovery
    }
    Err(e) => return Err(e.into()),
}

// BAD: Skipping CRC check (don't do this)
// (decode() always validates CRC)
```

---

## Performance Characteristics

### Encoding Performance

| Operation | Time | Notes |
|-----------|------|-------|
| `encode()` (no compression) | ~100-200ns | Varint + copy + CRC |
| `encode()` (LZ4, 1KB value) | ~2-5µs | Fast compression |
| `encode()` (Zstd, 1KB value) | ~10-50µs | Slower, better ratio |

### Decoding Performance

| Operation | Time | Notes |
|-----------|------|-------|
| `decode()` (no compression) | ~100-200ns | Varint + copy + CRC |
| `decode()` (LZ4, 1KB value) | ~1-3µs | Fast decompression |
| `decode()` (Zstd, 1KB value) | ~5-20µs | Slower decompression |

### Memory Efficiency

- **Zero-copy**: Uses `Bytes` (reference-counted) for key/value
- **Shared buffers**: Cloning a `Record` only increments refcount
- **Compression**: Can reduce memory footprint 3-10x for text data

---

## See Also

- [Wal API](wal) - Main WAL interface
- [WalConfig API](config) - Configuration options
- [RecordError](errors) - Error handling
- [How It Works: Record Format](../how-it-works/record-format) - Deep dive into wire format
- [Core Concepts: Append-Only](../core-concepts/append-only) - Why records are immutable
