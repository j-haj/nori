---
layout: default
title: Installation
parent: Getting Started
nav_order: 0
---

# Installation
{: .no_toc }

How to add nori-wal to your Rust project.
{: .fs-6 .fw-300 }

## Table of contents
{: .no_toc .text-delta }

1. TOC
{:toc}

---

## Add to Cargo.toml

Add nori-wal as a dependency in your `Cargo.toml`:

```toml
[dependencies]
nori-wal = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "fs"] }
```

{: .important }
nori-wal requires **tokio** for async I/O. Make sure you include the `fs` feature!

---

## Minimum Rust Version

nori-wal requires **Rust 1.75 or newer**. Check your version:

```bash
rustc --version
```

If you need to update:

```bash
rustup update stable
```

---

## Platform Support

nori-wal works on all platforms that Rust supports:

| Platform | Status | Notes |
|----------|--------|-------|
| **Linux** | Fully supported | Uses `fallocate()` for optimal pre-allocation |
| **macOS** | Fully supported | Uses `F_PREALLOCATE` for pre-allocation |
| **Windows** | Fully supported | Uses standard `set_len()` for pre-allocation |
| **BSD** | Supported | Uses fallback pre-allocation |
| **Others** | Untested | Should work, but not regularly tested |

---

## Dependencies

nori-wal has minimal dependencies:

### Required
- `tokio` (1.x) with `fs`, `io-util`, `sync`, `time`, `rt` features
- `bytes` (1.x) - Zero-copy byte buffers
- `crc32c` (0.6) - Fast CRC32C checksums
- `thiserror` (1.x) - Error handling
- `bitflags` (2.x) - Type-safe flags

### Compression (included by default)
- `lz4` (1.24) - Fast compression
- `zstd` (0.13) - High-ratio compression

### Platform-specific
- `libc` (0.2) - Only on Unix for optimized `fallocate()` and `fcntl()`

---

## Verifying Installation

Create a simple test program to verify everything works:

```rust
use nori_wal::{Wal, WalConfig};

#[tokio::main]
async fn main() {
    let config = WalConfig::default();
    let result = Wal::open(config).await;

    match result {
        Ok((wal, recovery_info)) => {
            println!("nori-wal is working!");
            println!("   Recovered {} records", recovery_info.valid_records);
            drop(wal);
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }
}
```

Run it:

```bash
cargo run
```

You should see:

```
nori-wal is working!
   Recovered 0 records
```

---

## Development Dependencies

If you're contributing to nori-wal, you'll also need:

```toml
[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tempfile = "3"
proptest = "1"  # For property-based testing
criterion = { version = "0.5", features = ["html_reports", "async_tokio"] }
```

---

## Next Steps

Now that nori-wal is installed:

1. **[Follow the Quickstart](quickstart)** to write your first WAL program
2. **[Learn about Configuration](configuration)** to tune for your use case
3. **[Understand Core Concepts](../core-concepts/what-is-wal)** to use it effectively

---

## Troubleshooting Installation

### Compilation fails on Windows

**Symptom:**
```
error: linking with `link.exe` failed
```

**Solution:** Make sure you have the [Visual C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) installed.

---

### Tokio feature errors

**Symptom:**
```
error: no method named `read` found for struct `File`
```

**Solution:** Make sure you have the `fs` feature enabled for tokio:

```toml
tokio = { version = "1", features = ["fs", "macros", "rt-multi-thread"] }
```

---

### Version conflicts

**Symptom:**
```
error: failed to select a version for `tokio`
```

**Solution:** nori-wal is compatible with tokio 1.x. If you have other dependencies requiring a specific tokio version, make sure they're compatible:

```bash
cargo tree | grep tokio
```

You can force a specific version in your `Cargo.toml`:

```toml
[dependencies]
tokio = "=1.35.0"  # Pin to a specific version if needed
```

---

### Can't find nori-wal on crates.io

**Symptom:**
```
error: no matching package named `nori-wal` found
```

**Solution:** If nori-wal hasn't been published to crates.io yet, you can use the git repository:

```toml
[dependencies]
nori-wal = { git = "https://github.com/j-haj/nori", branch = "main" }
```

Or use a local path during development:

```toml
[dependencies]
nori-wal = { path = "../norikv/crates/nori-wal" }
```

---

Need help? Check the [Troubleshooting Guide](../troubleshooting/) or [open an issue on GitHub](https://github.com/j-haj/nori/issues).
