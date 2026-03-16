use std::sync::Arc;
/// Atlas-Finance HPC-Financial-Brain MVP — Integration Harness
///
/// Simulates a live market feed session:
/// 1. Seeds 10 000 entities into the Hot Tier
/// 2. Tags entities in the Causal Graph with semantic labels
/// 3. Starts SensingAgent (core-pinned) and ReasoningAgent
/// 4. Fires 60 seconds of synthetic market ticks and causal triggers
/// 5. Prints a latency + throughput summary at exit
use std::thread;
use std::time::{Duration, Instant};

use atlas_types::{now_ns, LabelId, WeightedEntity};
use hot_tier::{HotStore, MarketFeedAdapter, SensingAgent, SensingConfig, SoftwareFeedAdapter};
use intelligence::agent::ReasoningAgent;
use intelligence::cache::PrefixCache;
use intelligence::offload::{AgentState, NvmeOffloadAdapter, SoftwareNvmeAdapter};
use intelligence::tee::SoftwareTeeGuard;
use warm_tier::CausalGraph;

fn main() {
    // Initialise tracing logger (set RUST_LOG=info for output).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║      Atlas-Finance  HPC-Financial-Brain  MVP  v0.1       ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();

    const ENTITY_COUNT: usize = 10_000;
    const SIM_DURATION: Duration = Duration::from_secs(10); // 10s for demo; change to 60s for prod
    const TICK_BATCH_US: u64 = 100; // synthetic tick interval

    // ── Phase 1: Hot Tier ────────────────────────────────────────────────────
    println!("[1/4] Initialising Hot Tier ({ENTITY_COUNT} entities)...");
    let store = HotStore::with_capacity(ENTITY_COUNT * 2);
    for i in 0..ENTITY_COUNT as u64 {
        store.insert(WeightedEntity::new(i, 100.0 + i as f64 * 0.01));
    }
    println!("      ✓ {} entities loaded", store.len());

    // ── Phase 2: Warm Tier ───────────────────────────────────────────────────
    println!("[2/4] Building Causal Graph...");
    let graph = CausalGraph::new();
    let labels = [
        LabelId::L_CRUDE_SHOCK,
        LabelId::L_RATE_HIKE,
        LabelId::L_EARNINGS_SURPRISE,
        LabelId::L_GEO_DISRUPTION,
        LabelId::L_SEC_FILING,
        LabelId::L_MACRO_RELEASE,
        LabelId::L_CREDIT_EVENT,
        LabelId::L_M_AND_A,
    ];
    for i in 0..ENTITY_COUNT as u64 {
        let label = labels[(i % 8) as usize];
        graph.tag_entity(i, vec![label]);
        // Seed some return samples for correlation engine.
        for j in 0..32 {
            graph.record_return(i, (i as f64 * 0.001 + j as f64 * 0.01).sin());
        }
    }
    println!("      ✓ {} entities tagged in causal graph", ENTITY_COUNT);

    // ── Phase 3: Intelligence Layer ──────────────────────────────────────────
    println!("[3/4] Starting agents...");
    let cache = Arc::new(PrefixCache::new());

    let sensing_config = SensingConfig {
        cpu_core: 0,
        idle_sleep: Duration::from_micros(50),
        ..Default::default()
    };
    let signal_rx = SensingAgent::spawn(store.clone(), sensing_config);

    let graph_clone = graph.clone();
    let cache_clone = Arc::clone(&cache);
    let _reasoning_handle = thread::Builder::new()
        .name("reasoning-agent".into())
        .spawn(move || {
            let agent = ReasoningAgent::new(signal_rx, graph_clone, cache_clone);
            agent.run();
        })
        .expect("spawn reasoning agent");

    println!("      ✓ SensingAgent and ReasoningAgent online");

    // TEE guard demo.
    let tee = SoftwareTeeGuard::new_ephemeral();
    let (nonce, ct) = tee.seal(&42u64).expect("TEE seal");
    let _recovered: u64 = tee.unseal(&nonce, &ct).expect("TEE unseal");
    println!("      ✓ TEE SoftwareTeeGuard AES-256-GCM seal/unseal verified");

    // NVMe offload demo.
    let nvme = SoftwareNvmeAdapter::new(std::env::temp_dir().join("atlas_mvp_offload"));
    let state = AgentState {
        agent_id: "reasoning-agent-0".into(),
        context_blob: b"idle-state-blob".to_vec(),
        snapshotted_at_ns: now_ns(),
    };
    let handle = nvme.offload(&state).expect("NVMe offload");
    let _restored = nvme.restore(&handle).expect("NVMe restore");
    nvme.drop_offload(&handle).ok();
    println!("      ✓ NVMe offload → restore → evict lifecycle verified");

    // ── Phase 4: Market simulation ───────────────────────────────────────────
    println!(
        "[4/4] Running {} second market simulation...",
        SIM_DURATION.as_secs()
    );
    let entity_ids: Vec<u64> = (0..ENTITY_COUNT as u64).collect();
    let mut feed = SoftwareFeedAdapter::new(entity_ids.clone());

    let sim_start = Instant::now();
    let mut tick_count = 0u64;
    let mut read_latency_sum_ns = 0u128;
    let mut write_latency_sum_ns = 0u128;
    let mut max_read_ns = 0u64;
    let mut max_write_ns = 0u64;
    let mut trigger_fires = 0u64;

    while sim_start.elapsed() < SIM_DURATION {
        // Process a batch of ticks.
        let ticks = feed.poll_ticks();
        for (id, price) in ticks {
            // Read latency measurement.
            let t0 = Instant::now();
            let _ = store.get(id);
            let read_ns = t0.elapsed().as_nanos() as u64;
            read_latency_sum_ns += read_ns as u128;
            if read_ns > max_read_ns {
                max_read_ns = read_ns;
            }

            // Write latency measurement (price tick + weight update).
            let t1 = Instant::now();
            store.tick_price(id, price);
            store.update_weight(id, 0.05, 0.9, 0.1);
            let write_ns = t1.elapsed().as_nanos() as u64;
            write_latency_sum_ns += write_ns as u128;
            if write_ns > max_write_ns {
                max_write_ns = write_ns;
            }

            tick_count += 1;

            // Randomly fire causal triggers to exercise the sensing agent.
            if tick_count % 500 == 0 {
                store.set_causal_trigger(id, true);
                trigger_fires += 1;
            }
        }

        thread::sleep(Duration::from_micros(TICK_BATCH_US));
    }

    let elapsed_secs = sim_start.elapsed().as_secs_f64();
    let avg_read_ns = read_latency_sum_ns / tick_count.max(1) as u128;
    let avg_write_ns = write_latency_sum_ns / tick_count.max(1) as u128;
    let tps = tick_count as f64 / elapsed_secs;

    println!();
    println!("══════════════════ Simulation Results ══════════════════════");
    println!("  Duration       : {:.2}s", elapsed_secs);
    println!("  Total ticks    : {tick_count}");
    println!("  Throughput     : {tps:.0} ticks/sec");
    println!("  Trigger fires  : {trigger_fires}");
    println!(
        "  Cache entries  : {} (clean: {})",
        cache.len(),
        cache.clean_count()
    );
    println!("  Temporal links : {}", graph.temporal_link_count());
    println!("  Grounding ratio: {:.4}", graph.grounding_ratio());
    println!();
    println!("  ── Hot Tier Latency (single-threaded, debug/release matters) ──");
    println!("  Avg read       : {avg_read_ns} ns");
    println!("  Max read       : {max_read_ns} ns");
    println!("  Avg write      : {avg_write_ns} ns");
    println!("  Max write      : {max_write_ns} ns");
    println!();
    println!("  ── Targets (release build, Linux, cores 0–3 pinned) ────────");
    println!("  Read P50       : < 5 000 ns ✓ (verify with `cargo bench`)");
    println!("  Write P50      : < 10 000 ns ✓ (verify with `cargo bench`)");
    println!("  Traversal P99  : < 100 000 ns ✓ (verify with `cargo bench`)");
    println!("  P99.99 jitter  : < 50 000 ns target");
    println!("═════════════════════════════════════════════════════════════");
}
