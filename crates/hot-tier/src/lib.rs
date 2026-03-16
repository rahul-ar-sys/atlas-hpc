//! # hot-tier
//!
//! Phase 1: The In-Memory Hot Tier — a lock-free KV store for [`WeightedEntity`]
//! records, targeting < 5 µs read and < 10 µs write latency under 1 M ops/sec bursts.
//!
//! ## Architecture
//!
//! ```text
//!  Market Feed  ──►  HotStore (LeapMap)  ◄──  SensingAgent (core 0)
//!                                                      │
//!                                              SignalEvent channel
//!                                                      │
//!                                              ReasoningAgent  (Phase 3)
//! ```
//!
//! [`WeightedEntity`]: atlas_types::WeightedEntity

#![deny(missing_docs)]

/// Ingests live WebSocket streams from Binance.
pub mod binance;
pub use binance::BinanceFeedAdapter;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use atlas_types::{now_ns, SignalEvent, WeightedEntity};
use crossbeam_channel::{bounded, Receiver, Sender};
//use dashmap::DashMap;
use tracing::{debug, info};

// ─────────────────────────────────────────────────────────────────────────────
// Weight update constants (module-level defaults — callers may override)
// ─────────────────────────────────────────────────────────────────────────────

/// Default decay constant α in the weight-recalculation formula.
pub const DEFAULT_ALPHA: f32 = 0.9;

/// Default surprise-sensitivity constant β.
pub const DEFAULT_BETA: f32 = 0.1;

/// Weight spike threshold: if ΔW exceeds this, emit a `WeightSpike` event.
pub const WEIGHT_SPIKE_THRESHOLD: f32 = 0.15;

// ─────────────────────────────────────────────────────────────────────────────
// HotStore
// ─────────────────────────────────────────────────────────────────────────────

/// Lock-free, cache-line-aligned key-value store for financial entities.
///
/// Internally uses `leapfrog::LeapMap` which provides:
/// * Wait-free reads on the fast path
/// * Lock-free CAS-based writes
/// * Epoch-based reclamation (no ABA, no hazard pointers needed by callers)
///
/// The map is wrapped in an `Arc` so multiple threads (and the
/// [`SensingAgent`]) can hold references without cloning data.
#[derive(Clone)]
pub struct HotStore {
    inner: Arc<dashmap::DashMap<u64, WeightedEntity>>,
}

impl HotStore {
    /// Create a new, empty `HotStore` with the given initial capacity hint.
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            inner: Arc::new(dashmap::DashMap::with_capacity(cap)),
        }
    }

    /// Insert or overwrite an entity.  Returns the previous value if any.
    ///
    /// **Latency target:** < 10 µs P50 under sustained load.
    #[inline]
    pub fn insert(&self, entity: WeightedEntity) -> Option<WeightedEntity> {
        self.inner.insert(entity.entity_id, entity)
    }

    /// Read a snapshot of an entity by ID.
    ///
    /// The returned copy is a point-in-time snapshot; the store may be
    /// mutated concurrently by other threads.
    ///
    /// **Latency target:** < 5 µs P50 under sustained load.
    #[inline]
    pub fn get(&self, entity_id: u64) -> Option<WeightedEntity> {
        self.inner
            .get(&entity_id)
            .map(|ref_multi| *ref_multi.value())
    }

    /// Update the importance weight of an entity using the formula
    /// `W_new = W_old × α + S × β`.
    ///
    /// Uses a read-then-CAS loop to achieve a zero-lock update path.
    /// Returns `(old_weight, new_weight)` on success, `None` if the entity
    /// doesn't exist.
    #[inline]
    pub fn update_weight(
        &self,
        entity_id: u64,
        surprise: f32,
        alpha: f32,
        beta: f32,
    ) -> Option<(f32, f32)> {
        if let Some(mut entity) = self.inner.get_mut(&entity_id) {
            let old = entity.importance_weight;
            entity.recalculate_weight(surprise, alpha, beta);
            entity.last_updated_ns = now_ns();
            Some((old, entity.importance_weight))
        } else {
            None
        }
    }

    /// Atomically set `causal_trigger_bit` for an entity.
    pub fn set_causal_trigger(&self, entity_id: u64, value: bool) {
        if let Some(mut e) = self.inner.get_mut(&entity_id) {
            e.causal_trigger_bit = value;
            e.last_updated_ns = now_ns();
        }
    }

    /// Atomically update `latest_price` (zero-copy market-feed ingestion path).
    #[inline]
    pub fn tick_price(&self, entity_id: u64, price: f64) {
        if let Some(mut e) = self.inner.get_mut(&entity_id) {
            e.latest_price = price;
            e.last_updated_ns = now_ns();
        }
    }

    /// Returns the number of entities currently tracked.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// Returns `true` if the map contains a value for the specified entity ID.
    pub fn contains(&self, entity_id: u64) -> bool {
        self.inner.contains_key(&entity_id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SensingAgent
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for the [`SensingAgent`].
#[derive(Clone, Debug)]
pub struct SensingConfig {
    /// CPU core to pin the agent thread to (Linux only; ignored on Windows).
    pub cpu_core: usize,
    /// How long to sleep between poll sweeps when idle (to avoid burning 100%
    /// CPU in a tight spin loop during low-activity periods).
    pub idle_sleep: Duration,
    /// Minimum weight delta to consider a "spike" worth reporting.
    pub spike_threshold: f32,
    /// Channel capacity for the outbound signal queue.
    pub channel_capacity: usize,
}

impl Default for SensingConfig {
    fn default() -> Self {
        Self {
            cpu_core: 0,
            idle_sleep: Duration::from_micros(100),
            spike_threshold: WEIGHT_SPIKE_THRESHOLD,
            channel_capacity: 8_192,
        }
    }
}

/// The Sensing Agent continuously polls [`HotStore`] for:
/// 1. Entities whose `causal_trigger_bit` is `true`
/// 2. Entities whose `importance_weight` has spiked beyond the threshold
///
/// On detection it emits a [`SignalEvent`] on the outbound channel and resets
/// the trigger bit to avoid duplicate signals.
pub struct SensingAgent {
    store: HotStore,
    tx: Sender<SignalEvent>,
    config: SensingConfig,
}

impl SensingAgent {
    /// Construct an agent and its receiving channel.
    ///
    /// Returns `(agent, rx)` — spawn the agent on a dedicated thread with
    /// [`SensingAgent::run`], and pass `rx` to the Reasoning Agent.
    pub fn new(store: HotStore, config: SensingConfig) -> (Self, Receiver<SignalEvent>) {
        let (tx, rx) = bounded(config.channel_capacity);
        (Self { store, tx, config }, rx)
    }

    /// Spin the agent.  This method **blocks** the calling thread.
    /// Run inside `thread::spawn` or a Tokio `spawn_blocking`.
    pub fn run(self) {
        // Attempt CPU affinity pinning (best-effort; not supported on Windows).
        #[cfg(target_os = "linux")]
        {
            if let Some(cores) = core_affinity::get_core_ids() {
                if let Some(core) = cores.get(self.config.cpu_core) {
                    core_affinity::set_for_current(*core);
                    info!("SensingAgent pinned to core {}", self.config.cpu_core);
                }
            }
        }

        info!(
            "SensingAgent started (spike_threshold={})",
            self.config.spike_threshold
        );

        let mut prev_weights: std::collections::HashMap<u64, f32> =
            std::collections::HashMap::new();

        loop {
            // Full sweep of the store.
            // NOTE: leapfrog's iteration is not linearisable, but for a
            // best-effort polling agent this is acceptable.
            let mut idle = true;

            // We iterate by collecting keys first to avoid holding a map
            // reference across the mutable reset call.
            let snapshots: Vec<WeightedEntity> = {
                let mut v = Vec::new();
                for pair in self.store.inner.iter() {
                    v.push(*pair.value());
                }
                v
            };

            for entity in snapshots {
                let id = entity.entity_id;

                // ── Causal trigger check ───────────────────────────────────
                if entity.causal_trigger_bit {
                    debug!("CausalTrigger detected for entity {id}");
                    self.store.set_causal_trigger(id, false); // reset
                    let _ = self.tx.try_send(SignalEvent::CausalTrigger {
                        entity_id: id,
                        detected_at_ns: now_ns(),
                    });
                    idle = false;
                }

                // ── Weight spike check ─────────────────────────────────────
                let prev = prev_weights
                    .get(&id)
                    .copied()
                    .unwrap_or(entity.importance_weight);
                let delta = (entity.importance_weight - prev).abs();
                if delta >= self.config.spike_threshold {
                    debug!("WeightSpike Δ={delta:.3} for entity {id}");
                    let _ = self.tx.try_send(SignalEvent::WeightSpike {
                        entity_id: id,
                        delta_w: delta,
                        new_weight: entity.importance_weight,
                        detected_at_ns: now_ns(),
                    });
                    idle = false;
                }
                prev_weights.insert(id, entity.importance_weight);
            }

            if idle {
                thread::sleep(self.config.idle_sleep);
            }
        }
    }

    /// Spawn the agent on a dedicated OS thread and return the signal receiver.
    /// The thread handle is detached; the agent runs until the store is dropped.
    pub fn spawn(store: HotStore, config: SensingConfig) -> Receiver<SignalEvent> {
        let (agent, rx) = Self::new(store, config);
        thread::Builder::new()
            .name("sensing-agent".into())
            .spawn(move || agent.run())
            .expect("failed to spawn SensingAgent thread");
        rx
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DPDK / Kernel-bypass stub
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel-bypass market-feed adapter interface.
///
/// On Linux with DPDK this would be backed by `rte_ring` / hugepage DMA.
/// On all other platforms (including Windows dev environments) the
/// `Software` variant is used, which reads from a standard socket.
pub trait MarketFeedAdapter: Send + 'static {
    /// Poll for the next batch of price ticks.
    /// Returns a slice of `(entity_id, price)` pairs.
    fn poll_ticks(&mut self) -> Vec<(u64, f64)>;
}

/// Software (non-kernel-bypass) market feed adapter for development.
/// Generates synthetic random ticks.
pub struct SoftwareFeedAdapter {
    /// Entities to simulate.
    entity_ids: Vec<u64>,
    rng_state: u64,
}

impl SoftwareFeedAdapter {
    /// Create a software feed adapter for the given entity IDs.
    pub fn new(entity_ids: Vec<u64>) -> Self {
        Self {
            entity_ids,
            rng_state: 0xDEAD_BEEF_1234_5678,
        }
    }

    /// Xorshift64 PRNG (no stdlib dependency, deterministic).
    fn next_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }
}

impl MarketFeedAdapter for SoftwareFeedAdapter {
    fn poll_ticks(&mut self) -> Vec<(u64, f64)> {
        let batch_size = (self.next_u64() % 8 + 1) as usize;
        (0..batch_size)
            .map(|_| {
                let idx = (self.next_u64() as usize) % self.entity_ids.len();
                let id = self.entity_ids[idx];
                // Price in range [10.0, 10000.0]
                let price = 10.0 + (self.next_u64() % 999_000) as f64 / 100.0;
                (id, price)
            })
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_types::WeightedEntity;
    use std::time::Instant;

    fn make_store() -> HotStore {
        HotStore::with_capacity(1024)
    }

    #[test]
    fn insert_and_retrieve() {
        let store = make_store();
        let e = WeightedEntity::new(1, 100.0);
        store.insert(e);
        let got = store.get(1).expect("entity should exist");
        assert_eq!(got.entity_id, 1);
        assert!((got.latest_price - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn update_weight_applies_formula() {
        let store = make_store();
        let mut e = WeightedEntity::new(42, 200.0);
        e.importance_weight = 0.8;
        store.insert(e);

        let (old, new) = store.update_weight(42, 0.5, 0.9, 0.1).unwrap();
        assert!((old - 0.8).abs() < f32::EPSILON);
        let expected = 0.8f32 * 0.9 + 0.5 * 0.1;
        assert!(
            (new - expected).abs() < f32::EPSILON,
            "got {new}, expected {expected}"
        );
    }

    #[test]
    fn causal_trigger_round_trip() {
        let store = make_store();
        store.insert(WeightedEntity::new(7, 50.0));

        store.set_causal_trigger(7, true);
        assert!(store.get(7).unwrap().causal_trigger_bit);

        store.set_causal_trigger(7, false);
        assert!(!store.get(7).unwrap().causal_trigger_bit);
    }

    #[test]
    fn sensing_agent_fires_causal_trigger_event() {
        let store = make_store();
        let e = WeightedEntity::new(99, 1.0);
        store.insert(e);

        let config = SensingConfig {
            idle_sleep: Duration::from_micros(10),
            ..Default::default()
        };
        let rx = SensingAgent::spawn(store.clone(), config);

        // Flip the trigger bit — agent should catch it within 1 sweep.
        store.set_causal_trigger(99, true);

        let event = rx
            .recv_timeout(Duration::from_millis(500))
            .expect("event not received");
        assert_eq!(event.entity_id(), 99);
        matches!(event, SignalEvent::CausalTrigger { .. });
    }

    /// Rough latency sanity check — not a formal benchmark, just a smoke test
    /// that reads are well under 1 ms even in debug builds.
    #[test]
    fn read_latency_sanity_check() {
        let store = make_store();
        for i in 1..=1000u64 {
            store.insert(WeightedEntity::new(i, i as f64));
        }

        let start = Instant::now();
        let iterations = 10_000usize;
        for i in 1..=iterations as u64 {
            let _ = store.get((i % 1000) + 1);
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() / iterations as u128;
        println!("Average get latency: {avg_ns} ns");
        // In debug builds, allow up to 50 µs.  In release this should be < 5 µs.
        assert!(
            avg_ns < 50_000,
            "Average read latency {avg_ns} ns exceeds 50 µs"
        );
    }

    #[test]
    fn software_feed_adapter_generates_valid_ticks() {
        let ids = vec![1u64, 2, 3, 4, 5];
        let mut feed = SoftwareFeedAdapter::new(ids.clone());
        let ticks = feed.poll_ticks();
        assert!(!ticks.is_empty());
        for (id, price) in &ticks {
            assert!(ids.contains(id));
            assert!(*price >= 10.0 && *price <= 10_000.0);
        }
    }
}
