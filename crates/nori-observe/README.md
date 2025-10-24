# nori-observe

Vendor-neutral observability ABI for NoriKV and other systems. Provides:
- tiny traits: `Meter`, `Counter`, `Gauge`, `Histogram`
- typed live-visualization events (`VizEvent` and friends)
- simple macros: `obs_count!`, `obs_gauge!`, `obs_hist!`, `obs_timed!`
- a `NoopMeter` for tests

Backends (Prometheus, OTLP, StatsD, viz daemon) live in separate crates and depend on this one.
