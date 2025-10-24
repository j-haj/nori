use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nori_wal::{FsyncPolicy, Record, Wal, WalConfig};
use std::sync::Arc;
use tempfile::TempDir;

fn concurrent_writes_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_writes");
    group.sample_size(15); // Fewer samples for concurrent operations

    let thread_counts = vec![1, 2, 4, 8];
    let record_size = 1024; // 1KB records
    let writes_per_thread = 100;

    for thread_count in thread_counts {
        let total_writes = thread_count * writes_per_thread;
        group.throughput(Throughput::Bytes(
            (total_writes * record_size) as u64,
        ));

        group.bench_function(
            BenchmarkId::new("concurrent", thread_count),
            |b| {
                b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let config = WalConfig {
                        dir: temp_dir.path().to_path_buf(),
                        fsync_policy: FsyncPolicy::Os, // Fast for concurrency testing
                        ..Default::default()
                    };
                    let (wal, _) = Wal::open(config).await.unwrap();
                    let wal = Arc::new(wal);

                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let mut handles = vec![];

                        for thread_id in 0..thread_count {
                            let wal_clone = wal.clone();
                            let handle = tokio::spawn(async move {
                                for i in 0..writes_per_thread {
                                    let key = bytes::Bytes::from(format!("t{}_key{}", thread_id, i));
                                    let record = Record::put(key, vec![0u8; record_size]);
                                    black_box(wal_clone.append(&record).await.unwrap());
                                }
                            });
                            handles.push(handle);
                        }

                        for handle in handles {
                            handle.await.unwrap();
                        }

                        wal.sync().await.unwrap();
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, concurrent_writes_benchmark);
criterion_main!(benches);
