//! # atlas-types
//!
//! Shared, `#[repr(C)]` data types used across all Atlas-Finance crates.
//! Designed for zero-copy transmission across thread / process boundaries.

#![deny(missing_docs)]

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// WeightedEntity
// ─────────────────────────────────────────────────────────────────────────────

/// A single financial entity tracked by the Hot Tier.
///
/// The struct is pinned to a 64-byte cache line (`align(64)`) to eliminate
/// false-sharing between concurrent readers/writers.  The layout is
/// deliberately `#[repr(C)]` so that DPDK zero-copy DMA can read fields
/// directly from mapped hugepage memory.
///
/// # Size contract
/// `size_of::<WeightedEntity>()` must equal **64**.  The padding field
/// enforces this at compile time.
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct WeightedEntity {
    /// Unique entity identifier (ticker CRC-64 or SEC CIK).
    pub entity_id: u64,
    /// Latest mid-price received from the market feed (IEEE-754 double).
    pub latest_price: f64,
    /// Rolling volatility score (σ of last N ticks, normalised 0–1).
    pub volatility_score: f32,
    /// Importance weight used by the Reasoning Agent (`W` in the formula).
    pub importance_weight: f32,
    /// When `true`, the Sensing Agent must fire a [`SignalEvent`] immediately.
    pub causal_trigger_bit: bool,
    /// 3 bytes of explicit padding (compiler would add them anyway).
    _pad: [u8; 3],
    /// Wall-clock timestamp of last mutation, nanoseconds since UNIX epoch.
    pub last_updated_ns: u64,
    /// Reserved for future fields; keeps struct at exactly 64 bytes.
    _reserved: [u8; 20],
}

// Compile-time size guard.
const _: () = assert!(
    core::mem::size_of::<WeightedEntity>() == 64,
    "WeightedEntity must be exactly one cache line (64 bytes)"
);

impl WeightedEntity {
    /// Create a new entity with sane defaults.
    pub fn new(entity_id: u64, latest_price: f64) -> Self {
        Self {
            entity_id,
            latest_price,
            volatility_score: 0.0,
            importance_weight: 0.5,
            causal_trigger_bit: false,
            _pad: [0; 3],
            last_updated_ns: 0,
            _reserved: [0; 20],
        }
    }

    /// Recalculate `importance_weight` in-place using the weighted formula:
    ///
    /// ```text
    /// W_new = (W_old × α) + (S × β)
    /// ```
    ///
    /// `alpha` is the decay constant (e.g. 0.9), `beta` is the surprise
    /// sensitivity (e.g. 0.1), and `surprise` is the normalised surprise
    /// factor `S` derived from the incoming tick delta.
    #[inline(always)]
    pub fn recalculate_weight(&mut self, surprise: f32, alpha: f32, beta: f32) {
        self.importance_weight = (self.importance_weight * alpha) + (surprise * beta);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SignalEvent
// ─────────────────────────────────────────────────────────────────────────────

/// Events emitted by the Sensing Agent and consumed by the Reasoning Agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SignalEvent {
    /// The importance weight of an entity spiked by `delta_w`.
    WeightSpike {
        /// Entity that experienced the spike.
        entity_id: u64,
        /// Absolute change in weight (always positive).
        delta_w: f32,
        /// New weight value after recalculation.
        new_weight: f32,
        /// Timestamp of detection, ns since UNIX epoch.
        detected_at_ns: u64,
    },
    /// The `causal_trigger_bit` of an entity flipped to `true`.
    CausalTrigger {
        /// Entity that set the causal trigger.
        entity_id: u64,
        /// Timestamp of detection, ns since UNIX epoch.
        detected_at_ns: u64,
    },
}

impl SignalEvent {
    /// Extract the entity ID regardless of variant.
    pub fn entity_id(&self) -> u64 {
        match self {
            SignalEvent::WeightSpike { entity_id, .. } => *entity_id,
            SignalEvent::CausalTrigger { entity_id, .. } => *entity_id,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LabelId
// ─────────────────────────────────────────────────────────────────────────────

/// Semantic label assigned to financial entities by the ingestor.
///
/// Labels drive causal graph edge materialisation in the Warm Tier.
/// New labels must be registered here and given a stable, non-zero `u32` ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LabelId(pub u32);

impl LabelId {
    /// Crude-oil supply/demand shock.
    pub const L_CRUDE_SHOCK: LabelId = LabelId(1);
    /// Federal Reserve rate-hike signal.
    pub const L_RATE_HIKE: LabelId = LabelId(2);
    /// Earnings-surprise event (beat or miss).
    pub const L_EARNINGS_SURPRISE: LabelId = LabelId(3);
    /// Geopolitical supply disruption.
    pub const L_GEO_DISRUPTION: LabelId = LabelId(4);
    /// Regulatory filing (SEC 8-K, 10-K).
    pub const L_SEC_FILING: LabelId = LabelId(5);
    /// Macro-economic data release (CPI, NFP, etc.).
    pub const L_MACRO_RELEASE: LabelId = LabelId(6);
    /// Credit-rating change.
    pub const L_CREDIT_EVENT: LabelId = LabelId(7);
    /// Merger / acquisition announcement.
    pub const L_M_AND_A: LabelId = LabelId(8);

    /// Returns `true` if this is a known, registered label.
    pub fn is_valid(self) -> bool {
        self.0 >= 1 && self.0 <= 8
    }
}

impl std::fmt::Display for LabelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            1 => "L_CRUDE_SHOCK",
            2 => "L_RATE_HIKE",
            3 => "L_EARNINGS_SURPRISE",
            4 => "L_GEO_DISRUPTION",
            5 => "L_SEC_FILING",
            6 => "L_MACRO_RELEASE",
            7 => "L_CREDIT_EVENT",
            8 => "L_M_AND_A",
            _ => "L_UNKNOWN",
        };
        write!(f, "{}({})", name, self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TemporalLink
// ─────────────────────────────────────────────────────────────────────────────

/// A `TEMPORAL_LINK` edge in the Causal Graph, materialised when two labels
/// demonstrate statistically significant historical co-movement (p < 0.05).
#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TemporalLink {
    /// Source label.
    pub src: LabelId,
    /// Destination label (directional: src → dst).
    pub dst: LabelId,
    /// Pearson p-value that justified materialisation (< 0.05).
    pub p_value: f32,
    /// Wall-clock of edge creation, ns since UNIX epoch.
    pub created_ns: u64,
    /// Edge weight (absolute Pearson r, 0–1).
    pub correlation_abs: f32,
}

impl TemporalLink {
    /// Create a new temporal link.  Panics in debug if `p_value >= 0.05`.
    pub fn new(src: LabelId, dst: LabelId, p_value: f32, correlation_abs: f32, created_ns: u64) -> Self {
        debug_assert!(p_value < 0.05, "TemporalLink requires p < 0.05, got {p_value}");
        Self { src, dst, p_value, correlation_abs, created_ns }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

/// Return the current wall-clock time as nanoseconds since the UNIX epoch.
/// Falls back to 0 on platforms where `std::time` is unavailable.
#[inline]
pub fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{size_of, align_of};

    #[test]
    fn weighted_entity_size_is_64_bytes() {
        assert_eq!(size_of::<WeightedEntity>(), 64);
    }

    #[test]
    fn weighted_entity_alignment_is_64() {
        assert_eq!(align_of::<WeightedEntity>(), 64);
    }

    #[test]
    fn weight_recalculation_formula() {
        let mut e = WeightedEntity::new(1, 100.0);
        e.importance_weight = 0.8;
        // W_new = 0.8 * 0.9 + 0.5 * 0.1 = 0.72 + 0.05 = 0.77
        e.recalculate_weight(0.5, 0.9, 0.1);
        let expected = 0.8f32 * 0.9 + 0.5 * 0.1;
        assert!((e.importance_weight - expected).abs() < f32::EPSILON,
            "Expected {expected}, got {}", e.importance_weight);
    }

    #[test]
    fn signal_event_entity_id_accessor() {
        let ev = SignalEvent::CausalTrigger { entity_id: 42, detected_at_ns: 0 };
        assert_eq!(ev.entity_id(), 42);

        let ev2 = SignalEvent::WeightSpike { entity_id: 99, delta_w: 0.3, new_weight: 0.9, detected_at_ns: 0 };
        assert_eq!(ev2.entity_id(), 99);
    }

    #[test]
    fn label_id_known_constants_are_valid() {
        assert!(LabelId::L_CRUDE_SHOCK.is_valid());
        assert!(LabelId::L_RATE_HIKE.is_valid());
        assert!(LabelId::L_M_AND_A.is_valid());
        assert!(!LabelId(0).is_valid());
        assert!(!LabelId(255).is_valid());
    }

    #[test]
    fn temporal_link_stores_fields_correctly() {
        let link = TemporalLink::new(
            LabelId::L_CRUDE_SHOCK,
            LabelId::L_GEO_DISRUPTION,
            0.03,
            0.72,
            now_ns(),
        );
        assert_eq!(link.src, LabelId::L_CRUDE_SHOCK);
        assert_eq!(link.dst, LabelId::L_GEO_DISRUPTION);
        assert!(link.p_value < 0.05);
    }
}
