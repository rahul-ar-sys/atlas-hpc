//! Criterion benchmarks for the Hot Tier.
//!
//! Run with: `cargo bench --package hot-tier`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use atlas_types::WeightedEntity;
use hot_tier::HotStore;

fn bench_reads(c: &mut Criterion) {
    let store = HotStore::with_capacity(100_000);
    for i in 0..10_000u64 {
        store.insert(WeightedEntity::new(i, i as f64));
    }

    let mut group = c.benchmark_group("hot_tier_read");
    group.throughput(Throughput::Elements(1));
    group.bench_function("single_get", |b| {
        b.iter(|| {
            // Access entity 0 — always present, tests pure read path.
            black_box(store.get(black_box(0)))
        })
    });
    group.bench_function("scatter_get_10k", |b| {
        let mut i: u64 = 0;
        b.iter(|| {
            let result = black_box(store.get(black_box(i % 10_000)));
            i = i.wrapping_add(1);
            result
        })
    });
    group.finish();
}

fn bench_writes(c: &mut Criterion) {
    let store = HotStore::with_capacity(100_000);
    for i in 0..10_000u64 {
        store.insert(WeightedEntity::new(i, i as f64));
    }

    let mut group = c.benchmark_group("hot_tier_write");
    group.throughput(Throughput::Elements(1));

    group.bench_function("update_weight", |b| {
        let mut i: u64 = 0;
        b.iter(|| {
            let result = store.update_weight(black_box(i % 10_000), 0.3, 0.9, 0.1);
            i = i.wrapping_add(1);
            black_box(result)
        })
    });

    group.bench_function("tick_price", |b| {
        let mut i: u64 = 0;
        b.iter(|| {
            store.tick_price(black_box(i % 10_000), black_box(123.45 + i as f64));
            i = i.wrapping_add(1);
        })
    });

    group.bench_function("insert_new", |b| {
        let mut id: u64 = 100_000;
        b.iter(|| {
            let e = WeightedEntity::new(black_box(id), black_box(id as f64));
            let result = store.insert(e);
            id += 1;
            black_box(result)
        })
    });
    group.finish();
}

fn bench_sustained_throughput(c: &mut Criterion) {
    let store = HotStore::with_capacity(1_000_000);
    for i in 0..1_000u64 {
        store.insert(WeightedEntity::new(i, i as f64));
    }

    let mut group = c.benchmark_group("hot_tier_sustained");
    for batch in [100usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(batch as u64));
        group.bench_with_input(
            BenchmarkId::new("mixed_read_write", batch),
            &batch,
            |b, &n| {
                b.iter(|| {
                    for i in 0..n as u64 {
                        if i % 3 == 0 {
                            black_box(store.update_weight(i % 1_000, 0.2, 0.9, 0.1));
                        } else {
                            black_box(store.get(i % 1_000));
                        }
                    }
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_reads, bench_writes, bench_sustained_throughput);
criterion_main!(benches);
