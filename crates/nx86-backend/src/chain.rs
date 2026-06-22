use std::collections::{HashMap, HashSet};

/// Number of observations required before a dispatcher edge becomes a direct
/// software chain (and is eligible for native patching).
pub const DEFAULT_CHAIN_THRESHOLD: u32 = 2;

/// Block-chaining activity counters. A [`crate::DispatchOutcome`] contains the
/// delta for one run; [`crate::Dispatcher::chain_stats`] returns cumulative data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChainStats {
    pub software_hits: usize,
    pub software_installs: usize,
    pub native_chain_entries: usize,
    pub native_installs: usize,
    pub invalidations: usize,
}

impl ChainStats {
    pub(crate) fn difference(self, earlier: Self) -> Self {
        Self {
            software_hits: self.software_hits.saturating_sub(earlier.software_hits),
            software_installs: self
                .software_installs
                .saturating_sub(earlier.software_installs),
            native_chain_entries: self
                .native_chain_entries
                .saturating_sub(earlier.native_chain_entries),
            native_installs: self.native_installs.saturating_sub(earlier.native_installs),
            invalidations: self.invalidations.saturating_sub(earlier.invalidations),
        }
    }
}

/// Host-independent hot-edge tracking and direct-chain cache.
#[derive(Debug)]
pub(crate) struct ChainCache {
    enabled: bool,
    threshold: u32,
    hot_edges: HashMap<(u64, u64), u32>,
    direct_chains: HashMap<u64, u64>,
    incoming: HashMap<u64, HashSet<u64>>,
    stats: ChainStats,
}

impl Default for ChainCache {
    fn default() -> Self {
        Self::new(DEFAULT_CHAIN_THRESHOLD)
    }
}

impl ChainCache {
    pub(crate) fn new(threshold: u32) -> Self {
        Self {
            enabled: true,
            threshold: threshold.max(1),
            hot_edges: HashMap::new(),
            direct_chains: HashMap::new(),
            incoming: HashMap::new(),
            stats: ChainStats::default(),
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }

    #[cfg(all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    ))]
    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Record an observed edge. Returns true only when this observation installs
    /// a new direct chain.
    pub(crate) fn observe(&mut self, source_pc: u64, target_pc: u64) -> bool {
        if !self.enabled {
            return false;
        }
        if self.direct_chains.get(&source_pc) == Some(&target_pc) {
            return false;
        }

        let count = self.hot_edges.entry((source_pc, target_pc)).or_default();
        *count = count.saturating_add(1);
        if *count < self.threshold {
            return false;
        }

        self.remove_outgoing(source_pc);
        self.direct_chains.insert(source_pc, target_pc);
        self.incoming
            .entry(target_pc)
            .or_default()
            .insert(source_pc);
        self.stats.software_installs = self.stats.software_installs.saturating_add(1);
        true
    }

    /// Confirm that a cached direct chain matches the block's actual successor.
    pub(crate) fn hit(&mut self, source_pc: u64, target_pc: u64) -> bool {
        if self.enabled && self.direct_chains.get(&source_pc) == Some(&target_pc) {
            self.stats.software_hits = self.stats.software_hits.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub(crate) fn record_native_entry(&mut self) {
        self.stats.native_chain_entries = self.stats.native_chain_entries.saturating_add(1);
    }

    #[cfg(all(
        feature = "native-patch-chaining",
        target_os = "linux",
        target_arch = "x86_64"
    ))]
    pub(crate) fn record_native_install(&mut self) {
        self.stats.native_installs = self.stats.native_installs.saturating_add(1);
    }

    pub(crate) fn invalidate(&mut self, pc: u64) {
        self.remove_outgoing(pc);
        if let Some(sources) = self.incoming.remove(&pc) {
            for source in sources {
                self.direct_chains.remove(&source);
            }
        }
        self.hot_edges
            .retain(|(source, target), _| *source != pc && *target != pc);
        self.stats.invalidations = self.stats.invalidations.saturating_add(1);
    }

    pub(crate) const fn stats(&self) -> ChainStats {
        self.stats
    }

    fn clear(&mut self) {
        self.hot_edges.clear();
        self.direct_chains.clear();
        self.incoming.clear();
    }

    fn remove_outgoing(&mut self, source_pc: u64) {
        let Some(old_target) = self.direct_chains.remove(&source_pc) else {
            return;
        };
        if let Some(sources) = self.incoming.get_mut(&old_target) {
            sources.remove(&source_pc);
            if sources.is_empty() {
                self.incoming.remove(&old_target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChainCache;

    #[test]
    fn hot_edge_installs_then_hits() {
        let mut cache = ChainCache::new(2);
        assert!(!cache.observe(0, 8));
        assert!(cache.observe(0, 8));
        assert!(cache.hit(0, 8));
        assert_eq!(cache.stats().software_installs, 1);
        assert_eq!(cache.stats().software_hits, 1);
    }

    #[test]
    fn invalidation_clears_incoming_and_outgoing_chains() {
        let mut cache = ChainCache::new(1);
        assert!(cache.observe(0, 8));
        assert!(cache.observe(8, 16));

        cache.invalidate(8);

        assert!(!cache.hit(0, 8));
        assert!(!cache.hit(8, 16));
        assert_eq!(cache.stats().invalidations, 1);
    }

    #[test]
    fn disabling_clears_and_prevents_chains() {
        let mut cache = ChainCache::new(1);
        assert!(cache.observe(0, 8));
        cache.set_enabled(false);

        assert!(!cache.hit(0, 8));
        assert!(!cache.observe(0, 8));
        assert!(!cache.enabled);
    }
}
