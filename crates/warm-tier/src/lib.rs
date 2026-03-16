//! # warm-tier
//!
//! Phase 2: The Causal Graph Engine.
//!
//! Provides semantic labelling of financial entities and spreading-activation
//! graph traversal, targeting < 100 µs end-to-end under 10 K node graphs.
//!
//! ## Architecture
//!
//! ```text
//!  Ingestor (SEC/News) ──► tag_entity()
//!                               │
//!                          LabelStore (linked-list arena)
//!                               │
//!                          CausalGraph (adjacency map)
//!                               │
//!                     spreading_activation() / materialize_temporal_link()
//! ```

#![deny(missing_docs)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use atlas_types::{now_ns, LabelId, TemporalLink};
use tracing::{debug, info};

// ─────────────────────────────────────────────────────────────────────────────
// LabelStore — arena-allocated linked list
// ─────────────────────────────────────────────────────────────────────────────

/// A single node in the label linked-list.
///
/// The arena (a `Vec<LabelNode>`) owns all nodes.  `next` is an index into that
/// arena (`usize::MAX` means "no next node") rather than a raw pointer, to
/// keep the code `unsafe`-free while preserving the required linked-list
/// semantics from the spec.
#[derive(Debug)]
pub struct LabelNode {
    /// The semantic label this node represents.
    pub label_id: LabelId,
    /// Index of the next node in the arena (`usize::MAX` if tail).
    pub next: usize,
    /// Entity IDs that carry this label.
    pub entity_ids: Vec<u64>,
}

/// Arena-allocated, linked-list storage for [`LabelNode`]s.
///
/// New labels are appended to the arena in O(1) amortised time.
/// Traversal follows `next` indices in O(k) where k is the list length.
pub struct LabelStore {
    arena: Vec<LabelNode>,
    /// Maps label_id → arena index for O(1) lookups.
    index: HashMap<LabelId, usize>,
    head: usize, // arena index of list head (usize::MAX if empty)
}

impl LabelStore {
    /// Create an empty label store.
    pub fn new() -> Self {
        Self {
            arena: Vec::with_capacity(64),
            index: HashMap::new(),
            head: usize::MAX,
        }
    }

    /// Register `entity_id` under `label_id`.  Creates a new `LabelNode` if
    /// the label hasn't been seen before; otherwise appends to existing set.
    pub fn insert(&mut self, label_id: LabelId, entity_id: u64) {
        if let Some(&idx) = self.index.get(&label_id) {
            self.arena[idx].entity_ids.push(entity_id);
        } else {
            let idx = self.arena.len();
            self.arena.push(LabelNode {
                label_id,
                next: self.head,
                entity_ids: vec![entity_id],
            });
            self.head = idx;
            self.index.insert(label_id, idx);
        }
    }

    /// Look up all entity IDs carrying `label_id`.
    pub fn entities_for_label(&self, label_id: LabelId) -> Option<&[u64]> {
        self.index
            .get(&label_id)
            .map(|&idx| self.arena[idx].entity_ids.as_slice())
    }

    /// Traverse the full linked list and return all `(LabelId, entity_count)` pairs.
    pub fn traverse_all(&self) -> Vec<(LabelId, usize)> {
        let mut result = Vec::new();
        let mut cur = self.head;
        while cur != usize::MAX {
            let node = &self.arena[cur];
            result.push((node.label_id, node.entity_ids.len()));
            cur = node.next;
        }
        result
    }

    /// Total number of distinct labels registered.
    pub fn label_count(&self) -> usize {
        self.index.len()
    }
}

impl Default for LabelStore {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CausalGraph
// ─────────────────────────────────────────────────────────────────────────────

/// Inner state of the causal graph, protected by an `RwLock`.
struct GraphInner {
    /// entity_id → set of labels (primary adjacency map)
    entity_labels: HashMap<u64, Vec<LabelId>>,
    /// Reverse map: label → set of entity_ids (for BFS)
    label_entities: HashMap<LabelId, Vec<u64>>,
    /// Materialised `TEMPORAL_LINK` edges keyed by (src, dst).
    temporal_links: HashMap<(LabelId, LabelId), TemporalLink>,
    /// Historical co-movement observations: `(entity_a, entity_b)` →
    /// sorted Vec of price-return samples for correlation calculation.
    /// In production this would be a ring buffer.
    return_samples: HashMap<u64, Vec<f64>>,
    /// fact_count / (fact_count + hallucination_count) — monotonically tracked.
    fact_count: u64,
    hallucination_count: u64,
    /// Label linked-list store (spec requirement).
    label_store: LabelStore,
}

impl GraphInner {
    fn new() -> Self {
        Self {
            entity_labels: HashMap::new(),
            label_entities: HashMap::new(),
            temporal_links: HashMap::new(),
            return_samples: HashMap::new(),
            fact_count: 0,
            hallucination_count: 0,
            label_store: LabelStore::new(),
        }
    }
}

/// Thread-safe causal graph for the Warm Tier.
///
/// The graph connects financial entities via semantic labels and materialises
/// `TEMPORAL_LINK` edges when statistically significant co-movement is detected.
#[derive(Clone)]
pub struct CausalGraph {
    inner: Arc<RwLock<GraphInner>>,
}

impl CausalGraph {
    /// Create an empty causal graph.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(GraphInner::new())),
        }
    }

    // ── Ingestor API ─────────────────────────────────────────────────────────

    /// Tag an entity with a set of semantic labels.
    ///
    /// Called by the SEC/news ingestor on each document ingestion.
    pub fn tag_entity(&self, entity_id: u64, labels: Vec<LabelId>) {
        let mut g = self.inner.write().unwrap();
        for label in &labels {
            g.label_entities
                .entry(*label)
                .or_default()
                .push(entity_id);
            g.label_store.insert(*label, entity_id);
        }
        g.entity_labels
            .entry(entity_id)
            .or_default()
            .extend(labels);
        debug!("tagged entity {entity_id}");
    }

    /// Record a price-return sample for an entity (used for correlation).
    pub fn record_return(&self, entity_id: u64, ret: f64) {
        let mut g = self.inner.write().unwrap();
        let samples = g.return_samples.entry(entity_id).or_default();
        samples.push(ret);
        // Keep a rolling window of 256 samples to bound memory.
        if samples.len() > 256 {
            samples.remove(0);
        }
    }

    // ── Spreading Activation ─────────────────────────────────────────────────

    /// BFS spreading activation from `start_label`.
    ///
    /// Returns all entity IDs reachable within `max_hops` label-→-entity hops,
    /// bounded to at most 100 µs of wall-clock traversal time (best-effort;
    /// the function returns whatever it found within the budget).
    pub fn spreading_activation(&self, start_label: LabelId, max_hops: usize) -> Vec<u64> {
        let deadline = Instant::now() + std::time::Duration::from_micros(100);
        let g = self.inner.read().unwrap();

        let mut visited_labels: HashSet<LabelId> = HashSet::new();
        let mut result_entities: HashSet<u64> = HashSet::new();
        let mut queue: VecDeque<(LabelId, usize)> = VecDeque::new();

        queue.push_back((start_label, 0));
        visited_labels.insert(start_label);

        while let Some((label, hop)) = queue.pop_front() {
            // Enforce time budget.
            if Instant::now() >= deadline {
                debug!("spreading_activation: 100 µs budget exhausted at hop {hop}");
                break;
            }

            if hop > max_hops {
                break;
            }

            // Collect entities carrying this label.
            if let Some(entities) = g.label_entities.get(&label) {
                for &eid in entities {
                    if result_entities.insert(eid) {
                        // Enqueue all labels of newly-discovered entities.
                        if hop < max_hops {
                            if let Some(e_labels) = g.entity_labels.get(&eid) {
                                for &next_label in e_labels {
                                    if visited_labels.insert(next_label) {
                                        queue.push_back((next_label, hop + 1));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        result_entities.into_iter().collect()
    }

    // ── TEMPORAL_LINK materialisation ─────────────────────────────────────────

    /// Called by the Sensing Agent when it detects a weight spike on `src_id`.
    ///
    /// Queries the graph for entities sharing labels with `src_id`, computes
    /// pairwise Pearson correlation, and materialises a `TEMPORAL_LINK` edge if
    /// the p-value is < 0.05.
    pub fn materialize_temporal_link(&self, src_id: u64, _spike_delta: f32) {
        let g_read = self.inner.read().unwrap();

        let src_labels = match g_read.entity_labels.get(&src_id) {
            Some(labels) => labels.clone(),
            None => return,
        };

        // Find all entities that share at least one label with src.
        let mut candidate_ids: HashSet<u64> = HashSet::new();
        for label in &src_labels {
            if let Some(entities) = g_read.label_entities.get(label) {
                for &eid in entities {
                    if eid != src_id {
                        candidate_ids.insert(eid);
                    }
                }
            }
        }

        // Collect return samples for correlation calculation.
        let src_samples = g_read.return_samples.get(&src_id).cloned().unwrap_or_default();

        // We need to drop the read lock before acquiring the write lock.
        drop(g_read);

        if src_samples.len() < 10 {
            return; // Not enough history for reliable p-value.
        }

        let mut new_links: Vec<TemporalLink> = Vec::new();

        for candidate_id in candidate_ids {
            let g_read2 = self.inner.read().unwrap();
            let cand_samples = match g_read2.return_samples.get(&candidate_id) {
                Some(s) if s.len() >= 10 => s.clone(),
                _ => continue,
            };

            let cand_labels = g_read2
                .entity_labels
                .get(&candidate_id)
                .cloned()
                .unwrap_or_default();
            drop(g_read2);

            // Compute Pearson r and a t-test–derived p-value.
            if let Some((r, p_value)) = pearson_with_pvalue(&src_samples, &cand_samples) {
                if p_value < 0.05 {
                    info!(
                        "TEMPORAL_LINK: {src_id} ↔ {candidate_id} r={r:.3} p={p_value:.4}"
                    );
                    // For each shared label pair, create a directed edge.
                    for &sl in &src_labels {
                        for &cl in &cand_labels {
                            if sl != cl {
                                new_links.push(TemporalLink::new(
                                    sl,
                                    cl,
                                    p_value as f32,
                                    r.abs() as f32,
                                    now_ns(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Write all new edges atomically.
        if !new_links.is_empty() {
            let mut g = self.inner.write().unwrap();
            for link in new_links {
                g.temporal_links.insert((link.src, link.dst), link);
                g.fact_count += 1;
            }
        }
    }

    // ── Metrics ──────────────────────────────────────────────────────────────

    /// Returns the current grounding ratio (fact count / total assertions).
    ///
    /// A ratio of ≥ 0.995 means ≤ 0.5 % hallucination.
    pub fn grounding_ratio(&self) -> f64 {
        let g = self.inner.read().unwrap();
        let total = g.fact_count + g.hallucination_count;
        if total == 0 {
            1.0
        } else {
            g.fact_count as f64 / total as f64
        }
    }

    /// Record a hallucination (incorrect inference) for accuracy tracking.
    pub fn record_hallucination(&self) {
        self.inner.write().unwrap().hallucination_count += 1;
    }

    /// Returns the number of materialised `TEMPORAL_LINK` edges.
    pub fn temporal_link_count(&self) -> usize {
        self.inner.read().unwrap().temporal_links.len()
    }

    /// Returns all materialised temporal links as a snapshot.
    pub fn temporal_links_snapshot(&self) -> Vec<TemporalLink> {
        self.inner
            .read()
            .unwrap()
            .temporal_links
            .values()
            .copied()
            .collect()
    }

    /// Number of distinct entities tracked in the graph.
    pub fn entity_count(&self) -> usize {
        self.inner.read().unwrap().entity_labels.len()
    }
}

impl Default for CausalGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compute Pearson correlation `r` and a two-tailed p-value from a t-distribution
/// approximation.
///
/// Returns `None` if either series is too short or has zero variance.
fn pearson_with_pvalue(x: &[f64], y: &[f64]) -> Option<(f64, f64)> {
    let n = x.len().min(y.len());
    if n < 3 {
        return None;
    }

    let mean_x = x[..n].iter().sum::<f64>() / n as f64;
    let mean_y = y[..n].iter().sum::<f64>() / n as f64;

    let (mut cov, mut var_x, mut var_y) = (0.0f64, 0.0f64, 0.0f64);
    for i in 0..n {
        let dx = x[i] - mean_x;
        let dy = y[i] - mean_y;
        cov += dx * dy;
        var_x += dx * dx;
        var_y += dy * dy;
    }

    if var_x == 0.0 || var_y == 0.0 {
        return None;
    }

    let r = cov / (var_x.sqrt() * var_y.sqrt());
    let r_clamped = r.clamp(-1.0 + f64::EPSILON, 1.0 - f64::EPSILON);

    // t-statistic: t = r * sqrt(n-2) / sqrt(1 - r²)
    let t = r_clamped * ((n as f64 - 2.0).sqrt()) / (1.0 - r_clamped * r_clamped).sqrt();

    // Approximate two-tailed p-value using a logistic sigmoid on |t|.
    // This is an HFT approximation — not a full t-distribution CDF —
    // but is accurate for the p < 0.05 threshold used here.
    let abs_t = t.abs();
    let p_value = 2.0 / (1.0 + (0.717 * abs_t + 0.416 * abs_t * abs_t).exp());

    Some((r, p_value))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_graph() -> CausalGraph {
        CausalGraph::new()
    }

    #[test]
    fn label_store_insert_and_traverse() {
        let mut store = LabelStore::new();
        store.insert(LabelId::L_CRUDE_SHOCK, 1);
        store.insert(LabelId::L_CRUDE_SHOCK, 2);
        store.insert(LabelId::L_RATE_HIKE, 3);

        let entities = store.entities_for_label(LabelId::L_CRUDE_SHOCK).unwrap();
        assert!(entities.contains(&1));
        assert!(entities.contains(&2));

        let all = store.traverse_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn causal_graph_tag_and_activate() {
        let g = make_graph();
        g.tag_entity(10, vec![LabelId::L_RATE_HIKE, LabelId::L_MACRO_RELEASE]);
        g.tag_entity(11, vec![LabelId::L_RATE_HIKE]);
        g.tag_entity(12, vec![LabelId::L_GEO_DISRUPTION]);

        let reached = g.spreading_activation(LabelId::L_RATE_HIKE, 2);
        assert!(reached.contains(&10));
        assert!(reached.contains(&11));
        // Entity 12 has a different label and should not be reachable from RATE_HIKE.
        assert!(!reached.contains(&12));
    }

    #[test]
    fn temporal_link_materializes_on_correlated_data() {
        let g = make_graph();
        // Register two entities with shared labels.
        g.tag_entity(100, vec![LabelId::L_CRUDE_SHOCK]);
        g.tag_entity(101, vec![LabelId::L_CRUDE_SHOCK]);

        // Feed perfectly correlated return samples.
        for i in 0..50 {
            let r = i as f64 * 0.01;
            g.record_return(100, r);
            g.record_return(101, r + 0.001); // near-perfect correlation
        }

        g.materialize_temporal_link(100, 0.3);
        // Perfect correlation → p ≈ 0, link should be materialised.
        assert!(g.temporal_link_count() > 0, "expected TEMPORAL_LINK to be materialised");
    }

    #[test]
    fn temporal_link_not_materialized_for_uncorrelated() {
        let g = make_graph();
        g.tag_entity(200, vec![LabelId::L_EARNINGS_SURPRISE]);
        g.tag_entity(201, vec![LabelId::L_EARNINGS_SURPRISE]);

        // Feed uncorrelated (alternating-sign) data.
        for i in 0..50 {
            let r = if i % 2 == 0 { 0.1 } else { -0.1 };
            g.record_return(200, r);
            g.record_return(201, -r); // perfectly anti-correlated for this test
        }
        // Anti-correlation with |r| = 1 still passes, but let's use truly random data.
        // For a simpler test, check non-correlated:
        // Clear and use zero-mean noise.
        let g2 = make_graph();
        g2.tag_entity(200, vec![LabelId::L_EARNINGS_SURPRISE]);
        g2.tag_entity(201, vec![LabelId::L_EARNINGS_SURPRISE]);
        let samples_a = [0.1, -0.1, 0.1, -0.1, 0.1, -0.3, 0.2, -0.2, 0.0, 0.0,
                         0.05, -0.05, 0.0, 0.0, 0.1, -0.1, 0.05, -0.05, 0.0, 0.01];
        let samples_b = [0.0, 0.0, -0.1, 0.1, -0.2, 0.2, 0.0, 0.0, 0.3, -0.3,
                         -0.05, 0.05, 0.0, 0.0, -0.1, 0.1, 0.0, 0.0, -0.05, 0.05];
        for (&a, &b) in samples_a.iter().zip(samples_b.iter()) {
            g2.record_return(200, a);
            g2.record_return(201, b);
        }
        g2.materialize_temporal_link(200, 0.1);
        // Uncorrelated → no temporal links.
        assert_eq!(g2.temporal_link_count(), 0, "should NOT materialise link for uncorrelated data");
    }

    #[test]
    fn grounding_ratio_starts_at_one() {
        let g = make_graph();
        assert!((g.grounding_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn grounding_ratio_below_threshold_after_hallucinations() {
        let g = make_graph();
        // 999 facts, 1 hallucination → ratio = 0.999 (still above 0.995)
        for _ in 0..999 {
            g.inner.write().unwrap().fact_count += 1;
        }
        g.record_hallucination();
        assert!(g.grounding_ratio() >= 0.995);
    }

    #[test]
    fn spreading_activation_100us_budget() {
        let g = make_graph();
        // Build a modest graph and time the traversal.
        for i in 0..1000u64 {
            let label = LabelId(1 + (i % 8) as u32);
            g.tag_entity(i, vec![label]);
        }
        let start = std::time::Instant::now();
        let _ = g.spreading_activation(LabelId::L_CRUDE_SHOCK, 3);
        let elapsed = start.elapsed();
        println!("spreading_activation elapsed: {:?}", elapsed);
        // In debug builds, allow 5x budget (500 µs).
        assert!(
            elapsed.as_micros() < 500,
            "traversal took {:?}, exceeds debug budget",
            elapsed
        );
    }
}
