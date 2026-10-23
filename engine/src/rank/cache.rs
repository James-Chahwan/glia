//! The long-lived holder of what a ranked answer rebuilds per call otherwise
//! (EA.2, item 5): PPR indexes, the carry adjacency (B0's walk) and, from
//! EA.3, the file index and the co-change index (`[rank-files]` /
//! `[rank-cochange]`, see those modules).
//!
//! An index is found by its [`SourceKey`] and [`PprIndex::matches`] (damping,
//! direction, iteration cap, epsilon and every edge weight, bit for bit;
//! `top_k` and `node_specificity` act after PPR and are no part of it). The
//! graph identity is the holder's: one RankCache serves one graph, and
//! [`RankCache::guard`] empties it when the graph's (nodes, edges) shape
//! moves. Entries live in a Vec in insertion order, so eviction never reads
//! map order, and an evicted index only costs a rebuild, never a score.
//!
//! Fired_on, stderr: `[rank-cache] holder=<h> source=<merged|quotient>
//! damping=<d> direction=<forward|backward|undirected> index=<hit|built>
//! nodes=<n> incidences=<i> build_ms=<b> entries=<e>` once per index lookup,
//! `[rank-cache] reset holder=<h> reason=graph_changed nodes=<a>-><b>
//! edges=<c>-><d>` on a guard reset.
//!
//! EC.8: the cache also holds the repo's calibration record
//! ([`RankCache::calibration`], `load_for_roots` over its roots, read once
//! per cache and dropped by a guard reset), so a ranked answer selects its
//! ranking without re-reading a file per call.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use glia_activation::algo::{Adjacency, GraphSource};
use glia_activation::{ActivationConfig, Direction, PprIndex};
use glia_code_domain::profile::CODE_TABLES;
use glia_graph::MergedGraph;
use serde::Serialize;

use crate::calibrate::{CalibrationHandle, load_for_roots};

use super::cochange::CochangeIndex;
use super::files::FileIndex;

/// The PPR indexes kept at once; the oldest inserted goes first.
const MAX_INDEXES: usize = 8;

/// Which view of the holder's graph an index was built over.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// The MergedGraph's own GraphSource: every graph's nodes in graph order,
    /// `all_edges()` (rank_eval's `all_ids` / `edges`).
    Merged,
    /// The identity quotient DD.10's blast walk ranks over.
    Quotient,
}

impl SourceKey {
    fn name(self) -> &'static str {
        match self {
            SourceKey::Merged => "merged",
            SourceKey::Quotient => "quotient",
        }
    }
}

fn direction_name(d: Direction) -> &'static str {
    match d {
        Direction::Forward => "forward",
        Direction::Backward => "backward",
        Direction::Undirected => "undirected",
    }
}

/// A snapshot of a cache's counters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RankCacheStats {
    pub holder: &'static str,
    /// PPR indexes held now.
    pub entries: usize,
    /// Lookups answered from a held index.
    pub hits: usize,
    /// Indexes built (a racing duplicate counts too).
    pub built: usize,
}

/// What a guard reset clears.
#[derive(Default)]
struct Slots {
    indexes: Vec<(SourceKey, Arc<PprIndex>)>,
    carry: Option<Arc<Adjacency>>,
    /// The candidate file space (EA.3).
    files: Option<Arc<FileIndex>>,
    /// The co-change index over `files` (EA.3): read from the snapshots, or
    /// given by [`RankCache::with_cochange`].
    cochange: Option<Arc<CochangeIndex>>,
}

/// One graph's ranking indexes, shared across calls and threads.
pub struct RankCache {
    holder: &'static str,
    roots: BTreeMap<u64, String>,
    labels: BTreeMap<u64, String>,
    /// `(nodes, edges)` of the graph the slots were built over.
    shape: Mutex<Option<(usize, usize)>>,
    slots: Mutex<Slots>,
    /// The calibration record of `roots` (EC.8), read on first use under
    /// this lock, so one cache reads it once.
    calibration: Mutex<Option<CalibrationHandle>>,
    hits: AtomicUsize,
    built: AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RankCache {
    /// An empty cache. `holder` names the owner in the marker (`cli`,
    /// `pygraph`, `watch`, `answer`); the repo roots and labels are the ones
    /// the answers locate and tie-break rows by.
    pub fn new(holder: &'static str, repo_roots: &BTreeMap<u64, String>, repo_labels: &BTreeMap<u64, String>) -> Self {
        RankCache {
            holder,
            roots: repo_roots.clone(),
            labels: repo_labels.clone(),
            shape: Mutex::new(None),
            slots: Mutex::new(Slots::default()),
            calibration: Mutex::new(None),
            hits: AtomicUsize::new(0),
            built: AtomicUsize::new(0),
        }
    }

    /// The PPR index of `g` (the `source` view of this cache's graph) under
    /// `config`: a held one, else built now (outside the lock: two racing
    /// builders build identical indexes and the first insert wins).
    pub fn ppr_index<G: GraphSource + ?Sized>(&self, g: &G, source: SourceKey, config: &ActivationConfig) -> Arc<PprIndex> {
        let held = {
            let slots = lock(&self.slots);
            slots.indexes.iter().find(|(s, ix)| *s == source && ix.matches(config)).map(|(_, ix)| Arc::clone(ix))
        };
        if let Some(ix) = held {
            self.hits.fetch_add(1, Ordering::Relaxed);
            let entries = lock(&self.slots).indexes.len();
            self.mark(source, config, "hit", &ix, 0.0, entries);
            return ix;
        }
        let t0 = Instant::now();
        let built = Arc::new(PprIndex::build(&g.node_ids(), g.edges(), config));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.built.fetch_add(1, Ordering::Relaxed);
        let (ix, entries) = {
            let mut slots = lock(&self.slots);
            let raced = slots.indexes.iter().find(|(s, ix)| *s == source && ix.matches(config)).map(|(_, ix)| Arc::clone(ix));
            let ix = match raced {
                Some(first) => first,
                None => {
                    slots.indexes.push((source, Arc::clone(&built)));
                    while slots.indexes.len() > MAX_INDEXES {
                        slots.indexes.remove(0);
                    }
                    built
                }
            };
            (ix, slots.indexes.len())
        };
        self.mark(source, config, "built", &ix, ms, entries);
        ix
    }

    fn mark(&self, source: SourceKey, config: &ActivationConfig, how: &str, ix: &PprIndex, ms: f64, entries: usize) {
        eprintln!(
            "[rank-cache] holder={} source={} damping={} direction={} index={how} nodes={} incidences={} build_ms={:.1} entries={entries}",
            self.holder,
            source.name(),
            config.damping,
            direction_name(config.direction),
            ix.len(),
            ix.incidences(),
            (ms * 10.0).trunc() / 10.0,
        );
    }

    /// The carry-edge adjacency of `merged` (`Adjacency::carry` over
    /// CODE_TABLES, B0's walk), built once.
    pub fn carry(&self, merged: &MergedGraph) -> Arc<Adjacency> {
        if let Some(adj) = lock(&self.slots).carry.as_ref() {
            return Arc::clone(adj);
        }
        let adj = Arc::new(Adjacency::carry(merged, &CODE_TABLES));
        let mut slots = lock(&self.slots);
        Arc::clone(slots.carry.get_or_insert(adj))
    }

    /// The candidate file space of `merged` ([`FileIndex::of`] under this
    /// cache's repo labels), built once; prints `[rank-files]` when built.
    pub fn files(&self, merged: &MergedGraph) -> Arc<FileIndex> {
        if let Some(f) = lock(&self.slots).files.as_ref() {
            return Arc::clone(f);
        }
        let t0 = Instant::now();
        let built = Arc::new(FileIndex::of(merged, &self.labels));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (ix, fresh) = {
            let mut slots = lock(&self.slots);
            match slots.files.as_ref() {
                Some(first) => (Arc::clone(first), false),
                None => {
                    slots.files = Some(Arc::clone(&built));
                    (built, true)
                }
            }
        };
        if fresh {
            eprintln!(
                "[rank-files] holder={} repos={} files={} generated={} not_code={} ms={:.1}",
                self.holder,
                ix.repos(),
                ix.len(),
                ix.generated(),
                ix.not_code(),
                (ms * 10.0).trunc() / 10.0,
            );
        }
        ix
    }

    /// The co-change index of `merged`'s file space: a given one
    /// ([`Self::with_cochange`]), else read from each repo root's history
    /// snapshot once; prints `[rank-cochange]` when it is first built.
    pub fn cochange(&self, merged: &MergedGraph) -> Arc<CochangeIndex> {
        if let Some(c) = lock(&self.slots).cochange.as_ref() {
            return Arc::clone(c);
        }
        let files = self.files(merged);
        let t0 = Instant::now();
        let built = Arc::new(CochangeIndex::read(&files, &self.roots));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (ix, fresh) = {
            let mut slots = lock(&self.slots);
            match slots.cochange.as_ref() {
                Some(first) => (Arc::clone(first), false),
                None => {
                    slots.cochange = Some(Arc::clone(&built));
                    (built, true)
                }
            }
        };
        if fresh {
            let s = ix.stats();
            if self.roots.is_empty() {
                eprintln!("[rank-cochange] holder={} none reason=no_roots", self.holder);
            } else if s.snapshots == 0 {
                eprintln!("[rank-cochange] holder={} none reason=no_snapshot", self.holder);
            } else {
                self.mark_cochange("snapshot", &ix, ms);
            }
        }
        ix
    }

    /// This cache with a caller-supplied co-change index (EA.9's replay, EC's
    /// calibration) in place of the snapshot read. A guard reset drops it.
    pub fn with_cochange(self, idx: CochangeIndex) -> Self {
        let ix = Arc::new(idx);
        self.mark_cochange("given", &ix, 0.0);
        lock(&self.slots).cochange = Some(ix);
        self
    }

    fn mark_cochange(&self, source: &str, ix: &CochangeIndex, ms: f64) {
        let s = ix.stats();
        eprintln!(
            "[rank-cochange] holder={} source={source} repos={} snapshots={} commits={} train_commits={} files={} pairs={} ms={:.1}",
            self.holder,
            s.repos,
            s.snapshots,
            s.commits,
            s.train_commits,
            s.files,
            s.pairs,
            (ms * 10.0).trunc() / 10.0,
        );
    }

    /// The calibration record of this cache's repo roots (EC.8):
    /// [`load_for_roots`] on first use (`NotLoaded` with no root,
    /// `Rejected("multi_repo")` for a merge, else the one root's
    /// `.glia/calibration-snapshot/`, with its `[calibration] read` line),
    /// then the held handle. A guard reset drops it.
    pub fn calibration(&self) -> CalibrationHandle {
        let mut held = lock(&self.calibration);
        held.get_or_insert_with(|| CalibrationHandle::new(load_for_roots(&self.roots))).clone()
    }

    /// Record `merged`'s shape on the first call; on a later call with
    /// another shape, empty every slot and print the reset line. Every answer
    /// that takes a RankCache calls this first. A defence, not the
    /// invalidation path: a PyGraph never changes edges and a WatchSession
    /// replaces its cache.
    pub fn guard(&self, merged: &MergedGraph) {
        let nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
        let edges = merged.all_edges().count();
        let mut shape = lock(&self.shape);
        match *shape {
            None => *shape = Some((nodes, edges)),
            Some((n0, e0)) if (n0, e0) != (nodes, edges) => {
                *lock(&self.slots) = Slots::default();
                *lock(&self.calibration) = None;
                *shape = Some((nodes, edges));
                eprintln!(
                    "[rank-cache] reset holder={} reason=graph_changed nodes={n0}->{nodes} edges={e0}->{edges}",
                    self.holder
                );
            }
            Some(_) => {}
        }
    }

    /// Repo id -> root, as given to [`Self::new`].
    pub fn repo_roots(&self) -> &BTreeMap<u64, String> {
        &self.roots
    }

    /// Repo id -> label, as given to [`Self::new`].
    pub fn repo_labels(&self) -> &BTreeMap<u64, String> {
        &self.labels
    }

    pub fn stats(&self) -> RankCacheStats {
        RankCacheStats {
            holder: self.holder,
            entries: lock(&self.slots).indexes.len(),
            hits: self.hits.load(Ordering::Relaxed),
            built: self.built.load(Ordering::Relaxed),
        }
    }
}

impl fmt::Debug for RankCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.stats();
        f.debug_struct("RankCache")
            .field("holder", &s.holder)
            .field("entries", &s.entries)
            .field("hits", &s.hits)
            .field("built", &s.built)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use glia_activation::plan::ActivationPlan;
    use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
    use glia_core::{Confidence, Edge, Node, NodeId, RepoId};
    use glia_graph::{RepoGraph, SymbolTable};

    use crate::rank::WIDE_DAMPING;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://rank-cache")
    }

    fn id(q: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, q)
    }

    /// a -CALLS-> b -CALLS-> c, d -CALLS-> c, a -IMPORTS-> d, plus `extra`
    /// more CALLS edges c -> a.
    fn merged(extra: usize) -> MergedGraph {
        let r = repo();
        let names = ["m::a", "m::b", "m::c", "m::d"];
        let mut nav = CodeNav::default();
        for q in names {
            nav.record(id(q), q.trim_start_matches("m::"), q, node_kind::FUNCTION, None);
        }
        let e = |f: &str, t: &str, c| Edge { from: id(f), to: id(t), category: c, confidence: Confidence::Strong, cells: Vec::new() };
        let mut edges = vec![
            e("m::a", "m::b", edge_category::CALLS),
            e("m::b", "m::c", edge_category::CALLS),
            e("m::d", "m::c", edge_category::CALLS),
            e("m::a", "m::d", edge_category::IMPORTS),
        ];
        edges.extend((0..extra).map(|_| e("m::c", "m::a", edge_category::CALLS)));
        MergedGraph::new(vec![RepoGraph {
            repo: r,
            nodes: names.iter().map(|&q| Node { id: id(q), repo: r, confidence: Confidence::Strong, cells: vec![] }).collect(),
            edges,
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }])
    }

    fn cache() -> RankCache {
        RankCache::new("answer", &BTreeMap::new(), &BTreeMap::new())
    }

    fn undirected(damping: f64) -> ActivationConfig {
        let mut c = CODE_TABLES.activation_config(None);
        c.direction = Direction::Undirected;
        c.damping = damping;
        c
    }

    #[test]
    fn second_lookup_is_a_hit_on_the_same_arc() {
        let (g, c) = (merged(0), cache());
        c.guard(&g);
        let cfg = undirected(0.5);
        let a = c.ppr_index(&g, SourceKey::Merged, &cfg);
        let b = c.ppr_index(&g, SourceKey::Merged, &cfg);
        assert!(Arc::ptr_eq(&a, &b));
        let s = c.stats();
        assert_eq!((s.entries, s.hits, s.built), (1, 1, 1));
        // top_k and node_specificity are no part of the key.
        let mut k = cfg.clone();
        k.top_k = 3;
        assert!(Arc::ptr_eq(&a, &c.ppr_index(&g, SourceKey::Merged, &k)));
        // another source is another entry
        let q = c.ppr_index(&g, SourceKey::Quotient, &cfg);
        assert!(!Arc::ptr_eq(&a, &q));
        assert_eq!(c.stats().entries, 2);
    }

    #[test]
    fn two_dampings_are_two_entries() {
        let (g, c) = (merged(0), cache());
        let a = c.ppr_index(&g, SourceKey::Merged, &undirected(0.5));
        let b = c.ppr_index(&g, SourceKey::Merged, &undirected(WIDE_DAMPING));
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(c.stats().entries, 2);
        assert_eq!(c.stats().built, 2);
    }

    #[test]
    fn a_ninth_config_evicts_the_first_inserted() {
        let (g, c) = (merged(0), cache());
        let cfgs: Vec<ActivationConfig> = (0..=MAX_INDEXES).map(|i| undirected(0.1 + 0.05 * i as f64)).collect();
        let first = c.ppr_index(&g, SourceKey::Merged, &cfgs[0]);
        let second = c.ppr_index(&g, SourceKey::Merged, &cfgs[1]);
        for cfg in &cfgs[2..] {
            c.ppr_index(&g, SourceKey::Merged, cfg);
        }
        assert_eq!(c.stats().entries, MAX_INDEXES);
        assert!(Arc::ptr_eq(&second, &c.ppr_index(&g, SourceKey::Merged, &cfgs[1])));
        let again = c.ppr_index(&g, SourceKey::Merged, &cfgs[0]);
        assert!(!Arc::ptr_eq(&first, &again), "the first inserted was evicted and rebuilt");
        assert_eq!(c.stats().built, MAX_INDEXES + 2);
    }

    #[test]
    fn guard_empties_the_cache_when_the_edges_move() {
        let c = cache();
        let g0 = merged(0);
        c.guard(&g0);
        c.ppr_index(&g0, SourceKey::Merged, &undirected(0.5));
        let adj0 = c.carry(&g0);
        assert!(Arc::ptr_eq(&adj0, &c.carry(&g0)));
        c.guard(&g0);
        assert_eq!(c.stats().entries, 1, "same shape keeps the cache");
        let g1 = merged(1);
        c.guard(&g1);
        assert_eq!(c.stats().entries, 0);
        assert!(!Arc::ptr_eq(&adj0, &c.carry(&g1)), "the adjacency was dropped too");
    }

    #[test]
    fn rank_over_the_cached_index_equals_rank_in_f64_bits() {
        let (g, c) = (merged(0), cache());
        let candidates: Vec<NodeId> = ["m::a", "m::b", "m::c", "m::d"].iter().map(|q| id(q)).collect();
        for d in [0.5, WIDE_DAMPING] {
            for dir in [Direction::Forward, Direction::Backward, Direction::Undirected] {
                let mut cfg = undirected(d);
                cfg.direction = dir;
                let plan: ActivationPlan<'_, MergedGraph> = ActivationPlan::new(cfg.clone());
                let ix = c.ppr_index(&g, SourceKey::Merged, &cfg);
                let want = plan.rank(&g, &[id("m::a")], &candidates);
                let got = plan.rank_indexed(&g, &ix, &[id("m::a")], &candidates);
                let bits = |v: &glia_activation::plan::ActivatedView| -> Vec<(NodeId, u64)> {
                    v.scores.iter().map(|&(n, s)| (n, s.to_bits())).collect()
                };
                assert_eq!(bits(&got), bits(&want), "damping {d} {dir:?}");
                assert!(want.scores.iter().any(|&(_, s)| s > 0.0));
            }
        }
    }
}
