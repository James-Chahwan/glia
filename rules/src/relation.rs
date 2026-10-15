//! Relation storage for levelled semi-naive evaluation, datafrog-style: sorted,
//! deduplicated batches with permuted key indexes and (height, rule) provenance.
//!
//! A relation holds rows of `arity` value handles. A row is stored once, in the
//! partition of the level (0 FACT, 1 DERIVED, 2 HEURISTIC) whose round first derived
//! it, and never updated, so "visible at level <= L" picks partitions instead of
//! filtering rows. Every index keeps its own copy of the rows with the columns
//! permuted (position `i` holds relation column `perm[i]`; index 0 is the identity),
//! so a lookup key is a prefix of the permuted columns. A partition holds stable runs
//! (oldest first) and `recent`, the last round's new rows. Each batch is row-major,
//! sorted lexicographically and free of duplicates, with its rows' heights and rule
//! indexes in separate columns and, on the identity index of a loaded relation, the
//! witness edge ordinals ([`NO_ORDINAL`] for none): `arity` x 4 + 6 bytes a row per
//! index, plus 4 for an ordinal. Another index's scans report [`NO_ORDINAL`]; the
//! ordinal of a row read there comes from [`Relation::contains`].
//!
//! [`Relation::insert_round`] first advances, then drops every row already present
//! at any level, so `recent` is exactly the round's new rows. [`Relation::advance`]
//! turns `recent` into a stable run and merges trailing runs while the earlier is no
//! more than twice the later (datafrog's geometric merge: O(log n) runs a part). No
//! row is in two batches of one index, so merges never meet an equal pair. Every
//! index sees the same rows in the same run shapes, since run lengths alone decide a
//! merge. [`Cursor::seek`] gallops forward from its previous position in every run
//! while keys arrive in non-decreasing order (the merge-join path) and binary
//! searches otherwise. Handle order carries no meaning: nothing here orders output.
//! No hash map. Columns are `u8`, so a relation of more than 256 columns has the
//! identity index only.

use std::ops::RangeInclusive;

use crate::source::{EdbRows, NO_ORDINAL};
use crate::value::{Handle, Tier};

/// The rule of a row no rule derived: EDB, aggregate, builtin and scc rows.
pub(crate) const EDB_RULE: u16 = u16::MAX;

/// A row's provenance (Souffle's two columns): its derivation height (0 for a loaded
/// row unless the loader gives one) and the index of the rule that derived it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowMeta {
    pub(crate) height: u32,
    pub(crate) rule: u16,
}

/// The batches of a partition a view reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    /// The stable runs: every row from before the last round.
    Stable,
    /// The last round's new rows.
    Recent,
    Both,
}

/// What a scan or cursor sees: the partitions of `levels` (`1..=0` sees none), the
/// batches `which` picks in each, and, for explain, only heights strictly below
/// `max_height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct View {
    pub(crate) levels: RangeInclusive<u8>,
    pub(crate) which: Which,
    pub(crate) max_height: Option<u32>,
}

/// What a scan or seek calls per row: the permuted row, its level, its provenance
/// and its witness ordinal ([`NO_ORDINAL`] off the identity index).
pub(crate) type Visit<'f> = dyn FnMut(&[Handle], Tier, RowMeta, u32) + 'f;

/// One sorted, duplicate-free batch in its index's permuted column order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Batch {
    rows: Vec<Handle>,
    heights: Vec<u32>,
    rules: Vec<u16>,
    ordinals: Option<Vec<u32>>,
}

/// One level's rows in one index.
#[derive(Clone, Debug, Default)]
struct Part {
    stable: Vec<Batch>,
    recent: Batch,
}

impl Part {
    fn batches(&self) -> impl Iterator<Item = &Batch> {
        self.stable.iter().chain(std::iter::once(&self.recent))
    }
}

#[derive(Clone, Debug)]
struct Index {
    perm: Vec<u8>,
    parts: [Part; 3],
}

/// A relation: its indexes (index 0 the identity permutation) and its row count.
#[derive(Clone, Debug)]
pub(crate) struct Relation {
    arity: usize,
    indexes: Vec<Index>,
    len: usize,
}

/// A run a cursor reads: the batch, its level and the lower bound of the last key.
struct Run<'r> {
    batch: &'r Batch,
    tier: Tier,
    pos: usize,
}

/// Lookups by key prefix over one index under one view.
pub(crate) struct Cursor<'r> {
    arity: usize,
    runs: Vec<Run<'r>>,
    max_height: Option<u32>,
    last: Vec<Handle>,
    started: bool,
}

impl Batch {
    fn len(&self) -> usize {
        self.heights.len()
    }

    fn row(&self, arity: usize, i: usize) -> &[Handle] {
        &self.rows[i * arity..(i + 1) * arity]
    }

    fn meta(&self, i: usize) -> RowMeta {
        RowMeta { height: self.heights[i], rule: self.rules[i] }
    }

    fn ordinal(&self, i: usize) -> u32 {
        self.ordinals.as_ref().map_or(NO_ORDINAL, |o| o[i])
    }

    /// Row `i`'s first `key.len()` columns sort below `key` (`key.len() <= arity`).
    fn below(&self, arity: usize, i: usize, key: &[Handle]) -> bool {
        self.row(arity, i)[..key.len()] < *key
    }

    fn has_prefix(&self, arity: usize, i: usize, key: &[Handle]) -> bool {
        self.row(arity, i)[..key.len()] == *key
    }

    /// The first position in `lo..hi` not below `key`, given every row before `lo` is
    /// below it and none from `hi` on is.
    fn search(&self, arity: usize, mut lo: usize, mut hi: usize, key: &[Handle]) -> usize {
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.below(arity, mid, key) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// The first row whose prefix is not below `key`: a binary search.
    fn lower_bound(&self, arity: usize, key: &[Handle]) -> usize {
        self.search(arity, 0, self.len(), key)
    }

    /// [`Batch::lower_bound`] when every row before `from` is below `key`: doubling
    /// steps from `from`, then a binary search inside the last step.
    fn gallop(&self, arity: usize, from: usize, key: &[Handle]) -> usize {
        let n = self.len();
        if from >= n || !self.below(arity, from, key) {
            return from;
        }
        let (mut lo, mut step) = (from, 1);
        let hi = loop {
            let probe = lo + step;
            if probe >= n {
                break n;
            }
            if !self.below(arity, probe, key) {
                break probe;
            }
            lo = probe;
            step *= 2;
        };
        self.search(arity, lo + 1, hi, key)
    }

    fn push_from(&mut self, src: &Batch, arity: usize, i: usize) {
        self.rows.extend_from_slice(src.row(arity, i));
        self.heights.push(src.heights[i]);
        self.rules.push(src.rules[i]);
        if let Some(o) = &mut self.ordinals {
            o.push(src.ordinal(i));
        }
    }

    /// A batch of rows given in sorted order; ordinals kept when any row has one.
    fn collect<'a>(
        arity: usize,
        rows: impl Iterator<Item = (&'a [Handle], RowMeta, u32)>,
    ) -> Batch {
        let mut b = Batch::default();
        let mut ords = Vec::new();
        for (row, meta, ord) in rows {
            debug_assert_eq!(row.len(), arity);
            b.rows.extend_from_slice(row);
            b.heights.push(meta.height);
            b.rules.push(meta.rule);
            ords.push(ord);
        }
        b.ordinals = ords.iter().any(|&o| o != NO_ORDINAL).then_some(ords);
        b
    }

    /// This identity-order batch with its columns permuted by `perm` and re-sorted;
    /// ordinals stay on the identity index.
    fn permuted(&self, arity: usize, perm: &[u8]) -> Batch {
        let mut flat = Vec::with_capacity(self.rows.len());
        for i in 0..self.len() {
            let r = self.row(arity, i);
            flat.extend(perm.iter().map(|&c| r[usize::from(c)]));
        }
        let at = |i: usize| &flat[i * arity..(i + 1) * arity];
        let mut order: Vec<usize> = (0..self.len()).collect();
        order.sort_unstable_by(|&a, &b| at(a).cmp(at(b)));
        let mut out = Batch::default();
        for i in order {
            out.rows.extend_from_slice(at(i));
            out.heights.push(self.heights[i]);
            out.rules.push(self.rules[i]);
        }
        out
    }

    /// The sorted union of two disjoint batches.
    fn merge(a: Batch, b: Batch, arity: usize) -> Batch {
        if a.len() == 0 {
            return b;
        }
        if b.len() == 0 {
            return a;
        }
        let mut out = Batch {
            rows: Vec::with_capacity(a.rows.len() + b.rows.len()),
            heights: Vec::with_capacity(a.len() + b.len()),
            rules: Vec::with_capacity(a.len() + b.len()),
            ordinals: (a.ordinals.is_some() || b.ordinals.is_some())
                .then(|| Vec::with_capacity(a.len() + b.len())),
        };
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if j == b.len() || (i < a.len() && a.row(arity, i) < b.row(arity, j)) {
                out.push_from(&a, arity, i);
                i += 1;
            } else {
                out.push_from(&b, arity, j);
                j += 1;
            }
        }
        out
    }
}

/// Push `run` as the newest stable run, merging trailing runs while the earlier one
/// is no more than twice the later.
fn push_run(stable: &mut Vec<Batch>, mut run: Batch, arity: usize) {
    if run.len() == 0 {
        return;
    }
    while stable.last().is_some_and(|last| last.len() <= 2 * run.len()) {
        if let Some(last) = stable.pop() {
            run = Batch::merge(last, run, arity);
        }
    }
    stable.push(run);
}

/// The whole rows `handles` handles and `rows` metas (or levels) describe: a
/// caller's surplus handles or metas are ignored.
fn row_count(arity: usize, handles: usize, rows: usize) -> usize {
    handles.checked_div(arity).map_or(rows, |whole| rows.min(whole))
}

/// `perm` lists each of the `arity` columns exactly once.
fn is_perm(perm: &[u8], arity: usize) -> bool {
    let mut seen = vec![false; arity];
    perm.len() == arity
        && perm.iter().all(|&c| {
            let fresh = seen.get(usize::from(c)).is_some_and(|s| !s);
            if fresh {
                seen[usize::from(c)] = true;
            }
            fresh
        })
}

fn part_of(level: Tier) -> usize {
    usize::from(level.level())
}

impl Relation {
    /// An empty relation with the identity index and each distinct valid
    /// permutation of `perms` after it, in order (the plan's requests, DA.10). A list
    /// that is not a permutation of the columns gets no index ([`Relation::index_of`]
    /// says so).
    pub(crate) fn new(arity: usize, perms: &[Vec<u8>]) -> Self {
        let identity = (0..arity).map(|c| u8::try_from(c).unwrap_or(u8::MAX)).collect();
        let index = |perm| Index { perm, parts: Default::default() };
        let mut rel = Relation { arity, indexes: vec![index(identity)], len: 0 };
        for p in perms {
            if rel.index_of(p).is_none() && is_perm(p, arity) {
                rel.indexes.push(index(p.clone()));
            }
        }
        rel
    }

    /// The index whose permutation is `perm`.
    pub(crate) fn index_of(&self, perm: &[u8]) -> Option<usize> {
        self.indexes.iter().position(|ix| ix.perm == perm)
    }

    /// The index for `perm`, built on first request from the identity index (one sort
    /// a run, so runs stay aligned; a sealed relation has one a level) and kept.
    /// `None` when `perm` is not a permutation of the columns.
    pub(crate) fn ensure_index(&mut self, perm: &[u8]) -> Option<usize> {
        if let Some(i) = self.index_of(perm) {
            return Some(i);
        }
        if !is_perm(perm, self.arity) {
            return None;
        }
        let arity = self.arity;
        let id = &self.indexes[0].parts;
        let parts = std::array::from_fn(|l| Part {
            stable: id[l].stable.iter().map(|b| b.permuted(arity, perm)).collect(),
            recent: id[l].recent.permuted(arity, perm),
        });
        self.indexes.push(Index { perm: perm.to_vec(), parts });
        Some(self.indexes.len() - 1)
    }

    /// Keep the positions of `order` (rows of the flat `rows`, sorted) whose row is in
    /// no batch of the identity index: one galloping merge a batch.
    fn drop_present(&self, rows: &[Handle], order: &mut Vec<usize>) {
        let arity = self.arity;
        for b in self.indexes[0].parts.iter().flat_map(Part::batches) {
            let mut pos = 0;
            order.retain(|&i| {
                let row = &rows[i * arity..(i + 1) * arity];
                pos = b.gallop(arity, pos, row);
                !(pos < b.len() && b.row(arity, pos) == row)
            });
        }
    }

    /// Store `batch` (identity order) at `level` in every index: as stable runs, or
    /// as `recent`.
    fn store(&mut self, level: Tier, batch: Batch, recent: bool) {
        let (arity, l) = (self.arity, part_of(level));
        self.len += batch.len();
        for ix in self.indexes.iter_mut().skip(1) {
            let p = batch.permuted(arity, &ix.perm);
            let part = &mut ix.parts[l];
            if recent { part.recent = p } else { push_run(&mut part.stable, p, arity) }
        }
        let part = &mut self.indexes[0].parts[l];
        if recent { part.recent = batch } else { push_run(&mut part.stable, batch, arity) }
    }

    /// Load EDB, aggregate, builtin or scc rows as stable runs, one per level they
    /// name, with `heights` per row (0 when `None`; aggregates pass AggOutput.heights,
    /// DA.8), rule [`EDB_RULE`] and the witness ordinals (`rows.ordinals` may be
    /// shorter than the rows, the rest none). A row given twice keeps its best level,
    /// then its lowest height; a row the relation already holds is dropped. Returns
    /// the rows added: 0 for rows of another arity.
    pub(crate) fn load(&mut self, rows: EdbRows, heights: Option<&[u32]>) -> usize {
        let arity = self.arity;
        if rows.arity != arity {
            return 0;
        }
        let n = row_count(arity, rows.rows.len(), rows.levels.len());
        let row = |i: usize| &rows.rows[i * arity..(i + 1) * arity];
        let height = |i: usize| heights.and_then(|h| h.get(i)).copied().unwrap_or(0);
        let ord = |i: usize| rows.ordinals.get(i).copied().unwrap_or(NO_ORDINAL);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_unstable_by(|&a, &b| {
            row(a)
                .cmp(row(b))
                .then(rows.levels[a].cmp(&rows.levels[b]))
                .then(height(a).cmp(&height(b)))
                .then(ord(a).cmp(&ord(b)))
        });
        order.dedup_by(|later, earlier| row(*later) == row(*earlier));
        self.drop_present(&rows.rows, &mut order);
        let mut added = 0;
        for level in Tier::ALL {
            let picked = order.iter().filter(|&&i| rows.levels[i] == level);
            let batch = Batch::collect(
                arity,
                picked.map(|&i| (row(i), RowMeta { height: height(i), rule: EDB_RULE }, ord(i))),
            );
            added += batch.len();
            self.store(level, batch, false);
        }
        added
    }

    /// One round at `level`: advance, sort `to_add` (flat, `arity` handles a row,
    /// one meta each) by (row, height, rule), keep each row's first (lowest height,
    /// then lowest rule index: properties of the program and data, never of ids), drop
    /// every row present at any level, and store the survivors as the level's
    /// `recent` in every index. Empties both vectors (capacity kept); returns the
    /// survivors.
    pub(crate) fn insert_round(
        &mut self,
        level: Tier,
        to_add: &mut Vec<Handle>,
        metas: &mut Vec<RowMeta>,
    ) -> usize {
        self.advance();
        let arity = self.arity;
        let n = row_count(arity, to_add.len(), metas.len());
        let row = |i: usize| &to_add[i * arity..(i + 1) * arity];
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_unstable_by(|&a, &b| {
            row(a)
                .cmp(row(b))
                .then(metas[a].height.cmp(&metas[b].height))
                .then(metas[a].rule.cmp(&metas[b].rule))
        });
        order.dedup_by(|later, earlier| row(*later) == row(*earlier));
        self.drop_present(to_add, &mut order);
        let batch = Batch::collect(arity, order.iter().map(|&i| (row(i), metas[i], NO_ORDINAL)));
        let added = batch.len();
        self.store(level, batch, true);
        to_add.clear();
        metas.clear();
        added
    }

    /// Every part's `recent` becomes its newest stable run (geometric merge).
    pub(crate) fn advance(&mut self) {
        let arity = self.arity;
        for part in self.indexes.iter_mut().flat_map(|ix| ix.parts.iter_mut()) {
            let recent = std::mem::take(&mut part.recent);
            push_run(&mut part.stable, recent, arity);
        }
    }

    /// The stratum is done: advance, then merge every part to one stable run (none
    /// for an empty part).
    pub(crate) fn seal(&mut self) {
        self.advance();
        let arity = self.arity;
        for part in self.indexes.iter_mut().flat_map(|ix| ix.parts.iter_mut()) {
            if part.stable.len() > 1 {
                // Newest (smallest) first: runs shrink geometrically, so this is linear.
                let runs = std::mem::take(&mut part.stable).into_iter().rev();
                part.stable.extend(runs.reduce(|acc, earlier| Batch::merge(earlier, acc, arity)));
            }
        }
    }

    /// `row` (relation column order) at any level: its level, provenance and witness
    /// ordinal ([`NO_ORDINAL`] when none).
    pub(crate) fn contains(&self, row: &[Handle]) -> Option<(Tier, RowMeta, u32)> {
        if row.len() != self.arity {
            return None;
        }
        let arity = self.arity;
        for (l, part) in self.indexes[0].parts.iter().enumerate() {
            for b in part.batches() {
                let i = b.lower_bound(arity, row);
                if i < b.len() && b.row(arity, i) == row {
                    return Some((Tier::ALL[l], b.meta(i), b.ordinal(i)));
                }
            }
        }
        None
    }

    /// Some row at any level, stable or recent, has the permuted prefix `key` on
    /// `index` (negation: the trailing columns are wildcards).
    pub(crate) fn exists_prefix(&self, index: usize, key: &[Handle]) -> bool {
        let arity = self.arity;
        let Some(ix) = self.indexes.get(index).filter(|_| key.len() <= arity) else {
            return false;
        };
        ix.parts.iter().flat_map(Part::batches).any(|b| {
            let i = b.lower_bound(arity, key);
            i < b.len() && b.has_prefix(arity, i, key)
        })
    }

    /// The batches of `index` that `view` selects, with their levels: level order,
    /// stable runs oldest first, then recent.
    fn selected(&self, index: usize, view: &View) -> Vec<(&Batch, Tier)> {
        let mut out = Vec::new();
        let Some(ix) = self.indexes.get(index) else {
            return out;
        };
        for (l, part) in ix.parts.iter().enumerate() {
            let tier = Tier::ALL[l];
            if !view.levels.contains(&tier.level()) {
                continue;
            }
            if matches!(view.which, Which::Stable | Which::Both) {
                out.extend(part.stable.iter().map(|b| (b, tier)));
            }
            if matches!(view.which, Which::Recent | Which::Both) && part.recent.len() > 0 {
                out.push((&part.recent, tier));
            }
        }
        out
    }

    /// A cursor over `index` under `view`.
    pub(crate) fn cursor(&self, index: usize, view: View) -> Cursor<'_> {
        let runs = self.selected(index, &view).into_iter().map(|(batch, tier)| Run {
            batch,
            tier,
            pos: 0,
        });
        Cursor {
            arity: self.arity,
            runs: runs.collect(),
            max_height: view.max_height,
            last: Vec::new(),
            started: false,
        }
    }

    /// Every row of `index` under `view`, permuted, run by run (each run sorted):
    /// `f(row, level, meta, ordinal)`.
    pub(crate) fn scan(&self, index: usize, view: View, f: &mut Visit<'_>) {
        let arity = self.arity;
        for (b, tier) in self.selected(index, &view) {
            for i in 0..b.len() {
                if view.max_height.is_none_or(|h| b.heights[i] < h) {
                    f(b.row(arity, i), tier, b.meta(i), b.ordinal(i));
                }
            }
        }
    }

    /// Rows at every level: the row budget's count (DA.14b).
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Rows in the last round.
    pub(crate) fn recent_len(&self) -> usize {
        self.indexes[0].parts.iter().map(|p| p.recent.len()).sum()
    }
}

impl Cursor<'_> {
    /// `f(row, level, meta, ordinal)` for every row whose permuted prefix is `key`
    /// (none when `key` is longer than a row). A key not below the previous one
    /// gallops on from each run's previous position; any other binary searches.
    pub(crate) fn seek(&mut self, key: &[Handle], f: &mut Visit<'_>) {
        let (arity, max_height) = (self.arity, self.max_height);
        if key.len() > arity {
            return;
        }
        let forward = self.started && key >= self.last.as_slice();
        for run in &mut self.runs {
            let b = run.batch;
            run.pos =
                if forward { b.gallop(arity, run.pos, key) } else { b.lower_bound(arity, key) };
            let mut i = run.pos;
            while i < b.len() && b.has_prefix(arity, i, key) {
                if max_height.is_none_or(|h| b.heights[i] < h) {
                    f(b.row(arity, i), run.tier, b.meta(i), b.ordinal(i));
                }
                i += 1;
            }
        }
        self.last.clear();
        self.last.extend_from_slice(key);
        self.started = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Seen = (Vec<Handle>, Tier, RowMeta, u32);

    fn m(height: u32, rule: u16) -> RowMeta {
        RowMeta { height, rule }
    }

    fn view(levels: RangeInclusive<u8>, which: Which) -> View {
        View { levels, which, max_height: None }
    }

    fn all() -> View {
        view(0..=2, Which::Both)
    }

    /// splitmix64: deterministic test data without a crate.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) % n
        }
    }

    fn insert(rel: &mut Relation, level: Tier, rows: &[(&[Handle], u32, u16)]) -> usize {
        let mut to_add: Vec<Handle> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
        let mut metas: Vec<RowMeta> = rows.iter().map(|r| m(r.1, r.2)).collect();
        let n = rel.insert_round(level, &mut to_add, &mut metas);
        assert!(to_add.is_empty() && metas.is_empty());
        n
    }

    fn scanned(rel: &Relation, index: usize, v: View) -> Vec<Seen> {
        let mut out = Vec::new();
        rel.scan(index, v, &mut |r, t, meta, o| out.push((r.to_vec(), t, meta, o)));
        out
    }

    fn sorted(mut v: Vec<Seen>) -> Vec<Seen> {
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        v
    }

    fn permute(row: &[Handle], perm: &[u8]) -> Vec<Handle> {
        perm.iter().map(|&c| row[usize::from(c)]).collect()
    }

    /// Every batch strictly sorted with matching columns; every index holds the same
    /// run shapes; ordinals only on the identity index; `len` is the row total.
    fn check(rel: &Relation) {
        let a = rel.arity;
        let shape = |ix: &Index| -> Vec<Vec<usize>> {
            ix.parts.iter().map(|p| p.batches().map(Batch::len).collect()).collect()
        };
        for (k, ix) in rel.indexes.iter().enumerate() {
            assert_eq!(shape(ix), shape(&rel.indexes[0]), "index {k}");
            for b in ix.parts.iter().flat_map(Part::batches) {
                assert_eq!(b.rows.len(), b.len() * a);
                assert_eq!(b.rules.len(), b.len());
                assert!(k == 0 || b.ordinals.is_none());
                assert!(b.ordinals.as_ref().is_none_or(|o| o.len() == b.len()));
                for i in 1..b.len() {
                    assert!(b.row(a, i - 1) < b.row(a, i), "index {k} batch unsorted");
                }
            }
        }
        assert_eq!(scanned(rel, 0, all()).len(), rel.len());
    }

    #[test]
    fn insert_round_dedups_and_keeps_lowest_height() {
        let mut r = Relation::new(2, &[]);
        let n = insert(&mut r, Tier::Fact, &[(&[1, 2], 5, 0), (&[1, 2], 3, 1), (&[2, 3], 4, 0)]);
        assert_eq!(n, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r.recent_len(), 2);
        assert_eq!(
            scanned(&r, 0, all()),
            [
                (vec![1, 2], Tier::Fact, m(3, 1), NO_ORDINAL),
                (vec![2, 3], Tier::Fact, m(4, 0), NO_ORDINAL)
            ]
        );
        // At equal heights the lower rule index wins, whatever the input order.
        let n = insert(&mut r, Tier::Fact, &[(&[7, 7], 2, 4), (&[7, 7], 2, 3), (&[7, 7], 9, 0)]);
        assert_eq!(n, 1);
        assert_eq!(r.contains(&[7, 7]), Some((Tier::Fact, m(2, 3), NO_ORDINAL)));
        check(&r);
    }

    #[test]
    fn rows_at_a_lower_level_block_reinsertion() {
        let mut r = Relation::new(2, &[vec![1, 0]]);
        assert_eq!(insert(&mut r, Tier::Fact, &[(&[1, 2], 1, 0)]), 1);
        // Still recent at FACT: the next round advances it, then drops it.
        assert_eq!(insert(&mut r, Tier::Derived, &[(&[1, 2], 0, 0), (&[3, 4], 2, 1)]), 1);
        assert_eq!(r.contains(&[1, 2]), Some((Tier::Fact, m(1, 0), NO_ORDINAL)));
        assert_eq!(r.contains(&[3, 4]), Some((Tier::Derived, m(2, 1), NO_ORDINAL)));
        // Any level blocks: a DERIVED row is not re-derived at HEURISTIC, or at FACT.
        assert_eq!(insert(&mut r, Tier::Heuristic, &[(&[3, 4], 0, 0), (&[1, 2], 0, 0)]), 0);
        assert_eq!(insert(&mut r, Tier::Fact, &[(&[3, 4], 0, 0)]), 0);
        assert_eq!(r.contains(&[3, 4]), Some((Tier::Derived, m(2, 1), NO_ORDINAL)));
        assert_eq!(r.contains(&[4, 3]), None);
        assert_eq!(r.contains(&[1]), None);
        assert_eq!((r.len(), r.recent_len()), (2, 0));
        check(&r);
    }

    #[test]
    fn indexes_stay_consistent() {
        let mut r = Relation::new(2, &[vec![0, 1], vec![1, 0], vec![1, 0], vec![0, 0], vec![2, 0]]);
        assert_eq!(r.indexes.len(), 2, "identity deduped, invalid perms dropped");
        assert_eq!(
            (r.index_of(&[0, 1]), r.index_of(&[1, 0]), r.index_of(&[0, 0])),
            (Some(0), Some(1), None)
        );
        let mut rng = Rng(7);
        let views = [Which::Stable, Which::Recent, Which::Both]
            .into_iter()
            .flat_map(|w| [view(0..=0, w), view(1..=1, w), view(2..=2, w), view(0..=2, w)]);
        let views: Vec<View> = views.collect();
        for _ in 0..50 {
            if rng.below(4) == 0 {
                r.advance();
            } else {
                let level = Tier::ALL[rng.below(3) as usize];
                let mut to_add = Vec::new();
                let mut metas = Vec::new();
                for _ in 0..rng.below(20) {
                    to_add.extend([rng.below(12) as Handle, rng.below(12) as Handle]);
                    metas.push(m(rng.below(6) as u32, rng.below(3) as u16));
                }
                r.insert_round(level, &mut to_add, &mut metas);
            }
            check(&r);
            for v in &views {
                let want: Vec<Seen> = scanned(&r, 0, v.clone())
                    .into_iter()
                    .map(|(row, t, meta, _)| (permute(&row, &[1, 0]), t, meta, NO_ORDINAL))
                    .collect();
                assert_eq!(sorted(scanned(&r, 1, v.clone())), sorted(want), "{v:?}");
            }
        }
        assert!(r.len() > 50, "the rounds stored rows: {}", r.len());
    }

    #[test]
    fn seek_matches_brute_force() {
        let perms = [vec![0, 1, 2], vec![2, 0, 1]];
        let mut r = Relation::new(3, &perms);
        let mut rng = Rng(11);
        for round in 0..20 {
            let mut to_add = Vec::new();
            let mut metas = Vec::new();
            for _ in 0..100 {
                to_add.extend((0..3).map(|_| rng.below(14) as Handle));
                metas.push(m(rng.below(8) as u32, 0));
            }
            r.insert_round(Tier::ALL[round % 3], &mut to_add, &mut metas);
            if round % 4 == 3 {
                r.advance();
            }
        }
        check(&r);
        assert!(r.indexes[0].parts.iter().map(|p| p.stable.len()).sum::<usize>() > 3);
        let mut keys: Vec<Vec<Handle>> = (0..500)
            .map(|_| (0..1 + rng.below(2)).map(|_| rng.below(15) as Handle).collect())
            .collect();
        let shuffled = keys.clone();
        keys.sort();
        let v = View { levels: 0..=2, which: Which::Both, max_height: Some(6) };
        for perm in &perms {
            let ix = r.index_of(perm).expect("planned index");
            let rows = scanned(&r, ix, v.clone());
            for seq in [&keys, &shuffled] {
                let mut cur = r.cursor(ix, v.clone());
                let mut hits = 0;
                for key in seq.iter() {
                    let mut got = Vec::new();
                    cur.seek(key, &mut |row, t, meta, o| got.push((row.to_vec(), t, meta, o)));
                    let want: Vec<Seen> =
                        rows.iter().filter(|s| s.0.starts_with(key)).cloned().collect();
                    hits += want.len();
                    assert_eq!(sorted(got), sorted(want), "perm {perm:?} key {key:?}");
                }
                assert!(hits > 500, "the keys matched rows: {hits}");
            }
        }
        // A key longer than a row matches nothing.
        let mut cur = r.cursor(0, all());
        let mut n = 0;
        cur.seek(&[1, 2, 3, 4], &mut |_, _, _, _| n += 1);
        assert_eq!(n, 0);
    }

    /// The single-column rows `v` sees, sorted; a scan and an empty-key seek agree.
    fn seen(r: &Relation, v: View) -> Vec<Handle> {
        let mut a: Vec<Handle> = scanned(r, 0, v.clone()).into_iter().map(|s| s.0[0]).collect();
        let mut b = Vec::new();
        r.cursor(0, v).seek(&[], &mut |row, _, _, _| b.push(row[0]));
        a.sort();
        b.sort();
        assert_eq!(a, b, "scan and an empty-key seek agree");
        a
    }

    #[test]
    fn views_filter_levels_runs_and_height() {
        let mut r = Relation::new(1, &[]);
        insert(&mut r, Tier::Fact, &[(&[1], 0, 0), (&[2], 5, 0)]);
        insert(&mut r, Tier::Derived, &[(&[3], 3, 0), (&[4], 4, 0)]);
        insert(&mut r, Tier::Derived, &[(&[5], 7, 1), (&[6], 2, 1)]);
        assert_eq!(seen(&r, view(0..=0, Which::Both)), [1, 2]);
        assert_eq!(seen(&r, view(1..=1, Which::Both)), [3, 4, 5, 6]);
        assert_eq!(seen(&r, view(0..=2, Which::Recent)), [5, 6]);
        assert_eq!(seen(&r, view(0..=2, Which::Stable)), [1, 2, 3, 4]);
        assert_eq!(seen(&r, view(1..=2, Which::Stable)), [3, 4]);
        assert_eq!(seen(&r, view(0..=0, Which::Recent)), Vec::<Handle>::new());
        // `levels < FACT`, the Old view of FACT's first round: empty.
        assert_eq!(seen(&r, view(RangeInclusive::new(1, 0), Which::Both)), Vec::<Handle>::new());
        let low = View { levels: 0..=2, which: Which::Both, max_height: Some(4) };
        assert_eq!(seen(&r, low), [1, 3, 6]);
        let levels: Vec<Tier> = scanned(&r, 0, all()).into_iter().map(|s| s.1).collect();
        assert_eq!(
            levels,
            [Tier::Fact, Tier::Fact, Tier::Derived, Tier::Derived, Tier::Derived, Tier::Derived]
        );
        r.advance();
        assert_eq!(seen(&r, view(0..=2, Which::Recent)), Vec::<Handle>::new());
        assert_eq!(seen(&r, view(0..=2, Which::Stable)), [1, 2, 3, 4, 5, 6]);
        assert_eq!(r.recent_len(), 0);
    }

    #[test]
    fn merge_keeps_log_runs() {
        let mut r = Relation::new(1, &[vec![0]]);
        let mut most = 0;
        for i in 0..1_000u32 {
            insert(&mut r, Tier::Fact, &[(&[1_000 - i], i, 0)]);
            most = most.max(r.indexes[0].parts[0].stable.len());
        }
        r.advance();
        let runs = &r.indexes[0].parts[0].stable;
        assert!(runs.len() <= 11 && most <= 11, "{} runs, at most {most}", runs.len());
        for w in runs.windows(2) {
            assert!(w[0].len() > 2 * w[1].len(), "runs shrink geometrically");
        }
        assert_eq!(r.len(), 1_000);
        check(&r);
    }

    #[test]
    fn exists_prefix_any_level() {
        let mut r = Relation::new(2, &[vec![1, 0]]);
        assert!(!r.exists_prefix(0, &[]));
        insert(&mut r, Tier::Heuristic, &[(&[5, 6], 2, 0)]);
        let rev = r.index_of(&[1, 0]).expect("planned");
        for _ in 0..2 {
            assert!(r.exists_prefix(0, &[5]));
            assert!(r.exists_prefix(0, &[5, 6]));
            assert!(r.exists_prefix(0, &[]));
            assert!(r.exists_prefix(rev, &[6]));
            assert!(r.exists_prefix(rev, &[6, 5]));
            assert!(!r.exists_prefix(0, &[6]));
            assert!(!r.exists_prefix(0, &[5, 7]));
            assert!(!r.exists_prefix(rev, &[5]));
            assert!(!r.exists_prefix(0, &[5, 6, 0]));
            assert!(!r.exists_prefix(9, &[5]));
            r.advance(); // recent, then stable
        }
    }

    #[test]
    fn seal_merges_to_one_run() {
        let mut r = Relation::new(2, &[vec![1, 0]]);
        let mut rng = Rng(3);
        for round in 0..40 {
            let rows: Vec<(Vec<Handle>, u32, u16)> = (0..rng.below(9))
                .map(|_| (vec![rng.below(30) as Handle, rng.below(30) as Handle], round, 0))
                .collect();
            let rows: Vec<(&[Handle], u32, u16)> =
                rows.iter().map(|(a, h, k)| (a.as_slice(), *h, *k)).collect();
            insert(&mut r, Tier::ALL[(round / 14) as usize], &rows);
        }
        assert!(r.indexes[0].parts.iter().any(|p| p.stable.len() > 1));
        let before: Vec<Vec<Seen>> = (0..2).map(|ix| sorted(scanned(&r, ix, all()))).collect();
        let len = r.len();
        r.seal();
        for p in r.indexes.iter().flat_map(|ix| ix.parts.iter()) {
            let rows: usize = p.stable.iter().map(Batch::len).sum();
            assert_eq!(p.stable.len(), usize::from(rows > 0));
            assert_eq!(p.recent.len(), 0);
        }
        for (ix, want) in before.iter().enumerate() {
            assert_eq!(&sorted(scanned(&r, ix, all())), want);
            // One run a level: a scan is sorted within each level.
            for l in 0..=2u8 {
                let s = scanned(&r, ix, view(l..=l, Which::Stable));
                assert!(s.windows(2).all(|w| w[0].0 < w[1].0));
            }
        }
        assert_eq!((r.len(), r.recent_len()), (len, 0));
        check(&r);
    }

    /// EDB rows over two levels, with witness ordinals on some.
    fn edges() -> EdbRows {
        EdbRows {
            arity: 2,
            rows: vec![4, 1, 1, 2, 3, 9, 2, 2, 4, 1],
            levels: vec![Tier::Fact, Tier::Fact, Tier::Derived, Tier::Fact, Tier::Derived],
            ordinals: vec![7, 3, NO_ORDINAL, 12, 5],
        }
    }

    fn build(perms: &[Vec<u8>]) -> Relation {
        let mut r = Relation::new(2, perms);
        assert_eq!(r.load(edges(), None), 4, "(4,1) twice: FACT wins");
        insert(&mut r, Tier::Derived, &[(&[1, 2], 0, 0), (&[8, 1], 2, 1), (&[0, 5], 1, 2)]);
        insert(&mut r, Tier::Heuristic, &[(&[6, 6], 3, 0), (&[2, 0], 4, 1)]);
        r
    }

    #[test]
    fn ensure_index_matches_a_planned_index() {
        let mut planned = build(&[vec![0, 1], vec![1, 0]]);
        let mut late = build(&[vec![0, 1]]);
        planned.seal();
        late.seal();
        assert_eq!(late.index_of(&[1, 0]), None);
        assert_eq!(late.ensure_index(&[1, 0]), Some(1));
        check(&late);
        let views = [
            all(),
            view(0..=0, Which::Stable),
            view(1..=1, Which::Both),
            view(2..=2, Which::Stable),
        ];
        for v in views {
            assert_eq!(scanned(&late, 1, v.clone()), scanned(&planned, 1, v.clone()), "{v:?}");
        }
        let kept = late.indexes[1].parts[0].stable[0].rows.as_ptr();
        assert_eq!(late.ensure_index(&[1, 0]), Some(1));
        assert_eq!(late.ensure_index(&[0, 1]), Some(0));
        assert_eq!(late.indexes.len(), 2);
        assert_eq!(late.indexes[1].parts[0].stable[0].rows.as_ptr(), kept, "not rebuilt");
        assert_eq!((late.ensure_index(&[0, 0]), late.ensure_index(&[1])), (None, None));
        // The identity index keeps the witness ordinals; the permuted one reports none.
        assert_eq!(late.contains(&[4, 1]), Some((Tier::Fact, m(0, EDB_RULE), 7)));
        assert_eq!(late.contains(&[2, 2]), Some((Tier::Fact, m(0, EDB_RULE), 12)));
        assert_eq!(late.contains(&[3, 9]), Some((Tier::Derived, m(0, EDB_RULE), NO_ORDINAL)));
        assert_eq!(late.contains(&[8, 1]), Some((Tier::Derived, m(2, 1), NO_ORDINAL)));
        assert_eq!(late.contains(&[1, 2]), Some((Tier::Fact, m(0, EDB_RULE), 3)), "loaded first");
        let mut cur = late.cursor(1, all());
        let mut got = Vec::new();
        cur.seek(&[1], &mut |row, t, meta, o| got.push((row.to_vec(), t, meta, o)));
        assert_eq!(
            got,
            [
                (vec![1, 4], Tier::Fact, m(0, EDB_RULE), NO_ORDINAL),
                (vec![1, 8], Tier::Derived, m(2, 1), NO_ORDINAL)
            ]
        );
        // On an unsealed relation the new index copies the run shapes, recent included.
        let mut open = build(&[]);
        assert_eq!(open.ensure_index(&[1, 0]), Some(1));
        check(&open);
        assert_eq!(open.recent_len(), 2);
        assert_eq!(scanned(&open, 1, view(0..=2, Which::Recent)).len(), 2);
    }

    #[test]
    fn load_partitions_by_level_with_heights() {
        let mut r = Relation::new(2, &[vec![1, 0]]);
        insert(&mut r, Tier::Fact, &[(&[2, 2], 1, 0)]);
        // An aggregate's rows: heights given, no ordinals; (2,2) is already present.
        let agg = EdbRows {
            arity: 2,
            rows: vec![5, 6, 2, 2, 7, 8, 5, 6],
            levels: vec![Tier::Heuristic, Tier::Derived, Tier::Derived, Tier::Derived],
            ordinals: Vec::new(),
        };
        assert_eq!(r.load(agg, Some(&[4, 1, 2, 3])), 2);
        assert_eq!(r.contains(&[5, 6]), Some((Tier::Derived, m(3, EDB_RULE), NO_ORDINAL)));
        assert_eq!(r.contains(&[7, 8]), Some((Tier::Derived, m(2, EDB_RULE), NO_ORDINAL)));
        assert_eq!(r.contains(&[2, 2]), Some((Tier::Fact, m(1, 0), NO_ORDINAL)));
        assert_eq!((r.len(), r.recent_len()), (3, 1), "a load leaves recent alone");
        let wrong = EdbRows { arity: 3, ..EdbRows::default() };
        assert_eq!(r.load(wrong, None), 0);
        check(&r);
    }

    #[test]
    fn nullary_relation_holds_one_row() {
        let mut r = Relation::new(0, &[vec![]]);
        assert_eq!(r.indexes.len(), 1);
        assert_eq!(r.contains(&[]), None);
        let (mut to_add, mut metas) = (Vec::new(), vec![m(4, 2), m(1, 3), m(1, 5)]);
        assert_eq!(r.insert_round(Tier::Derived, &mut to_add, &mut metas), 1);
        assert_eq!(r.contains(&[]), Some((Tier::Derived, m(1, 3), NO_ORDINAL)));
        assert!(r.exists_prefix(0, &[]));
        let mut metas = vec![m(0, 0)];
        assert_eq!(r.insert_round(Tier::Fact, &mut to_add, &mut metas), 0);
        assert_eq!(r.len(), 1);
        check(&r);
    }
}
