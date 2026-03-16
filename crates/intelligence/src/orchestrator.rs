//! Orchestrator — wires the Hot Tier, Warm Tier, and Intelligence layers.
//!
//! ```text
//! HotStore ──► SensingAgent (core 0) ──► [SignalEvent channel]
//!                                               │
//!                                       ReasoningAgent
//!                                      /           \
//!                              PrefixCache      CausalGraph
//!                                      \           /
//!                                    CacheInvalidator
//! ```

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use atlas_types::WeightedEntity;
use hot_tier::{HotStore, SensingAgent, SensingConfig};
use warm_tier::CausalGraph;

use crate::agent::ReasoningAgent;
use crate::cache::PrefixCache;

/// Full system orchestrator.
pub struct Orchestrator {
    pub store: HotStore,
    pub graph: CausalGraph,
    pub cache: Arc<PrefixCache>,
}

impl Orchestrator {
    /// Construct the orchestrator with all layers wired together.
    pub fn new(hot_cap: usize) -> Self {
        Self {
            store: HotStore::with_capacity(hot_cap),
            graph: CausalGraph::new(),
            cache: Arc::new(PrefixCache::new()),
        }
    }

    /// Seed the store with `n` synthetic entities for demo/testing.
    pub fn seed_entities(&self, n: usize) {
        for i in 0..n as u64 {
            let e = WeightedEntity::new(i, 100.0 + i as f64 * 0.01);
            self.store.insert(e);
        }
    }

    /// Start the full agent pipeline.  Returns a handle to the reasoning thread.
    ///
    /// The sensing agent is spawned on a daemon thread pinned to core 0 (Linux).
    pub fn start(self) -> thread::JoinHandle<()> {
        let config = SensingConfig {
            cpu_core: 0,
            idle_sleep: Duration::from_micros(200),
            ..Default::default()
        };
        let signal_rx = SensingAgent::spawn(self.store.clone(), config);

        let graph = self.graph.clone();
        let cache = Arc::clone(&self.cache);

        thread::Builder::new()
            .name("reasoning-agent".into())
            .spawn(move || {
                let agent = ReasoningAgent::new(signal_rx, graph, cache);
                agent.run();
            })
            .expect("failed to spawn ReasoningAgent thread")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_seeds_entities() {
        let o = Orchestrator::new(256);
        o.seed_entities(100);
        assert_eq!(o.store.len(), 100);
    }
}
