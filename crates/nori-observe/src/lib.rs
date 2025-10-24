//! nori-observe: vendor-neutral observability ABI.
//!
//! Core crates depend only on these traits and event types. Backends live elsewhere.

use std::time::Instant;

pub trait Counter: Send + Sync { fn inc(&self, v: u64); }
pub trait Gauge:   Send + Sync { fn set(&self, v: i64); }
pub trait Histogram: Send + Sync { fn observe(&self, v: f64); }

pub trait Meter: Send + Sync + 'static {
    fn counter(&self, name: &'static str, labels: &'static [(&'static str,&'static str)]) -> Box<dyn Counter>;
    fn gauge(&self,   name: &'static str, labels: &'static [(&'static str,&'static str)]) -> Box<dyn Gauge>;
    fn histo(&self,   name: &'static str, _buckets:&'static [f64], labels:&'static [(&'static str,&'static str)]) -> Box<dyn Histogram>;
    fn emit(&self, evt: VizEvent);
}

/// A do-nothing meter for tests and users who don't care about telemetry.
#[derive(Clone, Default)]
pub struct NoopMeter;
struct NoopC; impl Counter for NoopC { fn inc(&self, _v: u64) {} }
struct NoopG; impl Gauge   for NoopG { fn set(&self, _v: i64) {} }
struct NoopH; impl Histogram for NoopH { fn observe(&self, _v: f64) {} }
impl Meter for NoopMeter {
    fn counter(&self, _n:&'static str, _l:&'static [(&'static str,&'static str)]) -> Box<dyn Counter> { Box::new(NoopC) }
    fn gauge(&self,   _n:&'static str, _l:&'static [(&'static str,&'static str)]) -> Box<dyn Gauge>   { Box::new(NoopG) }
    fn histo(&self,   _n:&'static str, _b:&'static [f64], _l:&'static [(&'static str,&'static str)]) -> Box<dyn Histogram> { Box::new(NoopH) }
    fn emit(&self, _e: VizEvent) {}
}

/// Typed events for live visualization (keys/values never included).
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum VizEvent {
    Wal(WalEvt),
    Compaction(CompEvt),
    Raft(RaftEvt),
    Swim(SwimEvt),
    Shard(ShardEvt),
    Cache(CacheEvt),
}

#[derive(Clone, Debug)]
pub struct WalEvt { pub node: u32, pub seg: u64, pub kind: WalKind }
#[derive(Clone, Debug)]
pub enum WalKind { SegmentRoll{bytes:u64}, Fsync{ms:u32}, CorruptionTruncated }

#[derive(Clone, Debug)]
pub struct CompEvt { pub node:u32, pub level:u8, pub kind: CompKind }
#[derive(Clone, Debug)]
pub enum CompKind { Scheduled, Start, Progress{pct:u8}, Finish{in_bytes:u64,out_bytes:u64} }

#[derive(Clone, Debug)]
pub struct RaftEvt { pub shard:u32, pub term:u64, pub kind: RaftKind }
#[derive(Clone, Debug)]
pub enum RaftKind { VoteReq{from:u32}, VoteGranted{from:u32}, LeaderElected{node:u32}, StepDown }

#[derive(Clone, Debug)]
pub struct SwimEvt { pub node:u32, pub kind: SwimKind }
#[derive(Clone, Debug)]
pub enum SwimKind { Alive, Suspect, Confirm, Leave }

#[derive(Clone, Debug)]
pub struct ShardEvt { pub shard:u32, pub kind: ShardKind }
#[derive(Clone, Debug)]
pub enum ShardKind { Plan, SnapshotStart, SnapshotDone, Cutover }

#[derive(Clone, Debug)]
pub struct CacheEvt { pub name:&'static str, pub hit_ratio:f32 }

/// Macros (simple versions). Can be feature-gated if desired.
#[macro_export]
macro_rules! obs_count {
    ($m:expr, $name:expr, $labels:expr, $v:expr) => {{
        $m.counter($name, $labels).inc($v as u64);
    }};
}
#[macro_export]
macro_rules! obs_gauge {
    ($m:expr, $name:expr, $labels:expr, $v:expr) => {{
        $m.gauge($name, $labels).set($v as i64);
    }};
}
#[macro_export]
macro_rules! obs_hist {
    ($m:expr, $name:expr, $labels:expr, $v:expr) => {{
        $m.histo($name, &[], $labels).observe($v as f64);
    }};
}
#[macro_export]
macro_rules! obs_timed {
    ($m:expr, $name:expr, $labels:expr, $body:block) => {{
        let __t = std::time::Instant::now();
        let __ret = { $body };
        let __ms = __t.elapsed().as_secs_f64() * 1000.0;
        $m.histo($name, &[], $labels).observe(__ms);
        __ret
    }};
}
