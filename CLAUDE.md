# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

NoriKV is a **sharded, Raft-replicated, log-structured key–value store** with portable SDKs and first-class observability. The repository is organized as a Cargo workspace with multiple crates for storage, consensus, membership, and observability, plus server binaries and client SDKs.

## Build & Development Commands

### Building
```bash
# Build all workspace members
cargo build

# Build specific crates
cargo build -p nori-observe -p norikv-server

# Build specific public libraries
cargo build -p nori-wal -p nori-sstable -p nori-lsm -p nori-raft -p nori-swim
```

### Testing
```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p nori-lsm

# Run a specific test by name
cargo test -p nori-lsm test_name
```

### Linting & Formatting
```bash
# Format code (required before PRs)
cargo fmt

# Run clippy with strict warnings (required before PRs)
cargo clippy -- -D warnings
```

### MSRV
Minimum Supported Rust Version: **1.75**
Rust Edition: **2021**

## Architecture Overview

### Context Pack (Source of Truth)

- **`00_index.yaml`** - Start here; guides you through the context pack structure
- **`10_product.yaml`** - Goals, SLOs, limits
- **`20_architecture.yaml`** - Crate layering, published packages, stable ports/traits
- **`30_storage.yaml`** - WAL, SSTable, LSM compaction, Storage trait
- **`31_consensus.yaml`** - Raft core (consensus, read-index, leases, snapshots)
- **`32_membership.yaml`** - SWIM failure detection
- **`33_placement.yaml`** - Sharding/hashing (Jump Consistent Hash, xxhash64)
- **`34_transport.yaml`** + **`40_protocols.proto`** - gRPC/HTTP transport, API definitions
- **`35_metrics.yaml`** - Observability, metrics, VizEvent schema
- **`36_dashboard.yaml`** - Live dashboard design
- **`50-53_sdk_*.yaml`** - SDK guidance for TypeScript, Python, Go, Java
- **`60_ops.yaml`** - Operations
- **`70_testkit.yaml`** - Chaos testing, deterministic faults
- **`95_publishing.yaml`** - Publishing policy

### Layering Model
```
┌─────────────────────────────────────────┐
│  Server Node (DI composition)           │
├─────────────────────────────────────────┤
│  Adapters: LSM, Raft, SWIM, gRPC, HTTP  │
├─────────────────────────────────────────┤
│  Ports: Storage, ReplicatedLog,         │
│         Membership, Placement,          │
│         Transport, Router traits        │
├─────────────────────────────────────────┤
│  Domain: types, IDs, versions, errors   │
└─────────────────────────────────────────┘
```

### Workspace Crates

#### Public Libraries (intended for publication)
- **`nori-observe`** - Vendor-neutral observability ABI: `Meter` trait + `VizEvent` enums + macros
- **`nori-wal`** - Append-only write-ahead log with recovery and rotation
- **`nori-sstable`** - Immutable sorted tables with blocks, index, bloom, compression
- **`nori-lsm`** - Embeddable LSM engine (WAL+SST+compaction+snapshots); implements `Storage` trait
- **`nori-swim`** - SWIM-like membership/failure detector with events
- **`nori-raft`** - Raft core: consensus, read-index, leases, snapshots, joint reconfiguration

#### Internal Crates (not published)
- **`norikv-types`** - Shared IDs, error codes, small enums
- **`norikv-placement`** - Shard/replica planning with Jump Consistent Hash; minimal-move diffs
- **`norikv-transport-grpc`** - Tonic-based gRPC + HTTP façade
- **`norikv-testkit`** - Chaos simulator, linearizability checker, deterministic net/disk faults
- **`nori-observe-prom`** - Prometheus/OpenMetrics exporter (optional publication)
- **`nori-observe-otlp`** - OTLP gRPC exporter with trace exemplars (optional publication)

#### Applications
- **`apps/norikv-server`** - Main node binary (composition, config, health, auth, metrics, admin)
- **`apps/norikv-vizd`** - Event aggregator + WebSocket/gRPC-web stream for dashboard
- **`apps/norikv-dashboard`** - Next.js/React animated UI

#### SDKs
- **`sdks/typescript/`** - TypeScript client SDK
- **`sdks/python/`** - Python client SDK
- **`sdks/go/`** - Go client SDK
- **`sdks/java/`** - Java client SDK

## Critical Design Constraints

### Observability
- **Never import vendor-specific telemetry in core crates**
- Always use `nori-observe::Meter` trait injected via constructors
- Emit typed `VizEvent` enums for dashboard streaming
- Zero-allocation in hot paths; budgets: counter ≤80ns, histogram ≤200ns

### Hashing & Placement
- **All hashing must use the shared code path defined in `33_placement.yaml`**
- Key hash: `xxhash64(seed=0)`
- Consistent hash: `jump_consistent_hash`
- Default: 1024 virtual shards, replication factor 3

### Storage Invariants (from `30_storage.yaml`)
- Newest version shadows all older versions across LSM levels
- No key range overlaps in L≥1
- WAL recovery: prefix-valid only; truncate partial tail → exactly-once semantics for last committed version
- Compaction preserves tombstones if lower levels may contain older live values

### Code Organization
- Favor small, testable crates
- Keep traits stable; extend via adapters

## Testing Strategy

### Property Testing (via `norikv-testkit`)
- Deterministic chaos: simulated clock, network (drop/dup/reorder), disk faults
- Linearizability checker for single-key histories
- Scenario examples:
  - `partitions_and_restarts`: partition leaders, force election, heal, validate history
  - `l0_storm`: hammer writes, test compaction throttling, validate SLO tails
  - `wal_powercut`: kill during append, recover, check model parity

### Key Test Targets
- WAL recovery keeps last committed version
- Compaction never drops last write
- p95 GET latency: 10ms
- p95 PUT latency: 20ms

## SDK Development

- Language-specific API patterns
- Test vectors for key→shard mapping (must match across all SDKs)
- Error handling conventions
- Connection pooling strategies

## Skeleton Status
- Always reference @context/ files for planning, architecture, design decisions, and adding new features or code