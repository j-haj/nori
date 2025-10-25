# NoriKV

NoriKV is a **sharded, Raft-replicated, log-structured key–value store** with portable SDKs and first-class observability.

This repo is a Cargo workspace hosting multiple crates (WAL, SSTable, LSM, SWIM membership, Raft) and the server,

- Crates intended for publication: `nori-observe`, `nori-wal`, `nori-sstable`, `nori-lsm`, `nori-swim`, `nori-raft`.
- Internal crates: `norikv-transport-grpc`, `norikv-placement`, `norikv-types`, `norikv-testkit`, etc.

## Quick start (skeleton)

```bash
# From repo root
cargo build -p nori-observe -p norikv-server
```

> This is a skeleton. The core crates include minimal stubs so the workspace builds while you implement features.

## Documentation

- Project docs live in `docs/` (MkDocs/Docusaurus ready).
- Each public crate ships a README and examples.
