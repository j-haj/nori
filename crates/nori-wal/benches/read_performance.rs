use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nori_wal::{Position, Record, Wal, WalConfig};
use tempfile::TempDir;

fn sequential_read_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("sequential_read");
    group.sample_size(20);

    let record_counts = vec![100, 1000, 10000];
    let record_size = 1024; // 1KB records

    for record_count in record_counts {
        group.throughput(Throughput::Bytes(
            (record_count * record_size) as u64,
        ));

        group.bench_function(
            BenchmarkId::new("scan", record_count),
            |b| {
                b.to_async(tokio::runtime::Runtime::new().unwrap()).iter_custom(|iters| async move {
                    // Setup: create WAL and write records
                    let temp_dir = TempDir::new().unwrap();
                    let config = WalConfig {
                        dir: temp_dir.path().to_path_buf(),
                        ..Default::default()
                    };
                    let (wal, _) = Wal::open(config).await.unwrap();

                    // Write records
                    for i in 0..record_count {
                        let key = bytes::Bytes::from(format!("key{}", i));
                        let record =
                            Record::put(key, vec![0u8; record_size]);
                        wal.append(&record).await.unwrap();
                    }
                    wal.sync().await.unwrap();

                    // Benchmark: sequential scan
                    let start = std::time::Instant::now();
                    for _ in 0..iters {
                        let mut reader = wal
                            .read_from(Position {
                                segment_id: 0,
                                offset: 0,
                            })
                            .await
                            .unwrap();

                        let mut count = 0;
                        while let Some((record, _pos)) = reader.next_record().await.unwrap() {
                            black_box(&record);
                            count += 1;
                        }
                        black_box(count);
                    }
                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, sequential_read_benchmark);
criterion_main!(benches);
