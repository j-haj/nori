use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nori_wal::{Record, Wal, WalConfig};
use tempfile::TempDir;

fn recovery_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("recovery");
    group.sample_size(15);

    let record_counts = vec![1000, 10000, 50000];
    let record_size = 1024; // 1KB records

    for record_count in record_counts {
        group.throughput(Throughput::Bytes(
            (record_count * record_size) as u64,
        ));

        group.bench_function(
            BenchmarkId::new("clean_recovery", record_count),
            |b| {
                b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                    // Setup: create WAL, write records, then close
                    let temp_dir = TempDir::new().unwrap();
                    let config = WalConfig {
                        dir: temp_dir.path().to_path_buf(),
                        ..Default::default()
                    };

                    let (wal, _) = Wal::open(config.clone()).await.unwrap();

                    // Write records
                    for i in 0..record_count {
                        let key = bytes::Bytes::from(format!("key{}", i));
                        let record =
                            Record::put(key, vec![0u8; record_size]);
                        wal.append(&record).await.unwrap();
                    }
                    wal.sync().await.unwrap();
                    drop(wal); // Close the WAL

                    // Benchmark: recovery on reopen
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let (wal, recovery_info) = Wal::open(config.clone()).await.unwrap();
                        black_box(wal);
                        black_box(recovery_info);
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

fn multi_segment_recovery_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_segment_recovery");
    group.sample_size(10);

    // Force multiple segments by using small segment size
    let segment_sizes = vec![
        ("2_segments", 1024 * 1024),      // 1 MB
        ("5_segments", 512 * 1024),       // 512 KB
    ];
    let record_size = 1024;
    let total_records = 5000;

    for (name, segment_size) in segment_sizes {
        group.throughput(Throughput::Bytes(
            (total_records * record_size) as u64,
        ));

        group.bench_function(
            BenchmarkId::new("recovery", name),
            |b| {
                b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let config = WalConfig {
                        dir: temp_dir.path().to_path_buf(),
                        max_segment_size: segment_size,
                        ..Default::default()
                    };

                    let (wal, _) = Wal::open(config.clone()).await.unwrap();

                    // Write enough records to span multiple segments
                    for i in 0..total_records {
                        let key = bytes::Bytes::from(format!("key{}", i));
                        let record =
                            Record::put(key, vec![0u8; record_size]);
                        wal.append(&record).await.unwrap();
                    }
                    wal.sync().await.unwrap();
                    drop(wal);

                    // Benchmark: recovery
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let (wal, recovery_info) = Wal::open(config.clone()).await.unwrap();
                        black_box(wal);
                        black_box(recovery_info);
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    recovery_benchmark,
    multi_segment_recovery_benchmark
);
criterion_main!(benches);
