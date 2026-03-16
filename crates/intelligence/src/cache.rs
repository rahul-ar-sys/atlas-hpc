//! Prefix Cache with dirty-bit invalidation.
//!
//! Implements position-independent prefix caching to bypass the context-prefill
//! penalty for static financial documents.  Cache keys are derived from
//! `entity_id` (in production: a hash of the entity_id + label fingerprint).

use std::collections::HashSet;
use dashmap::DashMap;
use atlas_types::LabelId;
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Cache entry
// ─────────────────────────────────────────────────────────────────────────────

/// A cached reasoning result for one entity.
#[derive(Clone, Debug)]
pub struct CachedReasoning {
    /// Entity this cache entry covers.
    pub entity_id: u64,
    /// Entities discovered via spreading activation at cache-fill time.
    pub related_entities: Vec<u64>,
    /// Labels that contributed to this entry (used for dirty-bit invalidation).
    pub label_set: Vec<LabelId>,
    /// Wall-clock insertion time (ns since UNIX epoch).
    pub inserted_ns: u64,
    /// Whether this entry has been invalidated (dirty-bit).
    pub dirty: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// PrefixCache
// ─────────────────────────────────────────────────────────────────────────────

/// Lock-free prefix cache backed by `DashMap` (sharded RwLock).
///
/// Each entry is keyed by `entity_id` (simplified from the production key
/// which would be a 64-bit hash of entity_id ⊕ label fingerprint).
pub struct PrefixCache {
    entries: DashMap<u64, CachedReasoning>,
}

impl PrefixCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self { entries: DashMap::new() }
    }

    /// Insert a new cache entry.
    pub fn insert(&self, entity_id: u64, related_entities: Vec<u64>, label_set: Vec<LabelId>) {
        let entry = CachedReasoning {
            entity_id,
            related_entities,
            label_set,
            inserted_ns: atlas_types::now_ns(),
            dirty: false,
        };
        self.entries.insert(entity_id, entry);
    }

    /// Returns `true` if a clean (non-dirty) entry exists for this entity.
    pub fn contains(&self, entity_id: u64) -> bool {
        self.entries
            .get(&entity_id)
            .map(|e| !e.dirty)
            .unwrap_or(false)
    }

    /// Retrieve the related entity list for a cache entry.
    pub fn get_related(&self, entity_id: u64) -> Option<Vec<u64>> {
        self.entries
            .get(&entity_id)
            .filter(|e| !e.dirty)
            .map(|e| e.related_entities.clone())
    }

    /// **Dirty-Bit Invalidation**: when a graph node updates, clear *only*
    /// cache entries whose `label_set` includes the affected labels.
    ///
    /// This preserves clean, unaffected entries, avoiding a full cache flush.
    pub fn invalidate_dirty(&self, entity_id: u64, updated_labels: &[LabelId]) {
        let updated_set: HashSet<LabelId> = updated_labels.iter().copied().collect();

        for mut entry in self.entries.iter_mut() {
            let entry_labels: HashSet<LabelId> = entry.label_set.iter().copied().collect();
            let is_dirty = entry.entity_id == entity_id
                || !entry_labels.is_disjoint(&updated_set);

            if is_dirty && !entry.dirty {
                debug!("dirty-bit invalidating cache entry for entity {}", entry.entity_id);
                entry.dirty = true;
            }
        }
    }

    /// Evict all dirty entries, freeing memory.
    pub fn evict_dirty(&self) {
        self.entries.retain(|_, v| !v.dirty);
    }

    /// Total number of entries (including dirty).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Count of clean (non-dirty) entries.
    pub fn clean_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.dirty).count()
    }
}

impl Default for PrefixCache {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cache() -> PrefixCache {
        PrefixCache::new()
    }

    #[test]
    fn insert_and_hit() {
        let c = make_cache();
        c.insert(1, vec![2, 3], vec![LabelId::L_RATE_HIKE]);
        assert!(c.contains(1));
        assert_eq!(c.get_related(1).unwrap(), vec![2, 3]);
    }

    #[test]
    fn dirty_bit_invalidates_only_affected() {
        let c = make_cache();
        // entity 1 → RATE_HIKE
        c.insert(1, vec![2], vec![LabelId::L_RATE_HIKE]);
        // entity 5 → CRUDE_SHOCK (unrelated)
        c.insert(5, vec![6], vec![LabelId::L_CRUDE_SHOCK]);

        // Invalidate entries touching RATE_HIKE.
        c.invalidate_dirty(1, &[LabelId::L_RATE_HIKE]);

        // entry 1 should be dirty, entry 5 should be clean.
        assert!(!c.contains(1), "entity 1 should be dirty/invalid");
        assert!(c.contains(5), "entity 5 should still be clean");
    }

    #[test]
    fn evict_dirty_frees_entries() {
        let c = make_cache();
        c.insert(1, vec![], vec![LabelId::L_RATE_HIKE]);
        c.insert(2, vec![], vec![LabelId::L_CRUDE_SHOCK]);
        c.invalidate_dirty(1, &[LabelId::L_RATE_HIKE]);
        assert_eq!(c.len(), 2);
        c.evict_dirty();
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn clean_count_excludes_dirty() {
        let c = make_cache();
        c.insert(10, vec![], vec![LabelId::L_RATE_HIKE]);
        c.insert(11, vec![], vec![LabelId::L_RATE_HIKE]);
        c.insert(12, vec![], vec![LabelId::L_CRUDE_SHOCK]);
        c.invalidate_dirty(10, &[LabelId::L_RATE_HIKE]);
        // entries 10 and 11 share the RATE_HIKE label → both dirty
        assert_eq!(c.clean_count(), 1);
    }
}
