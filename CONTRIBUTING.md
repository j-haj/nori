# Contributing to NoriKV

Thanks for your interest!

- Rust edition 2021; MSRV 1.75
- Run `cargo fmt`, `clippy -D warnings`, and `cargo test` before submitting PRs.
- Keep design docs and `context/` files in sync with code changes.
- Avoid vendor-specific telemetry in core crates; use `nori-observe`.
