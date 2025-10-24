use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nori_wal::{FsyncPolicy, Record, Wal, WalConfig};
use std::time::Duration;
use tempfile::TempDir;

fn sequential_writes_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential_writes");

    // Test different record sizes
    let record_sizes = vec![
        ("100B", 100),
        ("1KB", 1024),
        ("10KB", 10 * 1024),
    ];

    // Test different fsync policies
    let policies = vec![
        ("os", FsyncPolicy::Os),
        ("batch_5ms", FsyncPolicy::Batch(Duration::from_millis(5))),
        ("always", FsyncPolicy::Always),
    ];

    for (size_name, size) in &record_sizes {
        for (policy_name, policy) in &policies {
            let bench_name = format!("{}_{}", policy_name, size_name);

            group.throughput(Throughput::Bytes(*size as u64));
            group.bench_function(
                BenchmarkId::new("append", &bench_name),
                |b| {
                    b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                        // Setup: create WAL
                        let temp_dir = TempDir::new().unwrap();
                        let config = WalConfig {
                            dir: temp_dir.path().to_path_buf(),
                            fsync_policy: *policy,
                            ..Default::default()
                        };
                        let (wal, _) = Wal::open(config).await.unwrap();
                        let value = vec![0u8; *size];

                        let start = std::time::Instant::now();
                        for _ in 0..iters {
                            let record = Record::put(b"benchmark_key".as_slice(), bytes::Bytes::from(value.clone()));
                            black_box(wal.append(&record).await.unwrap());
                        }
                        start.elapsed()
                    });
                },
            );
        }
    }

    group.finish();
}

fn batch_writes_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_writes");
    group.sample_size(20); // Fewer samples for batch operations

    // Test batches of different sizes
    let batch_sizes = vec![10, 100, 1000];
    let record_size = 1024; // 1KB records

    for batch_size in batch_sizes {
        group.throughput(Throughput::Bytes((batch_size * record_size) as u64));
        group.bench_function(
            BenchmarkId::new("append_batch", batch_size),
            |b| {
                b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let config = WalConfig {
                        dir: temp_dir.path().to_path_buf(),
                        fsync_policy: FsyncPolicy::Os, // Fast for batching
                        ..Default::default()
                    };
                    let (wal, _) = Wal::open(config).await.unwrap();
                    let records: Vec<_> = (0..batch_size)
                        .map(|i| {
                            Record::put(
                                bytes::Bytes::from(format!("key{}", i)),
                                vec![0u8; record_size],
                            )
                        })
                        .collect();

                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        for record in &records {
                            black_box(wal.append(record).await.unwrap());
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

criterion_group!(benches, sequential_writes_benchmark, batch_writes_benchmark);
criterion_main!(benches);
