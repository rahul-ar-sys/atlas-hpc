//! Agent trait and ReasoningAgent implementation.

use std::sync::Arc;
use atlas_types::{LabelId, SignalEvent};
use crossbeam_channel::Receiver;
use tracing::{info, debug};
use warm_tier::CausalGraph;
use crate::cache::PrefixCache;

// ─────────────────────────────────────────────────────────────────────────────
// AgentTrait
// ─────────────────────────────────────────────────────────────────────────────

/// Context passed to an agent for each reasoning request.
#[derive(Clone, Debug)]
pub struct ReasoningContext {
    /// The signal that triggered this reasoning cycle.
    pub trigger: SignalEvent,
    /// Entities reached via spreading activation from the trigger entity.
    pub related_entities: Vec<u64>,
    /// Cache hit on this cycle.
    pub cache_hit: bool,
}

/// The result of one reasoning cycle.
#[derive(Clone, Debug)]
pub struct Insight {
    /// Entity this insight is about.
    pub entity_id: u64,
    /// Human-readable reasoning output (placeholder text in MVP).
    pub summary: String,
    /// Confidence score [0, 1].
    pub confidence: f32,
    /// Labels that contributed to this insight.
    pub contributing_labels: Vec<LabelId>,
}

/// Core reasoning interface.  In production this would call an LLM with
/// cached KV context.  In the MVP it generates a deterministic stub insight.
pub trait AgentTrait: Send + 'static {
    /// Process a reasoning context and produce an insight.
    fn reason(&self, ctx: &ReasoningContext) -> Insight;
}

// ─────────────────────────────────────────────────────────────────────────────
// ReasoningAgent
// ─────────────────────────────────────────────────────────────────────────────

/// The Reasoning Agent subscribes to the signal channel from the Hot Tier,
/// enriches context from the Causal Graph, checks the Prefix Cache, and
/// produces [`Insight`] records.
pub struct ReasoningAgent {
    signal_rx: Receiver<SignalEvent>,
    graph: CausalGraph,
    cache: Arc<PrefixCache>,
}

impl ReasoningAgent {
    /// Create a new reasoning agent.
    pub fn new(
        signal_rx: Receiver<SignalEvent>,
        graph: CausalGraph,
        cache: Arc<PrefixCache>,
    ) -> Self {
        Self { signal_rx, graph, cache }
    }

    /// Blocking run loop.  Call from a dedicated thread.
    pub fn run(self) {
        info!("ReasoningAgent started");
        for signal in &self.signal_rx {
            debug!("ReasoningAgent received {:?}", signal);
            let entity_id = signal.entity_id();

            // Filter out heartbeat sentinels from SensingAgent disconnect checks.
            if entity_id == u64::MAX {
                continue;
            }

            // Check prefix cache first (position-independent key).
            let cache_key = entity_id; // simplified: key = entity_id
            let cache_hit = self.cache.contains(cache_key);

            let related = if cache_hit {
                self.cache.get_related(cache_key).unwrap_or_default()
            } else {
                // Miss — run spreading activation.
                self.graph.spreading_activation(
                    LabelId::L_CRUDE_SHOCK, // use first known label as pivot
                    2,
                )
            };

            // Trigger TEMPORAL_LINK materialisation if it's a weight spike.
            if let SignalEvent::WeightSpike { delta_w, .. } = &signal {
                self.graph.materialize_temporal_link(entity_id, *delta_w);
            }

            let ctx = ReasoningContext {
                trigger: signal.clone(),
                related_entities: related.clone(),
                cache_hit,
            };

            let insight = self.produce_insight(&ctx);

            // Store result in cache.
            if !cache_hit {
                self.cache.insert(cache_key, related, insight.contributing_labels.clone());
            }

            info!(
                "Insight for entity {}: \"{}\" (confidence={:.2}, cache_hit={})",
                insight.entity_id, insight.summary, insight.confidence, cache_hit
            );
        }
        info!("ReasoningAgent: signal channel closed — shutting down");
    }

    fn produce_insight(&self, ctx: &ReasoningContext) -> Insight {
        let eid = ctx.trigger.entity_id();
        let confidence = if ctx.cache_hit { 0.92 } else { 0.75 };
        let summary = format!(
            "Entity {} triggered by {:?}. {} related entities discovered (cache_hit={}).",
            eid,
            match &ctx.trigger {
                SignalEvent::WeightSpike { delta_w, .. } => format!("WeightSpike(Δ={delta_w:.3})"),
                SignalEvent::CausalTrigger { .. } => "CausalTrigger".into(),
            },
            ctx.related_entities.len(),
            ctx.cache_hit,
        );
        Insight {
            entity_id: eid,
            summary,
            confidence,
            contributing_labels: vec![LabelId::L_CRUDE_SHOCK, LabelId::L_RATE_HIKE],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_context_can_be_built() {
        let trigger = SignalEvent::CausalTrigger { entity_id: 1, detected_at_ns: 0 };
        let ctx = ReasoningContext {
            trigger,
            related_entities: vec![2, 3, 4],
            cache_hit: false,
        };
        assert_eq!(ctx.related_entities.len(), 3);
    }
}
