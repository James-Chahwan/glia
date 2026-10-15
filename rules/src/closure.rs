//! The `reaches` / `scc` builtins over activation::algo (DA.11).
//!
//! `reaches(x, y)` holds when `y` is reachable from `x` by ONE OR MORE hops of the
//! set's categories: the transitive closure `+` (CodeQL's `+`, GQL's `->{1,}`), so
//! `(x, x)` holds exactly when `x` is on a cycle, row for row what a recursive `tc`
//! rule over `edge/3` derives; `reaches/4` bounds the hop count to `1..=depth`. It
//! runs `algo::reach::bfs_levels` over ONE [`Adjacency`] per category set (R1 section
//! 1.3), never generic recursion: a row's tier is the first of the three passes
//! (FACT, DERIVED, HEURISTIC) that reaches it, a seed starting at its own level and
//! an edge walkable from its tier's pass on (R1 section 4.3). Three modes:
//! [`Closure::reach_from`] is the planner's hoisted multi-source walk (DA.10),
//! [`Closure::reach`] the per-binding walk memoised per (set, source, direction,
//! depth), and [`Closure::reach_all`] the unbound fallback, one walk per indexed id
//! and stopped at exec's row budget.
//!
//! `scc(x, rep)` stays native Tarjan (R1 section 3: mutual reach is quadratic in
//! Datalog): every member `x` of a non-trivial strongly-connected component of the
//! set, with `rep` the member whose rendered key (then id) is smallest, so the
//! representative survives a checkout that re-salts NodeIds. A member's tier is the
//! first level at which it and `rep` reach each other; `rep`'s own row takes the best
//! cycle through it. Both walks stay inside the component (a path between two members
//! never leaves it), on a per-component index, so the whole call is O(V + E).
//!
//! Explain ([`Closure::path`], [`Closure::cycle`]) builds witnesses that do not follow
//! edge order, which follows path-derived NodeIds (R1 section 10): a one-level walk
//! at the row's tier with every depth layer ranked by rendered key picks each node's
//! parent, and the edge between a parent and its child is the walkable one with the
//! smallest (tier, category name, ordinal), so two parallel edges (CALLS beside USES)
//! never resolve by edge order. Hops run in edge direction and carry the GLOBAL edge
//! ordinal, which DB maps back to its `&Edge` for EVIDENCE.
//!
//! The index, the rank and the sorted edge pairs are built on first use; the memo is
//! a [`Lookup`], looked up and never iterated. An index whose kept edges disagree with
//! the tier slice yields no rows and counts an error; nothing here panics. The
//! counters ([`ClosureStats`]) feed DA.14b's
//! `[rules] closure cats=<names|*|carry> mode=<..> bfs=<n> rows=<r>` line.

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::reach::{LevelBfs, LevelWalk, bfs_levels};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_activation::profile::Registries;
use glia_core::{Edge, EdgeCategoryId, NodeId};

use crate::analysis::CatSet;
use crate::source::FactSource;
use crate::value::{Lookup, Tier};

/// The passes a closure walk runs: FACT, DERIVED, HEURISTIC.
const LEVELS: u8 = 3;
/// An edge level no pass walks.
const NEVER: u8 = u8::MAX;
/// Dense-index marker: in no component, or not reached.
const NONE: u32 = u32::MAX;

/// One edge of a witness, in edge direction: its ends, category, position in
/// [`GraphSource::edges`] and tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hop {
    pub(crate) from: NodeId,
    pub(crate) to: NodeId,
    pub(crate) category: EdgeCategoryId,
    pub(crate) ordinal: u32,
    pub(crate) tier: Tier,
}

/// One category set's counters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetStats {
    pub(crate) cats: CatSet,
    /// `bfs_levels` walks run, explain's ranked walks included.
    pub(crate) bfs: usize,
    /// Rows the walks yielded (a memo hit is not recounted).
    pub(crate) rows: usize,
    /// Rows the per-source memo holds: what exec's row budget counts (DA.14a).
    pub(crate) memo_rows: usize,
    pub(crate) memo_hits: usize,
    /// Calls per mode.
    pub(crate) multi: usize,
    pub(crate) per_source: usize,
    pub(crate) all: usize,
    pub(crate) scc: usize,
    pub(crate) explain: usize,
    /// Calls refused because the index and the tier slice disagree.
    pub(crate) errors: usize,
}

impl SetStats {
    fn new(cats: CatSet) -> Self {
        SetStats {
            cats,
            bfs: 0,
            rows: 0,
            memo_rows: 0,
            memo_hits: 0,
            multi: 0,
            per_source: 0,
            all: 0,
            scc: 0,
            explain: 0,
            errors: 0,
        }
    }

    /// `*`, `carry`, or the category names sorted and joined by `|`.
    pub(crate) fn label(&self, registries: &Registries) -> String {
        match &self.cats {
            CatSet::All => "*".to_string(),
            CatSet::Carry => "carry".to_string(),
            CatSet::Of(ids) => {
                let mut names: Vec<String> = ids.iter().map(|c| category_name(registries, *c)).collect();
                names.sort();
                names.dedup();
                names.join("|")
            }
        }
    }

    /// The evaluation modes the set ran in, `multi,per-source,all,scc` order.
    pub(crate) fn modes(&self) -> String {
        let used = [(self.multi, "multi"), (self.per_source, "per-source"), (self.all, "all"), (self.scc, "scc")];
        used.iter().filter(|(n, _)| *n > 0).map(|(_, m)| *m).collect::<Vec<_>>().join(",")
    }
}

/// Every category set's counters, in first-use order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ClosureStats {
    pub(crate) sets: Vec<SetStats>,
}

impl ClosureStats {
    /// Rows every per-source memo holds.
    pub(crate) fn memo_rows(&self) -> usize {
        self.sets.iter().map(|s| s.memo_rows).sum()
    }
}

/// A kept edge as explain reads it, sorted by (from, to, level, category-name
/// rank, kept position): the first entry of a node pair is its canonical hop.
#[derive(Clone, Copy, Debug)]
struct Pair {
    from: u32,
    to: u32,
    level: u8,
    name: u32,
    kept: u32,
    category: EdgeCategoryId,
}

/// One category set's index: the adjacency, each kept edge's tier level and global
/// ordinal (kept position = global edge order filtered by the set, the order
/// `Adjacency::build` keeps), and the explain-time tables built on first request.
struct CatIndex {
    keep: CategorySet,
    adj: Adjacency,
    kept_level: Vec<u8>,
    kept_ordinal: Vec<u32>,
    ok: bool,
    /// Dense index -> rank by (rendered key, id).
    rank: Option<Vec<u32>>,
    pairs: Option<Vec<Pair>>,
    /// Dense index -> its non-trivial component, or [`NONE`].
    comp_of: Option<Vec<u32>>,
}

impl CatIndex {
    fn build<S: FactSource + ?Sized>(src: &S, tiers: &[Tier], keep: CategorySet) -> Self {
        let adj = Adjacency::build(src, &keep);
        let mut kept_level = Vec::with_capacity(adj.kept_edges());
        let mut kept_ordinal = Vec::with_capacity(adj.kept_edges());
        let mut total = 0usize;
        for (i, e) in src.edges().enumerate() {
            total += 1;
            if keep.contains(e.category) {
                kept_level.push(tiers.get(i).map_or(NEVER, |t| t.level()));
                kept_ordinal.push(u32::try_from(i).unwrap_or(u32::MAX));
            }
        }
        let ok = tiers.len() == total && kept_level.len() == adj.kept_edges();
        if !ok {
            eprintln!(
                "[rules] closure: index mismatch ({} tiers for {total} edges, {} kept for {} indexed), the set yields no rows",
                tiers.len(),
                kept_level.len(),
                adj.kept_edges()
            );
        }
        CatIndex { keep, adj, kept_level, kept_ordinal, ok, rank: None, pairs: None, comp_of: None }
    }

    /// The three-pass `+` walk from `seeds` (each at its level).
    fn levelled(&self, seeds: &[(NodeId, u8)], backward: bool, depth: Option<u32>) -> LevelBfs {
        let mut how = LevelWalk::new(direction(backward), LEVELS);
        how.plus = true;
        how.max_depth = depth.map_or(usize::MAX, |d| usize::try_from(d).unwrap_or(usize::MAX));
        bfs_levels(&self.adj, seeds, &self.kept_level, &how)
    }

    /// Build the rank and the sorted pairs, once.
    fn explain_tables<S: FactSource + ?Sized>(&mut self, src: &S) {
        if self.rank.is_none() {
            let mut order: Vec<(String, u64, u32)> = (0..self.dense())
                .map(|ix| {
                    let id = self.adj.id(ix);
                    (src.render_node(id), id.0, ix)
                })
                .collect();
            order.sort_unstable();
            let mut rank = vec![0u32; order.len()];
            for (r, (_, _, ix)) in order.iter().enumerate() {
                rank[*ix as usize] = u32::try_from(r).unwrap_or(u32::MAX);
            }
            self.rank = Some(rank);
        }
        if self.pairs.is_none() {
            self.pairs = Some(self.sorted_pairs(src));
        }
    }

    fn sorted_pairs<S: FactSource + ?Sized>(&self, src: &S) -> Vec<Pair> {
        let mut out = Vec::with_capacity(self.kept_level.len());
        let mut kept = 0u32;
        for e in src.edges() {
            if !self.keep.contains(e.category) {
                continue;
            }
            if let (Some(from), Some(to)) = (self.adj.index_of(e.from), self.adj.index_of(e.to)) {
                let level = self.kept_level.get(kept as usize).copied().unwrap_or(NEVER);
                out.push(Pair { from, to, level, name: 0, kept, category: e.category });
            }
            kept += 1;
        }
        // Category-name rank, by id for the lookup.
        let registries = src.tables().registries;
        let mut ids: Vec<u32> = out.iter().map(|p| p.category.0).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut named: Vec<(String, u32)> =
            ids.iter().map(|&c| (category_name(&registries, EdgeCategoryId(c)), c)).collect();
        named.sort_unstable();
        let mut by_id: Vec<(u32, u32)> =
            named.iter().enumerate().map(|(r, (_, c))| (*c, u32::try_from(r).unwrap_or(u32::MAX))).collect();
        by_id.sort_unstable();
        for p in &mut out {
            p.name = by_id.binary_search_by_key(&p.category.0, |&(c, _)| c).map_or(u32::MAX, |i| by_id[i].1);
        }
        out.sort_unstable_by_key(|p| (p.from, p.to, p.level, p.name, p.kept));
        out
    }

    /// The canonical hop `from -> to` walkable at `level`.
    fn hop(&self, from: u32, to: u32, level: u8) -> Option<Hop> {
        let pairs = self.pairs.as_deref()?;
        let at = pairs.partition_point(|p| (p.from, p.to) < (from, to));
        let p = pairs.get(at).filter(|p| p.from == from && p.to == to && p.level <= level)?;
        Some(Hop {
            from: self.adj.id(from),
            to: self.adj.id(to),
            category: p.category,
            ordinal: self.kept_ordinal.get(p.kept as usize).copied().unwrap_or(u32::MAX),
            tier: Tier::from_level(p.level)?,
        })
    }

    /// A shortest `+` path at `level` from `seeds` to `target`, parents by rank,
    /// in edge direction (forward: seed first; backward: target first). Needs
    /// [`Self::explain_tables`].
    fn ranked_path(
        &self,
        seeds: &[NodeId],
        target: NodeId,
        level: u8,
        backward: bool,
        within: Option<&[bool]>,
    ) -> Option<Vec<Hop>> {
        let rank = self.rank.as_deref()?;
        let goal = self.adj.index_of(target)?;
        let one: Vec<u8> = self.kept_level.iter().map(|&l| if l <= level { 0 } else { NEVER }).collect();
        let mut how = LevelWalk::new(direction(backward), 1);
        how.plus = true;
        how.parent_rank = Some(rank);
        how.within = within;
        let starts: Vec<(NodeId, u8)> = seeds.iter().map(|&s| (s, 0)).collect();
        let walk = bfs_levels(&self.adj, &starts, &one, &how);
        let mut record = vec![NONE; self.adj.len()];
        for (i, r) in walk.reached.iter().enumerate() {
            if let Some(ix) = self.adj.index_of(r.id) {
                record[ix as usize] = u32::try_from(i).unwrap_or(NONE);
            }
        }
        // Each parent sits one layer up, so the chain ends at depth 1, on a seed.
        let mut hops = Vec::new();
        let mut at = goal;
        loop {
            let r = walk.reached.get(*record.get(at as usize)? as usize)?;
            let parent = self.adj.index_of(r.parent)?;
            let (from, to) = if backward { (at, parent) } else { (parent, at) };
            hops.push(self.hop(from, to, level)?);
            if r.depth <= 1 {
                break;
            }
            if hops.len() > self.adj.len() {
                return None;
            }
            at = parent;
        }
        if !backward {
            hops.reverse();
        }
        Some(hops)
    }

    fn dense(&self) -> u32 {
        u32::try_from(self.adj.len()).unwrap_or(u32::MAX)
    }
}

/// One component as its own graph: the members (ranked, so dense index = rank) and
/// the kept edges between them in kept order. Its index is the `within` mask made
/// physical: a walk on it costs O(component), where a masked walk over the set's
/// index would pay O(V) set-up per component.
struct Component<'e> {
    ids: Vec<NodeId>,
    edges: Vec<&'e Edge>,
}

impl GraphSource for Component<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.ids.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter().copied())
    }
}

/// A per-source walk: (set, source, backward, depth or `u32::MAX`).
type MemoKey = (usize, NodeId, bool, u32);

/// The closure builtins over one source, for one evaluation.
pub(crate) struct Closure<'s, S: FactSource + ?Sized> {
    src: &'s S,
    /// By global edge ordinal ([`crate::source::edge_tiers`]).
    tiers: &'s [Tier],
    sets: Vec<(CatSet, CatIndex)>,
    memo: Lookup<MemoKey, Vec<(NodeId, Tier)>>,
    stats: ClosureStats,
}

impl<'s, S: FactSource + ?Sized> Closure<'s, S> {
    pub(crate) fn new(src: &'s S, tiers: &'s [Tier]) -> Self {
        Closure { src, tiers, sets: Vec::new(), memo: Lookup::new(), stats: ClosureStats::default() }
    }

    /// The set's position, its index built on first use.
    fn set(&mut self, cats: &CatSet) -> usize {
        if let Some(i) = self.sets.iter().position(|(c, _)| c == cats) {
            return i;
        }
        let keep = match cats {
            CatSet::All => CategorySet::all(),
            CatSet::Carry => CategorySet::of(self.src.tables().carry_edges),
            CatSet::Of(ids) => CategorySet::of(ids),
        };
        self.sets.push((cats.clone(), CatIndex::build(self.src, self.tiers, keep)));
        self.stats.sets.push(SetStats::new(cats.clone()));
        self.sets.len() - 1
    }

    /// Multi-source: every node reachable from a seed (each seed at its own tier),
    /// with its tier, in walk order. A seed is a row only when a walk reaches it.
    pub(crate) fn reach_from(
        &mut self,
        cats: &CatSet,
        seeds: &[(NodeId, Tier)],
        backward: bool,
        depth: Option<u32>,
    ) -> Vec<(NodeId, Tier)> {
        let set = self.set(cats);
        let (index, stats) = (&self.sets[set].1, &mut self.stats.sets[set]);
        stats.multi += 1;
        if !index.ok {
            stats.errors += 1;
            return Vec::new();
        }
        let starts: Vec<(NodeId, u8)> = seeds.iter().map(|&(id, t)| (id, t.level())).collect();
        let rows = rows_of(&index.levelled(&starts, backward, depth));
        stats.bfs += 1;
        stats.rows += rows.len();
        rows
    }

    /// Per source, memoised: the rows of `reaches(source, _)` (backward:
    /// `reaches(_, source)`).
    pub(crate) fn reach(&mut self, cats: &CatSet, source: NodeId, backward: bool, depth: Option<u32>) -> &[(NodeId, Tier)] {
        let set = self.set(cats);
        let key = (set, source, backward, depth.unwrap_or(u32::MAX));
        let (index, stats) = (&self.sets[set].1, &mut self.stats.sets[set]);
        if self.memo.contains_key(&key) {
            stats.memo_hits += 1;
        } else {
            stats.per_source += 1;
            let rows = if index.ok {
                let rows = rows_of(&index.levelled(&[(source, 0)], backward, depth));
                stats.bfs += 1;
                stats.rows += rows.len();
                stats.memo_rows += rows.len();
                rows
            } else {
                stats.errors += 1;
                Vec::new()
            };
            self.memo.insert(key, rows);
        }
        self.memo.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every `(x, y, tier)` of a `reaches` with both ends free: one walk per indexed
    /// id (the nodes, then the dangling endpoints, in index order), not memoised.
    /// `None` as soon as the rows would pass `limit` (exec's remaining budget).
    pub(crate) fn reach_all(&mut self, cats: &CatSet, depth: Option<u32>, limit: usize) -> Option<Vec<(NodeId, NodeId, Tier)>> {
        let set = self.set(cats);
        let (index, stats) = (&self.sets[set].1, &mut self.stats.sets[set]);
        stats.all += 1;
        if !index.ok {
            stats.errors += 1;
            return Some(Vec::new());
        }
        let mut out = Vec::new();
        for ix in 0..index.dense() {
            let x = index.adj.id(ix);
            let walk = index.levelled(&[(x, 0)], false, depth);
            stats.bfs += 1;
            stats.rows += walk.reached.len();
            if out.len() + walk.reached.len() > limit {
                return None;
            }
            out.extend(rows_of(&walk).into_iter().map(|(y, t)| (x, y, t)));
        }
        Some(out)
    }

    /// Every `(x, rep, tier)` of `scc` over `cats`: components in Tarjan's order,
    /// members by rendered key; `rep` is the member with the smallest rendered key.
    pub(crate) fn scc_rows(&mut self, cats: &CatSet) -> Vec<(NodeId, NodeId, Tier)> {
        let set = self.set(cats);
        let src = self.src;
        let (index, stats) = (&self.sets[set].1, &mut self.stats.sets[set]);
        stats.scc += 1;
        if !index.ok {
            stats.errors += 1;
            return Vec::new();
        }
        let comps = strongly_connected(&index.adj);
        let comp_of = component_of(&index.adj, &comps);
        // The kept edges inside a component, bucketed by it, in kept order.
        let mut inside: Vec<Vec<(&Edge, u8)>> = vec![Vec::new(); comps.len()];
        let mut kept = 0usize;
        for e in src.edges() {
            if !index.keep.contains(e.category) {
                continue;
            }
            let level = index.kept_level.get(kept).copied().unwrap_or(NEVER);
            kept += 1;
            let (Some(f), Some(t)) = (index.adj.index_of(e.from), index.adj.index_of(e.to)) else { continue };
            let c = comp_of[f as usize];
            if c != NONE && c == comp_of[t as usize] {
                inside[c as usize].push((e, level));
            }
        }
        let mut rows = Vec::new();
        for (members, edges) in comps.iter().zip(inside) {
            let mut ranked: Vec<(String, u64)> = members.iter().map(|&id| (src.render_node(id), id.0)).collect();
            ranked.sort_unstable();
            let Some(&(_, rep)) = ranked.first() else { continue };
            let rep = NodeId(rep);
            let levels: Vec<u8> = edges.iter().map(|(_, l)| *l).collect();
            let sub = Component { ids: ranked.iter().map(|(_, id)| NodeId(*id)).collect(), edges: edges.into_iter().map(|(e, _)| e).collect() };
            let adj = Adjacency::build(&sub, &CategorySet::all());
            let mut how = LevelWalk::new(Walk::Forward, LEVELS);
            how.plus = true;
            let to = levels_by_index(&adj, &bfs_levels(&adj, &[(rep, 0)], &levels, &how));
            how.walk = Walk::Backward;
            let back = levels_by_index(&adj, &bfs_levels(&adj, &[(rep, 0)], &levels, &how));
            stats.bfs += 2;
            // Members are the sub-index's first ids, in rank order.
            for (ix, x) in sub.ids.iter().enumerate() {
                let level = if *x == rep { to[ix] } else { to[ix].max(back[ix]) };
                if let Some(tier) = Tier::from_level(level) {
                    rows.push((*x, rep, tier));
                }
            }
        }
        stats.rows += rows.len();
        rows
    }

    /// Explain: a shortest `+` path at `level` from the seeds to `target` (forward),
    /// or from `target` to a seed (backward), hops in edge direction, parents chosen
    /// by rendered rank. `None` when `target` is not reached at `level`.
    pub(crate) fn path(&mut self, cats: &CatSet, seeds: &[NodeId], target: NodeId, level: Tier, backward: bool) -> Option<Vec<Hop>> {
        let set = self.set(cats);
        self.stats.sets[set].explain += 1;
        if !self.sets[set].1.ok {
            self.stats.sets[set].errors += 1;
            return None;
        }
        self.sets[set].1.explain_tables(self.src);
        self.stats.sets[set].bfs += 1;
        self.sets[set].1.ranked_path(seeds, target, level.level(), backward, None)
    }

    /// Explain: a shortest cycle at `level` that starts and ends at `rep` and passes
    /// `x` (`x == rep`: a shortest cycle through `rep`), inside their component.
    /// Empty when they are not in one component, or not at `level`.
    pub(crate) fn cycle(&mut self, cats: &CatSet, x: NodeId, rep: NodeId, level: Tier) -> Vec<Hop> {
        let set = self.set(cats);
        self.stats.sets[set].explain += 1;
        if !self.sets[set].1.ok {
            self.stats.sets[set].errors += 1;
            return Vec::new();
        }
        let index = &mut self.sets[set].1;
        index.explain_tables(self.src);
        if index.comp_of.is_none() {
            index.comp_of = Some(component_of(&index.adj, &strongly_connected(&index.adj)));
        }
        let index = &self.sets[set].1;
        let comp_of = index.comp_of.as_deref().unwrap_or(&[]);
        let at = |id: NodeId| index.adj.index_of(id).and_then(|ix| comp_of.get(ix as usize).copied());
        let c = at(rep).unwrap_or(NONE);
        if c == NONE || at(x) != Some(c) {
            return Vec::new();
        }
        let mask: Vec<bool> = comp_of.iter().map(|&k| k == c).collect();
        let l = level.level();
        let walk = |from: NodeId, to: NodeId| index.ranked_path(&[from], to, l, false, Some(&mask));
        let hops = if x == rep {
            self.stats.sets[set].bfs += 1;
            walk(rep, rep)
        } else {
            self.stats.sets[set].bfs += 2;
            walk(rep, x).zip(walk(x, rep)).map(|(mut out, back)| {
                out.extend(back);
                out
            })
        };
        hops.unwrap_or_default()
    }

    /// The counters, per category set in first-use order.
    pub(crate) fn stats(&self) -> &ClosureStats {
        &self.stats
    }
}

fn direction(backward: bool) -> Walk {
    if backward { Walk::Backward } else { Walk::Forward }
}

/// A walk's rows: every reached node with its tier, in walk order.
fn rows_of(walk: &LevelBfs) -> Vec<(NodeId, Tier)> {
    walk.reached.iter().filter_map(|r| Tier::from_level(r.level).map(|t| (r.id, t))).collect()
}

/// Dense index -> the level a walk reached it at, [`NEVER`] when unreached.
fn levels_by_index(adj: &Adjacency, walk: &LevelBfs) -> Vec<u8> {
    let mut out = vec![NEVER; adj.len()];
    for r in &walk.reached {
        if let Some(ix) = adj.index_of(r.id) {
            out[ix as usize] = r.level;
        }
    }
    out
}

/// Dense index -> its component's position in `comps`, [`NONE`] outside them.
fn component_of(adj: &Adjacency, comps: &[Vec<NodeId>]) -> Vec<u32> {
    let mut out = vec![NONE; adj.len()];
    for (c, members) in comps.iter().enumerate() {
        for id in members {
            if let Some(ix) = adj.index_of(*id) {
                out[ix as usize] = u32::try_from(c).unwrap_or(NONE);
            }
        }
    }
    out
}

/// A category's registered name; an unregistered id renders `#<id>`.
fn category_name(registries: &Registries, c: EdgeCategoryId) -> String {
    registries.category_name(c).map_or_else(|| format!("#{}", c.0), str::to_string)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use glia_activation::profile::{DomainTables, EntryRule};
    use glia_core::{Confidence, NodeKindId};

    use super::*;
    use crate::source::{MemSource, edge_tiers};

    const FUNCTION: NodeKindId = NodeKindId(1);
    const CALLS: EdgeCategoryId = EdgeCategoryId(1);
    const USES: EdgeCategoryId = EdgeCategoryId(2);
    const IMPORTS: EdgeCategoryId = EdgeCategoryId(3);
    const F: Tier = Tier::Fact;
    const D: Tier = Tier::Derived;
    const H: Tier = Tier::Heuristic;
    const EVERY: [EdgeCategoryId; 3] = [CALLS, USES, IMPORTS];

    const TABLES: DomainTables = DomainTables {
        graph_type: "closure-test",
        registries: Registries {
            node_kinds: &[(FUNCTION, "FUNCTION")],
            edge_categories: &[(CALLS, "CALLS"), (USES, "USES"), (IMPORTS, "IMPORTS")],
            cell_types: &[],
        },
        entry: EntryRule { kinds: &[], roles: &[], named: &[] },
        carry_edges: &[CALLS, USES],
        effect_sinks: &[],
        activation_weights: &[],
        activation_presets: &[],
        community_weights: &[],
        identity_rules: &[],
    };

    /// `(from, to, category, tier)` over node indexes.
    type E = (u64, u64, EdgeCategoryId, Tier);

    /// Nodes `0..n` (id = index), rendered `n000`, `n001`, ..
    fn graph(n: u64, edges: &[E]) -> MemSource {
        graph_with(n, edges, |i| i, |i| format!("n{i:03}"))
    }

    /// Nodes `0..n` with id `id(i)` rendered `render(i)`; edge ends map through `id`.
    fn graph_with(n: u64, edges: &[E], id: impl Fn(u64) -> u64, render: impl Fn(u64) -> String) -> MemSource {
        let mut g = MemSource::new(TABLES);
        for i in 0..n {
            g.node(NodeId(id(i)), FUNCTION, render(i));
        }
        for &(a, b, c, t) in edges {
            g.edge(NodeId(id(a)), NodeId(id(b)), c, Confidence::Strong, t);
        }
        g
    }

    fn ids(rows: &[(NodeId, Tier)]) -> Vec<(u64, Tier)> {
        let mut out: Vec<(u64, Tier)> = rows.iter().map(|(id, t)| (id.0, *t)).collect();
        out.sort();
        out
    }

    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    /// `m` edges over `0..n`, categories and tiers mixed; with `dangling`, ~4% end
    /// on an id past `n` (no node).
    fn random_edges(seed: u64, n: u64, m: usize, dangling: bool) -> Vec<E> {
        let mut r = Lcg(seed);
        (0..m)
            .map(|_| {
                let a = r.below(n);
                let b = if dangling && r.below(25) == 0 { n + r.below(3) } else { r.below(n) };
                (a, b, EVERY[r.below(3) as usize], Tier::ALL[r.below(3) as usize])
            })
            .collect()
    }

    /// Brute force over the edge list: for each level L, what seeds of tier <= L
    /// reach by one or more hops of `keep` edges of tier <= L (within `depth` hops);
    /// a node's tier is the first such L.
    fn oracle(edges: &[E], seeds: &[(u64, Tier)], keep: &[EdgeCategoryId], backward: bool, depth: Option<u32>) -> Vec<(u64, Tier)> {
        let mut best: Vec<(u64, Tier)> = Vec::new();
        for level in Tier::ALL {
            let mut frontier: Vec<u64> = seeds.iter().filter(|s| s.1 <= level).map(|s| s.0).collect();
            let mut seen: HashSet<u64> = HashSet::new();
            let mut d = 0u32;
            while !frontier.is_empty() && depth.is_none_or(|m| d < m) {
                let mut next = Vec::new();
                for &u in &frontier {
                    for &(a, b, c, t) in edges {
                        let (from, to) = if backward { (b, a) } else { (a, b) };
                        if t <= level && keep.contains(&c) && from == u && seen.insert(to) {
                            next.push(to);
                        }
                    }
                }
                frontier = next;
                d += 1;
            }
            for v in seen {
                if !best.iter().any(|b| b.0 == v) {
                    best.push((v, level));
                }
            }
        }
        best.sort();
        best
    }

    /// Brute-force scc rows over node indexes: x is in a non-trivial component when
    /// it reaches itself; rep is the member with the smallest `render`; a member's
    /// tier is the first level at which it and rep reach each other (rep: the first
    /// level with a cycle through it).
    fn oracle_scc(nodes: &[u64], edges: &[E], render: &dyn Fn(u64) -> String) -> Vec<(u64, u64, Tier)> {
        let reach = |u: u64| oracle(edges, &[(u, F)], &EVERY, false, None);
        let level = |r: &[(u64, Tier)], v: u64| r.iter().find(|x| x.0 == v).map(|x| x.1);
        let reaches: Vec<Vec<(u64, Tier)>> = nodes.iter().map(|&u| reach(u)).collect();
        let at = |u: u64| nodes.iter().position(|&x| x == u).map(|i| &reaches[i]);
        let mut rows = Vec::new();
        for (i, &x) in nodes.iter().enumerate() {
            if level(&reaches[i], x).is_none() {
                continue;
            }
            let comp: Vec<u64> = nodes
                .iter()
                .copied()
                .filter(|&v| v == x || (level(&reaches[i], v).is_some() && at(v).and_then(|r| level(r, x)).is_some()))
                .collect();
            let rep = comp.iter().copied().min_by_key(|&v| (render(v), v)).unwrap_or(x);
            let rr = at(rep).map(Vec::as_slice).unwrap_or(&[]);
            let tier = if x == rep {
                level(rr, rep)
            } else {
                level(rr, x).zip(level(&reaches[i], rep)).map(|(a, b)| a.max(b))
            };
            rows.push((x, rep, tier.unwrap_or(H)));
        }
        rows.sort();
        rows
    }

    #[test]
    fn reach_from_matches_bfs_levels_semantics() {
        let mut tiers_seen = HashSet::new();
        for (seed, m) in (1..=6u64).zip([260, 260, 260, 150, 150, 150]) {
            let edges = random_edges(seed, 120, m, true);
            let g = graph(120, &edges);
            let tiers = edge_tiers(&g);
            let mut c = Closure::new(&g, &tiers);
            let mut r = Lcg(seed * 7919);
            let seeds = [(r.below(120), F), (r.below(120), D)];
            let starts: Vec<(NodeId, Tier)> = seeds.iter().map(|&(s, t)| (NodeId(s), t)).collect();
            for backward in [false, true] {
                for (cats, keep) in [(CatSet::All, &EVERY[..]), (CatSet::Of(vec![CALLS, USES]), &[CALLS, USES][..])] {
                    let rows = c.reach_from(&cats, &starts, backward, None);
                    assert_eq!(ids(&rows), oracle(&edges, &seeds, keep, backward, None), "seed {seed} backward {backward}");
                    assert_eq!(rows.iter().map(|r| r.0).collect::<HashSet<_>>().len(), rows.len(), "a node is one row");
                }
            }
            tiers_seen.extend(c.reach_from(&CatSet::All, &starts, false, None).iter().map(|r| r.1));
            assert_eq!(c.stats().sets.len(), 2, "one index per category set");
            assert_eq!(c.stats().sets[0].multi, 3);
            assert_eq!(c.stats().sets[0].bfs, 3);
        }
        assert_eq!(tiers_seen.len(), 3, "the corpus has rows at every tier");
    }

    #[test]
    fn carry_default_set() {
        let g = graph(6, &[(0, 1, CALLS, F), (1, 2, IMPORTS, F), (2, 3, USES, F), (0, 4, IMPORTS, F), (1, 5, USES, F)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        assert_eq!(ids(c.reach(&CatSet::Carry, NodeId(0), false, None)), vec![(1, F), (5, F)]);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, None)), vec![(1, F), (2, F), (3, F), (4, F), (5, F)]);
        assert_eq!(ids(c.reach(&CatSet::Of(vec![IMPORTS]), NodeId(1), false, None)), vec![(2, F)]);
        assert_eq!(c.stats().sets[0].label(&TABLES.registries), "carry");
        assert_eq!(c.stats().sets[1].label(&TABLES.registries), "*");
        assert_eq!(c.stats().sets[2].label(&TABLES.registries), "IMPORTS");
    }

    #[test]
    fn per_source_memo() {
        let g = graph(4, &[(0, 1, CALLS, F), (1, 2, CALLS, D), (2, 3, CALLS, H)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        let first = c.reach(&CatSet::All, NodeId(0), false, None).to_vec();
        let second = c.reach(&CatSet::All, NodeId(0), false, None).to_vec();
        assert_eq!(first, second);
        assert_eq!(ids(&first), vec![(1, F), (2, D), (3, H)]);
        let s = &c.stats().sets[0];
        assert_eq!((s.bfs, s.per_source, s.memo_hits, s.memo_rows), (1, 1, 1, 3));
        // Direction and depth are part of the key.
        c.reach(&CatSet::All, NodeId(0), true, None);
        c.reach(&CatSet::All, NodeId(0), false, Some(1));
        assert_eq!(c.stats().sets[0].bfs, 3);
        assert_eq!(c.stats().memo_rows(), 4);
        assert_eq!(c.stats().sets[0].modes(), "per-source");
    }

    #[test]
    fn depth_bound() {
        let chain = graph(4, &[(0, 1, CALLS, F), (1, 2, CALLS, F), (2, 3, CALLS, F)]);
        let tiers = edge_tiers(&chain);
        let mut c = Closure::new(&chain, &tiers);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, Some(2))), vec![(1, F), (2, F)]);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, Some(0))), vec![]);
        // Plus semantics: the source is a row only on a cycle.
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, None)), vec![(1, F), (2, F), (3, F)]);
        let cycle = graph(2, &[(0, 1, CALLS, F), (1, 0, CALLS, F)]);
        let tiers = edge_tiers(&cycle);
        let mut c = Closure::new(&cycle, &tiers);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, None)), vec![(0, F), (1, F)]);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, Some(1))), vec![(1, F)]);
    }

    #[test]
    fn backward() {
        let g = graph(4, &[(0, 1, CALLS, F), (1, 2, CALLS, D), (2, 3, CALLS, F)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(3), true, None)), vec![(0, D), (1, D), (2, F)]);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(3), false, None)), vec![]);
        // Backward witnesses run in edge direction, from the target.
        let hops = c.path(&CatSet::All, &[NodeId(3)], NodeId(0), D, true).unwrap_or_default();
        let ends: Vec<(u64, u64)> = hops.iter().map(|h| (h.from.0, h.to.0)).collect();
        assert_eq!(ends, vec![(0, 1), (1, 2), (2, 3)]);
        assert_eq!(c.path(&CatSet::All, &[NodeId(3)], NodeId(0), F, true), None);
    }

    #[test]
    fn scc_rows_match_tarjan() {
        let (mut tiers_seen, mut widest) = (HashSet::new(), 0usize);
        for seed in 1..=8u64 {
            let n = 40;
            let edges = random_edges(seed, n, 70, true);
            let render = |i: u64| if i < n { format!("r{:03}", (i * 37) % n) } else { format!("#{i:016x}") };
            let g = graph_with(n, &edges, |i| i, render);
            let tiers = edge_tiers(&g);
            let mut c = Closure::new(&g, &tiers);
            let mut rows: Vec<(u64, u64, Tier)> = c.scc_rows(&CatSet::All).iter().map(|(x, r, t)| (x.0, r.0, *t)).collect();
            rows.sort();
            let mut nodes: Vec<u64> = (0..n).collect();
            nodes.extend(edges.iter().flat_map(|e| [e.0, e.1]).filter(|&v| v >= n));
            nodes.sort_unstable();
            nodes.dedup();
            assert_eq!(rows, oracle_scc(&nodes, &edges, &render), "seed {seed}");
            // Membership is Tarjan's over the full graph.
            let comps = strongly_connected(&Adjacency::build(&g, &CategorySet::all()));
            let mut want: Vec<Vec<u64>> = comps.iter().map(|m| m.iter().map(|id| id.0).collect()).collect();
            let mut got: Vec<Vec<u64>> = Vec::new();
            for &(_, rep, _) in &rows {
                let mut m: Vec<u64> = rows.iter().filter(|r| r.1 == rep).map(|r| r.0).collect();
                m.sort_unstable();
                if !got.contains(&m) {
                    got.push(m);
                }
            }
            want.iter_mut().for_each(|m| m.sort_unstable());
            want.sort();
            got.sort();
            assert_eq!(got, want, "seed {seed}");
            tiers_seen.extend(rows.iter().map(|r| r.2));
            widest = widest.max(got.iter().map(Vec::len).max().unwrap_or(0));
        }
        assert_eq!(tiers_seen.len(), 3, "the corpus has members at every tier");
        assert!(widest >= 3, "the corpus has a component of three or more");
        // a <-> b at FACT; c only through the HEURISTIC b -> c.
        let g = graph(3, &[(0, 1, CALLS, F), (1, 0, CALLS, F), (1, 2, CALLS, H), (2, 0, CALLS, F)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        let mut rows = c.scc_rows(&CatSet::All);
        rows.sort_by_key(|r| r.0.0);
        assert_eq!(rows, vec![(NodeId(0), NodeId(0), F), (NodeId(1), NodeId(0), F), (NodeId(2), NodeId(0), H)]);
        assert_eq!(c.stats().sets[0].modes(), "scc");
    }

    #[test]
    fn self_loop_component() {
        let g = graph(2, &[(0, 1, CALLS, F), (0, 0, USES, D)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        assert_eq!(c.scc_rows(&CatSet::All), vec![(NodeId(0), NodeId(0), D)]);
        let hops = c.cycle(&CatSet::All, NodeId(0), NodeId(0), D);
        assert_eq!(hops, vec![Hop { from: NodeId(0), to: NodeId(0), category: USES, ordinal: 1, tier: D }]);
        assert!(c.cycle(&CatSet::All, NodeId(0), NodeId(0), F).is_empty());
        assert!(c.cycle(&CatSet::All, NodeId(1), NodeId(0), H).is_empty(), "1 is in no component");
        assert!(c.scc_rows(&CatSet::Carry).iter().any(|r| r.0 == NodeId(0)));
        assert!(c.scc_rows(&CatSet::Of(vec![CALLS])).is_empty());
    }

    /// Renders a path's hops.
    fn shown(g: &MemSource, hops: &[Hop]) -> Vec<(String, u32, String, Tier)> {
        hops.iter().map(|h| (g.render_node(h.from), h.category.0, g.render_node(h.to), h.tier)).collect()
    }

    #[test]
    fn canonical_path() {
        // s = 0, a = 1, b = 2, t = 3; b renders before a, a's edges come first.
        let render = |i: u64| ["s", "m_a", "k_b", "t"][i as usize].to_string();
        let forward = [(0, 1, CALLS, F), (1, 3, CALLS, F), (0, 2, CALLS, F), (2, 3, CALLS, F)];
        let mut reversed = forward;
        reversed.reverse();
        let mut seen = Vec::new();
        for edges in [&forward, &reversed] {
            let g = graph_with(4, edges, |i| i, render);
            let tiers = edge_tiers(&g);
            let mut c = Closure::new(&g, &tiers);
            let hops = c.path(&CatSet::All, &[NodeId(0)], NodeId(3), F, false).unwrap_or_default();
            assert_eq!(
                shown(&g, &hops),
                vec![("s".into(), CALLS.0, "k_b".into(), F), ("k_b".into(), CALLS.0, "t".into(), F)]
            );
            seen.push(shown(&g, &hops));
            assert_eq!(c.path(&CatSet::All, &[NodeId(0)], NodeId(0), H, false), None, "s is on no cycle");
            assert_eq!(c.stats().sets[0].explain, 2);
        }
        assert_eq!(seen[0], seen[1]);
    }

    #[test]
    fn cycle_is_valid_and_canonical() {
        let n = 30;
        let render = |i: u64| format!("q{:03}", (i * 7) % n);
        for seed in 11..=16u64 {
            let edges = random_edges(seed, n, 60, false);
            let mut reversed = edges.clone();
            reversed.reverse();
            let mut runs = Vec::new();
            for (list, salt) in [(&edges, 0u64), (&reversed, 0x9e37_79b9_7f4a_7c15)] {
                let id = move |i: u64| if salt == 0 { i } else { i.wrapping_mul(salt) ^ 0xabcdef };
                let g = graph_with(n, list, id, render);
                let tiers = edge_tiers(&g);
                let all: Vec<&Edge> = g.edges().collect();
                let mut c = Closure::new(&g, &tiers);
                let mut run = Vec::new();
                for (x, rep, tier) in c.scc_rows(&CatSet::All) {
                    let hops = c.cycle(&CatSet::All, x, rep, tier);
                    assert!(!hops.is_empty(), "seed {seed}: a cycle at the row's tier");
                    for h in &hops {
                        let e = all[h.ordinal as usize];
                        assert_eq!((e.from, e.to, e.category), (h.from, h.to, h.category), "a hop is its edge");
                        assert_eq!(tiers[h.ordinal as usize], h.tier);
                        assert!(h.tier <= tier);
                    }
                    assert!(hops.windows(2).all(|w| w[0].to == w[1].from), "hops chain");
                    assert_eq!((hops[0].from, hops[hops.len() - 1].to), (rep, rep));
                    assert!(hops.iter().any(|h| h.from == x));
                    if x == rep {
                        // A shortest cycle through rep at the tier.
                        let rep_ix = (0..n).find(|&i| NodeId(id(i)) == rep).unwrap_or(u64::MAX);
                        let shortest = (1..=n as u32)
                            .find(|&d| oracle(list, &[(rep_ix, F)], &EVERY, false, Some(d)).iter().any(|r| r.0 == rep_ix && r.1 <= tier));
                        assert_eq!(Some(hops.len() as u32), shortest);
                    }
                    if let Some(stricter) = tier.level().checked_sub(1).and_then(Tier::from_level) {
                        assert!(c.cycle(&CatSet::All, x, rep, stricter).is_empty(), "no cycle below the row's tier");
                    }
                    run.push((g.render_node(x), g.render_node(rep), tier, shown(&g, &hops)));
                }
                run.sort();
                runs.push(run);
            }
            assert!(!runs[0].is_empty(), "seed {seed} has a cycle");
            assert_eq!(runs[0], runs[1], "seed {seed}: re-salted ids and reversed edges give the same witnesses");
        }
    }

    #[test]
    fn dangling_endpoints() {
        let g = graph(2, &[(0, 99, CALLS, F), (99, 1, CALLS, D)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(0), false, None)), vec![(1, D), (99, F)]);
        assert_eq!(ids(c.reach(&CatSet::All, NodeId(99), true, None)), vec![(0, F)]);
        let hops = c.path(&CatSet::All, &[NodeId(0)], NodeId(1), D, false).unwrap_or_default();
        assert_eq!(hops.iter().map(|h| (h.from.0, h.to.0, h.ordinal)).collect::<Vec<_>>(), vec![(0, 99, 0), (99, 1, 1)]);
        // A dangling id with edges both ways is a component member like any node.
        let g = graph(1, &[(0, 99, CALLS, F), (99, 0, CALLS, F)]);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        let rep = NodeId(99); // `#0000000000000063` renders before `n000`
        let mut rows = c.scc_rows(&CatSet::All);
        rows.sort_by_key(|r| r.0.0);
        assert_eq!(rows, vec![(NodeId(0), rep, F), (rep, rep, F)]);
    }

    #[test]
    fn parallel_edges_pick_by_name() {
        let pick = |edges: &[E], level: Tier| {
            let g = graph(2, edges);
            let tiers = edge_tiers(&g);
            let mut c = Closure::new(&g, &tiers);
            c.path(&CatSet::All, &[NodeId(0)], NodeId(1), level, false).unwrap_or_default()
        };
        let hop = |category, ordinal, tier| vec![Hop { from: NodeId(0), to: NodeId(1), category, ordinal, tier }];
        assert_eq!(pick(&[(0, 1, USES, F), (0, 1, CALLS, F)], F), hop(CALLS, 1, F));
        assert_eq!(pick(&[(0, 1, CALLS, F), (0, 1, USES, F)], F), hop(CALLS, 0, F));
        assert_eq!(pick(&[(0, 1, CALLS, H), (0, 1, USES, F)], H), hop(USES, 1, F));
        assert_eq!(pick(&[(0, 1, USES, F), (0, 1, CALLS, H)], H), hop(USES, 0, F));
        // Only the HEURISTIC edge: no FACT path.
        assert_eq!(pick(&[(0, 1, CALLS, H)], F), vec![]);
        // Two call sites: the ordinal breaks the tie.
        assert_eq!(pick(&[(0, 1, CALLS, D), (0, 1, CALLS, D)], D), hop(CALLS, 0, D));
    }

    #[test]
    fn reach_all_is_every_source_within_the_budget() {
        let edges = random_edges(5, 25, 40, true);
        let g = graph(25, &edges);
        let tiers = edge_tiers(&g);
        let mut c = Closure::new(&g, &tiers);
        let mut all: Vec<(u64, u64, Tier)> =
            c.reach_all(&CatSet::All, None, usize::MAX).unwrap_or_default().iter().map(|(x, y, t)| (x.0, y.0, *t)).collect();
        all.sort();
        let mut want = Vec::new();
        let mut sources: Vec<u64> = (0..25).collect();
        sources.extend(edges.iter().map(|e| e.1).filter(|&v| v >= 25));
        sources.sort_unstable();
        sources.dedup();
        for x in sources {
            want.extend(oracle(&edges, &[(x, F)], &EVERY, false, None).into_iter().map(|(y, t)| (x, y, t)));
        }
        want.sort();
        assert!(!want.is_empty());
        assert_eq!(all, want);
        assert_eq!(c.reach_all(&CatSet::All, None, want.len() - 1), None, "past the budget");
        assert_eq!(c.reach_all(&CatSet::All, None, want.len()).map(|r| r.len()), Some(want.len()));
        assert_eq!(c.stats().sets[0].all, 3);
        assert_eq!(c.stats().sets[0].memo_rows, 0, "the unbound walk is not memoised");
    }

    #[test]
    fn a_tier_slice_that_disagrees_yields_nothing() {
        let g = graph(3, &[(0, 1, CALLS, F), (1, 0, CALLS, F)]);
        let short = vec![F];
        let mut c = Closure::new(&g, &short);
        assert!(c.reach(&CatSet::All, NodeId(0), false, None).is_empty());
        assert!(c.reach_from(&CatSet::All, &[(NodeId(0), F)], false, None).is_empty());
        assert!(c.scc_rows(&CatSet::All).is_empty());
        assert_eq!(c.path(&CatSet::All, &[NodeId(0)], NodeId(1), F, false), None);
        assert!(c.cycle(&CatSet::All, NodeId(0), NodeId(0), F).is_empty());
        assert_eq!(c.reach_all(&CatSet::All, None, 10), Some(Vec::new()));
        assert_eq!(c.stats().sets[0].errors, 6);
        assert_eq!(c.stats().sets[0].bfs, 0);
    }
}
