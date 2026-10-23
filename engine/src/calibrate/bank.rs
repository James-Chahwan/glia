//! The structure bank (EC.4, items 10 and 2): the replay's text-free file
//! rankers, scored from the arithmetic the answers run (EA.4's
//! [`rank::base_keys`]) so the calibration picks among exactly what an answer
//! applies.
//!
//! - `P1`: undirected PPR at CODE_TABLES' damping (0.5), seeded uniformly over
//!   every node of the seed files, node scores max-pooled per file
//!   (rank_eval `run_query`'s `Ppr::uniform` + `file_pool(.., false)`), key
//!   `[s, 0, 0]`.
//! - `PT*`: the same at [`rank::WIDE_DAMPING`], sum-pooled (`base,d85`).
//! - `B0`: carry-edge BFS both ways, unbounded depth, key `[-min depth (or
//!   -1e9 unreached), file degree, 0]` (`bfs_key`).
//! - `H10` / `H10[PT*]` / `H10[B0]`: co-change first, `[C0, P1, 0]`,
//!   `[C0, PT*, 0]` (`lex2`), `[C0, -depth, degree]` (r3's `H10[B0]`), C0 the
//!   query's prefix co-change (EC.2). The composition is
//!   [`rank::rank_files_with_c0`]'s step d, built here from the memoised base
//!   keys so one query pays one PPR per damping; the bank test pins the two
//!   equal.
//!
//! A find query (EC.13) seeds by its text's top files, each node of file f
//! weighted `w_f / |nodes(f)|` (`r3_transfer`'s find rule); every other shape
//! seeds uniformly. The bank is a [`StateHook`] of the replay: register it with
//! [`super::ReplayState::register`], and a candidate reads it through
//! [`structure_keys`].
//!
//! Fired_on, stderr, once per [`StructIndex::build`]: `[calibrate] bank
//! structure files=<f> nodes=<n> ppr_indexes=2 carry_edges=<e>`; when a
//! candidate position names no file of the rank cache's file space:
//! `[calibrate] bank structure unmapped=<u> of <f>` (those files read 0 /
//! unreached).

use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;

use glia_activation::{ActivationConfig, Direction};
use glia_code_domain::profile::CODE_TABLES;
use glia_core::NodeId;
use glia_graph::MergedGraph;

use super::candidate::{QueryCtx, RankKey, StateHook};
use super::record::Shape;
use crate::intent::text::Candidates;
use crate::rank::{FileIndex, RankBase, RankCache, SourceKey, base_keys};

/// The structure bank of one calibrate run: the rank cache whose PPR indexes,
/// carry adjacency and file space every query reads, and the candidate
/// positions' files in that space.
#[non_exhaustive]
pub struct StructIndex {
    cache: Arc<RankCache>,
    files: Arc<FileIndex>,
    /// Per candidate position, its file in `files` (None when the position
    /// names no file there).
    pos_file: Vec<Option<u32>>,
}

impl std::fmt::Debug for StructIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructIndex")
            .field("candidates", &self.pos_file.len())
            .field("files", &self.files.len())
            .finish()
    }
}

impl StateHook for StructIndex {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The PPR config of a PPR base, as EA.4's `ppr_config` builds it (so the
/// cache lookup hits the index the answers use): CODE_TABLES' config,
/// undirected, unbounded top_k, the base's damping.
fn ppr_config(base: RankBase) -> Option<ActivationConfig> {
    let damping = base.damping()?;
    let mut cfg = CODE_TABLES.activation_config(None);
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    cfg.damping = damping;
    Some(cfg)
}

impl StructIndex {
    /// The bank over `merged` for the candidate files `cands` (EB.3's view
    /// over `cache.files(merged)`). Builds both PPR indexes and the carry
    /// adjacency in `cache` now, so the replay's queries only read them.
    pub fn build(merged: &MergedGraph, cands: &Candidates, cache: Arc<RankCache>) -> StructIndex {
        let files = cache.files(merged);
        let pos_file: Vec<Option<u32>> = cands
            .files
            .iter()
            .zip(&cands.paths)
            .map(|(&f, p)| ((f as usize) < files.len() && files.path(f) == p.as_str()).then_some(f))
            .collect();
        let mut indexes = 0usize;
        for base in [RankBase::Ppr, RankBase::PprWide] {
            if let Some(cfg) = ppr_config(base) {
                cache.ppr_index(merged, SourceKey::Merged, &cfg);
                indexes += 1;
            }
        }
        let carry = cache.carry(merged);
        let nodes: usize = pos_file.iter().flatten().map(|&f| files.nodes(f).len()).sum();
        let unmapped = pos_file.iter().filter(|f| f.is_none()).count();
        eprintln!(
            "[calibrate] bank structure files={} nodes={nodes} ppr_indexes={indexes} carry_edges={}",
            pos_file.len(),
            carry.kept_edges()
        );
        if unmapped > 0 {
            eprintln!(
                "[calibrate] bank structure unmapped={unmapped} of {}",
                pos_file.len()
            );
        }
        StructIndex {
            cache,
            files,
            pos_file,
        }
    }

    /// The candidate files.
    pub fn len(&self) -> usize {
        self.pos_file.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos_file.is_empty()
    }

    /// The nodes of candidate position `p`, in the file space's node order.
    fn nodes_at(&self, p: u32) -> &[NodeId] {
        match self.pos_file.get(p as usize).copied().flatten() {
            Some(f) => self.files.nodes(f),
            None => &[],
        }
    }

    /// Candidate position `p`'s file degree (rank_eval `file_deg`).
    fn degree_at(&self, p: usize) -> f64 {
        self.pos_file
            .get(p)
            .copied()
            .flatten()
            .map_or(0.0, |f| self.files.degree(f))
    }

    /// A per-file-space key vector read back per candidate position.
    fn by_position(&self, keys: &[RankKey]) -> Vec<RankKey> {
        self.pos_file
            .iter()
            .map(|f| match f {
                Some(f) => keys.get(*f as usize).copied().unwrap_or([0.0; 3]),
                None => [0.0; 3],
            })
            .collect()
    }
}

/// `Ppr::uniform` over every node of the seed files (candidate positions),
/// flattened in seed order, first occurrence kept: weight 1 / the seeds the
/// P1 index holds, as [`crate::rank::rank_files_with_c0`] weighs them.
pub fn uniform_seed_nodes(idx: &StructIndex, merged: &MergedGraph, seed_files: &[u32]) -> Vec<(NodeId, f64)> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut seeds: Vec<NodeId> = Vec::new();
    for &p in seed_files {
        for &u in idx.nodes_at(p) {
            if seen.insert(u) {
                seeds.push(u);
            }
        }
    }
    let held = match ppr_config(RankBase::Ppr) {
        Some(cfg) => {
            let ix = idx.cache.ppr_index(merged, SourceKey::Merged, &cfg);
            seeds.iter().filter(|&&u| ix.index_of(u).is_some()).count()
        }
        None => seeds.len(),
    };
    let w = if held == 0 { 0.0 } else { 1.0 / held as f64 };
    seeds.into_iter().map(|u| (u, w)).collect()
}

/// `r3_transfer`'s find seeding: each (candidate position, weight) spreads
/// `w / |nodes|` over the file's nodes.
pub fn weighted_seed_nodes(idx: &StructIndex, top: &[(u32, f64)]) -> Vec<(NodeId, f64)> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for &(p, w) in top {
        let ns = idx.nodes_at(p);
        if ns.is_empty() {
            continue;
        }
        let each = w / ns.len() as f64;
        for &u in ns {
            if seen.insert(u) {
                out.push((u, each));
            }
        }
    }
    out
}

/// The pooled PPR score per candidate position: `P1` (damping 0.5, max pool)
/// or, `wide`, `PT*` (damping 0.85, sum pool). An empty or unheld
/// personalisation scores 0 everywhere.
pub fn ppr_files(idx: &StructIndex, merged: &MergedGraph, pers: &[(NodeId, f64)], wide: bool) -> Vec<f64> {
    let base = if wide { RankBase::PprWide } else { RankBase::Ppr };
    idx.by_position(&base_keys(merged, &idx.cache, pers, base))
        .into_iter()
        .map(|k| k[0])
        .collect()
}

/// `B0`'s key per candidate position: `[-min depth or -1e9, degree, 0]` from
/// a carry-edge BFS both ways from `seeds`.
pub fn bfs_files(idx: &StructIndex, merged: &MergedGraph, seeds: &[NodeId]) -> Vec<RankKey> {
    let pers: Vec<(NodeId, f64)> = seeds.iter().map(|&u| (u, 0.0)).collect();
    idx.by_position(&base_keys(merged, &idx.cache, &pers, RankBase::Bfs))
}

/// The query's personalisation: weighted for a find query, else uniform.
fn query_pers(idx: &StructIndex, q: &QueryCtx<'_>) -> Vec<(NodeId, f64)> {
    if q.query.shape == Shape::Find {
        let top: Vec<(u32, f64)> = q
            .query
            .seeds
            .iter()
            .enumerate()
            .map(|(i, &f)| (f, q.query.weights.get(i).copied().unwrap_or(0.0)))
            .collect();
        weighted_seed_nodes(idx, &top)
    } else {
        uniform_seed_nodes(idx, q.merged, &q.query.seeds)
    }
}

/// The keys of the structure ranker `name` (`P1`, `PT*`, `B0`, `H10`,
/// `H10[PT*]`, `H10[B0]`) for one replayed query, one per candidate position;
/// None for any other name or with no [`StructIndex`] registered. The
/// signals are memoised under `p1`, `pt`, `b0` (B0's depth component) and
/// `c0`, so every candidate of every set pays once per query.
pub fn structure_keys(name: &str, q: &QueryCtx<'_>) -> Option<Vec<RankKey>> {
    if !matches!(name, "P1" | "PT*" | "B0" | "H10" | "H10[PT*]" | "H10[B0]") {
        return None;
    }
    let idx = q.state.hook::<StructIndex>()?;
    let m = idx.len();
    let p1 = || {
        q.memo.get_or("p1", || {
            ppr_files(idx, q.merged, &query_pers(idx, q), false)
        })
    };
    let pt = || {
        q.memo.get_or("pt", || {
            ppr_files(idx, q.merged, &query_pers(idx, q), true)
        })
    };
    let b0 = || {
        q.memo.get_or("b0", || {
            let seeds: Vec<NodeId> = query_pers(idx, q).into_iter().map(|(u, _)| u).collect();
            bfs_files(idx, q.merged, &seeds).into_iter().map(|k| k[0]).collect()
        })
    };
    let c0 = || {
        q.memo.get_or("c0", || {
            let mut v = vec![0.0; m];
            for &(f, s) in &q.query.c0 {
                if let Some(x) = v.get_mut(f as usize) {
                    *x = s;
                }
            }
            v
        })
    };
    let at = |v: &[f64], p: usize| v.get(p).copied().unwrap_or(0.0);
    let keys: Vec<RankKey> = match name {
        "P1" => {
            let p = p1();
            (0..m).map(|i| [at(&p, i), 0.0, 0.0]).collect()
        }
        "PT*" => {
            let p = pt();
            (0..m).map(|i| [at(&p, i), 0.0, 0.0]).collect()
        }
        "B0" => {
            let d = b0();
            (0..m).map(|i| [at(&d, i), idx.degree_at(i), 0.0]).collect()
        }
        "H10" => {
            let (c, p) = (c0(), p1());
            (0..m).map(|i| [at(&c, i), at(&p, i), 0.0]).collect()
        }
        "H10[PT*]" => {
            let (c, p) = (c0(), pt());
            (0..m).map(|i| [at(&c, i), at(&p, i), 0.0]).collect()
        }
        _ => {
            let (c, d) = (c0(), b0());
            (0..m)
                .map(|i| [at(&c, i), at(&d, i), idx.degree_at(i)])
                .collect()
        }
    };
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibrate::candidate::{Memo, ReplayState};
    use crate::calibrate::replay::{ReplayCommit, ReplayQuery};
    use crate::calibrate::select::better;
    use crate::rank::{Ranking, rank_files_with_c0};
    use std::path::Path;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    /// Six python files: a -> b -> c chain, d and e both import c, and f
    /// stands alone (unreached by any walk from a).
    fn toy() -> (tempfile::TempDir, MergedGraph, Arc<RankCache>, Candidates) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("toy");
        write(&root, "a.py", "from b import bf\n\ndef af():\n    return bf()\n");
        write(&root, "b.py", "from c import cf\n\ndef bf():\n    return cf()\n\ndef bg():\n    return 2\n");
        write(&root, "c.py", "def cf():\n    return 1\n");
        write(&root, "d.py", "from c import cf\n\ndef df():\n    return cf()\n");
        write(&root, "e.py", "from c import cf\n\ndef ef():\n    return cf()\n");
        write(&root, "f.py", "def ff():\n    return 3\n");
        let r = crate::generate_one(root.to_str().expect("utf-8")).expect("build");
        let cache = Arc::new(RankCache::new("calibrate", &r.repo_roots, &r.repo_labels));
        let files = cache.files(&r.merged);
        let cands = Candidates::of(&files, files.repo(0));
        (tmp, r.merged, cache, cands)
    }

    fn query(seeds: Vec<u32>, c0: Vec<(u32, f64)>) -> ReplayQuery {
        ReplayQuery {
            tc: 0,
            commit: 0,
            shape: Shape::from_seed_files(seeds.len()),
            weights: vec![1.0; seeds.len()],
            seeds,
            targets: vec![],
            c0,
        }
    }

    fn keys(name: &str, merged: &MergedGraph, cands: &Candidates, state: &ReplayState, q: &ReplayQuery, memo: &Memo) -> Vec<RankKey> {
        let excluded = vec![false; cands.len()];
        let commit = ReplayCommit::default();
        let ctx = QueryCtx {
            merged,
            cands,
            commit: &commit,
            query: q,
            excluded: &excluded,
            state,
            memo,
            repo_root: Path::new("."),
        };
        structure_keys(name, &ctx).expect(name)
    }

    #[test]
    fn bank_keys_are_the_answers_arithmetic() {
        let (_tmp, merged, cache, cands) = toy();
        assert_eq!(cands.paths, ["a.py", "b.py", "c.py", "d.py", "e.py", "f.py"]);
        let mut state = ReplayState::new(cands.len(), 30);
        state.register(Box::new(StructIndex::build(&merged, &cands, Arc::clone(&cache))));
        let pos = |p: &str| cands.position(p).expect(p) as u32;
        let (a, c, d, f) = (pos("a.py"), pos("c.py"), pos("d.py"), pos("f.py"));
        // d and e tie on structure from a (both import c); d co-changed.
        let q = query(vec![a], vec![(d, 0.5)]);
        let memo = Memo::new();
        let p1 = keys("P1", &merged, &cands, &state, &q, &memo);
        let pt = keys("PT*", &merged, &cands, &state, &q, &memo);
        let b0 = keys("B0", &merged, &cands, &state, &q, &memo);
        let h10 = keys("H10", &merged, &cands, &state, &q, &memo);
        let hpt = keys("H10[PT*]", &merged, &cands, &state, &q, &memo);
        let hb0 = keys("H10[B0]", &merged, &cands, &state, &q, &memo);
        assert_eq!(memo.keys(), ["b0", "c0", "p1", "pt"]);

        // H10 = [c0, p1, 0], and so on for the other two co-change forms.
        for i in 0..cands.len() {
            let c0 = if i as u32 == d { 0.5 } else { 0.0 };
            assert_eq!(h10[i], [c0, p1[i][0], 0.0]);
            assert_eq!(hpt[i], [c0, pt[i][0], 0.0]);
            assert_eq!(hb0[i], [c0, b0[i][0], b0[i][1]]);
        }

        // Equal, bit for bit, to the answers' rank_files_with_c0 (one file
        // space: candidate position == FileIndex index here).
        let dense: Vec<f64> = (0..cands.len()).map(|i| if i as u32 == d { 0.5 } else { 0.0 }).collect();
        for (name, ours) in [("P1", &p1), ("PT*", &pt), ("B0", &b0), ("H10", &h10), ("H10[PT*]", &hpt), ("H10[B0]", &hb0)] {
            let r = Ranking::from_harness_name(name).expect(name);
            let fr = rank_files_with_c0(&merged, &cache, &[a], &[], r, Some(&dense));
            assert_eq!(&fr.keys, ours, "{name}");
        }

        // B0: f is unreached; c sits two hops from a's nodes or fewer.
        assert_eq!(b0[f as usize][0], -1e9);
        assert!(b0[c as usize][0] < 0.0 && b0[c as usize][0] > -1e9);
        assert_eq!(b0[f as usize][1], cands.deg[f as usize]);
        assert_eq!(p1[f as usize][0], 0.0);

        // Ties go to the smaller path: d and e are structurally symmetric.
        let e = pos("e.py") as usize;
        assert_eq!(p1[d as usize], p1[e]);
        assert!(better(&p1[d as usize], d as usize, &p1[e], e));
        assert!(!better(&p1[e], e, &p1[d as usize], d as usize));
        // ... and co-change lifts d above every non-co-changed file in H10.
        for i in 0..cands.len() {
            if i as u32 != d {
                assert!(better(&h10[d as usize], d as usize, &h10[i], i), "{i}");
            }
        }
    }

    #[test]
    fn pt_sums_where_p1_takes_the_max() {
        let (_tmp, merged, cache, cands) = toy();
        let idx = StructIndex::build(&merged, &cands, Arc::clone(&cache));
        let a = cands.position("a.py").expect("a") as u32;
        let pers = uniform_seed_nodes(&idx, &merged, &[a]);
        assert!(!pers.is_empty());
        let w = 1.0 / pers.len() as f64;
        assert!(pers.iter().all(|&(_, x)| x == w));
        let files = cache.files(&merged);
        for (wide, base) in [(false, RankBase::Ppr), (true, RankBase::PprWide)] {
            let got = ppr_files(&idx, &merged, &pers, wide);
            let ix = cache.ppr_index(&merged, SourceKey::Merged, &ppr_config(base).expect("ppr"));
            let (v, _) = ix.vector_weighted(&pers);
            for (p, &f) in cands.files.iter().enumerate() {
                let mut acc = 0.0f64;
                for &u in files.nodes(f) {
                    let s = ix.index_of(u).map_or(0.0, |i| v[i]);
                    acc = if wide { acc + s } else { acc.max(s) };
                }
                assert_eq!(got[p], acc, "wide={wide} file={}", cands.paths[p]);
            }
        }
        // b.py holds three nodes (MODULE, bf, bg): its sum exceeds its max.
        let b = cands.position("b.py").expect("b");
        let p1 = ppr_files(&idx, &merged, &pers, false);
        let sum05: f64 = {
            let ix = cache.ppr_index(&merged, SourceKey::Merged, &ppr_config(RankBase::Ppr).expect("ppr"));
            let (v, _) = ix.vector_weighted(&pers);
            files.nodes(cands.files[b]).iter().map(|&u| ix.index_of(u).map_or(0.0, |i| v[i])).sum()
        };
        assert!(sum05 > p1[b], "sum {sum05} max {}", p1[b]);
        // Empty personalisation: zeros, and BFS reaches nothing.
        assert!(ppr_files(&idx, &merged, &[], true).iter().all(|&s| s == 0.0));
        assert!(bfs_files(&idx, &merged, &[]).iter().all(|k| k[0] == -1e9));
    }

    #[test]
    fn find_seeds_weigh_by_file() {
        let (_tmp, merged, cache, cands) = toy();
        let idx = StructIndex::build(&merged, &cands, cache);
        let (b, c) = (cands.position("b.py").expect("b") as u32, cands.position("c.py").expect("c") as u32);
        let pers = weighted_seed_nodes(&idx, &[(b, 0.6), (c, 0.4)]);
        let nb = idx.nodes_at(b).len();
        let nc = idx.nodes_at(c).len();
        assert_eq!(pers.len(), nb + nc);
        assert!(pers[..nb].iter().all(|&(_, w)| w == 0.6 / nb as f64));
        assert!(pers[nb..].iter().all(|&(_, w)| w == 0.4 / nc as f64));
        assert!(weighted_seed_nodes(&idx, &[(99, 1.0)]).is_empty());
    }

    #[test]
    fn unknown_name_or_no_bank_is_none() {
        let (_tmp, merged, _cache, cands) = toy();
        let state = ReplayState::new(cands.len(), 30);
        let q = query(vec![0], vec![]);
        let memo = Memo::new();
        let excluded = vec![false; cands.len()];
        let commit = ReplayCommit::default();
        let ctx = QueryCtx {
            merged: &merged,
            cands: &cands,
            commit: &commit,
            query: &q,
            excluded: &excluded,
            state: &state,
            memo: &memo,
            repo_root: Path::new("."),
        };
        assert!(structure_keys("P1", &ctx).is_none(), "no bank registered");
        assert!(structure_keys("H9s50", &ctx).is_none());
    }
}
