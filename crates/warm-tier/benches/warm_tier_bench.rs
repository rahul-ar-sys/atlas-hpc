//! Criterion benchmarks for the Warm Tier causal graph.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use atlas_types::LabelId;
use warm_tier::CausalGraph;

fn seed_graph(g: &CausalGraph, entity_count: usize) {
    for i in 0..entity_count as u64 {
        let label = LabelId(1 + (i % 8) as u32);
        g.tag_entity(i, vec![label]);
        // Add some return samples for correlation materialisation tests.
        for j in 0..32 {
            g.record_return(i, (i as f64 + j as f64 * 0.01).sin());
        }
    }
}

fn bench_spreading_activation(c: &mut Criterion) {
    let mut group = c.benchmark_group("warm_tier_spreading_activation");

    for n in [1_000usize, 10_000, 100_000] {
        let g = CausalGraph::new();
        seed_graph(&g, n);

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("entities", n),
            &n,
            |b, _| {
                b.iter(|| {
                    black_box(g.spreading_activation(black_box(LabelId::L_CRUDE_SHOCK), 3))
                })
            },
        );
    }
    group.finish();
}

fn bench_tag_entity(c: &mut Criterion) {
    let g = CausalGraph::new();
    let mut group = c.benchmark_group("warm_tier_ingest");
    group.throughput(Throughput::Elements(1));
    group.bench_function("tag_entity", |b| {
        let mut id: u64 = 0;
        b.iter(|| {
            g.tag_entity(black_box(id), vec![LabelId::L_SEC_FILING]);
            id += 1;
        })
    });
    group.finish();
}

fn bench_materialize_link(c: &mut Criterion) {
    let g = CausalGraph::new();
    seed_graph(&g, 100);

    let mut group = c.benchmark_group("warm_tier_temporal_link");
    group.bench_function("materialize_100_entities", |b| {
        b.iter(|| {
            g.materialize_temporal_link(black_box(0), black_box(0.25))
        })
    });
    group.finish();
}

criterion_group!(benches, bench_spreading_activation, bench_tag_entity, bench_materialize_link);
criterion_main!(benches);
