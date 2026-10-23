//! The measured file ranking and the row order the answers take from it
//! (EA.4, items 1 and 2).
//!
//! [`rank_files`] computes, over every candidate file of the
//! [`FileIndex`], the key rank_eval.rs scored for a [`Ranking`]:
//! - `P1` / `PT*`: undirected PPR seeded uniformly over every node of the seed
//!   files (`Ppr::uniform`, `base_cfg` / the grid's `base,d85`), node scores
//!   max- / sum-pooled per file (`file_pool`), key `[score, 0, 0]` (`scalar`);
//! - `B0`: carry-edge BFS both ways from the same nodes, key `[-depth (or
//!   -1e9 unreached), file degree, 0]` (`bfs_key`);
//! - co-change first (`H10`, `H10[PT*]`, `H10[B0]`): `[C0, base..]` (`lex2`,
//!   r3's `H10[B0]`).
//!
//! `order` is every file but the seed files, best first by rank_eval's
//! `better` (copied verbatim: `>` / `<` per component, then the smaller file
//! index), so a tie goes to the smaller (repo label, path), never a NodeId.
//! [`order_rows`] is the one rule that turns that ranking into an answer's
//! row order.
//!
//! Fired_on, stderr: `[rank-files] rank ranking=<harness name>
//! seed_files=<s> extra_seeds=<e> candidates=<n> cochange_files=<c>
//! index=<hit|built|bfs> ms=<m>` once per [`rank_files`] call.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use glia_activation::algo::{Walk, reach};
use glia_activation::{Direction, PprIndex};
use glia_code_domain::profile::CODE_TABLES;
use glia_core::NodeId;
use glia_graph::MergedGraph;
use serde::Serialize;

use super::cache::{RankCache, SourceKey};
use super::cochange::c0;
use super::files::FileIndex;
use super::ranking::{RankBase, Ranking};

/// B0's key for a file no walk reached (rank_eval `bfs_key`).
const UNREACHED: f64 = -1e9;

/// One ranking of the whole candidate file space.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileRanking {
    pub ranking: Ranking,
    /// The seed files, in the order given (C0 sums in this order).
    pub seed_files: Vec<u32>,
    /// Every candidate file but the seed files, best first.
    pub order: Vec<u32>,
    /// Per candidate file, the key `order` compares.
    pub keys: Vec<[f64; 3]>,
    /// Per candidate file, C0 (zeros unless co-change ranks first).
    pub c0: Vec<f64>,
    /// Repo id -> label, from the RankCache, for [`Self::top`] (FileIndex
    /// holds repo ids only).
    labels: BTreeMap<u64, String>,
}

/// One file of [`FileRanking::top`].
#[derive(Serialize, Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RankedFile {
    /// 1-based.
    pub rank: usize,
    pub file: String,
    /// The repo label.
    pub repo: String,
    pub cochange: f64,
    /// The pooled PPR score (`P1` / `PT*` bases).
    pub ppr: Option<f64>,
    /// The BFS depth (`B0` base), when reached.
    pub depth: Option<usize>,
    /// The answer's rows located in this file.
    pub rows: usize,
}

impl FileRanking {
    /// File `f`'s 0-based place in `order`; None for a seed file or no
    /// candidate.
    pub fn position(&self, f: u32) -> Option<usize> {
        self.order.iter().position(|&x| x == f)
    }

    /// The structural part of `f`'s key: where it sits after C0.
    fn base_part(&self, f: u32) -> Option<&[f64]> {
        let k = self.keys.get(f as usize)?;
        Some(if self.ranking.cochange_first {
            &k[1..]
        } else {
            &k[..]
        })
    }

    /// A repo's label: the cache's, else `repo#<id % 1000>` (FileIndex's
    /// fallback).
    fn repo_label(&self, repo: u64) -> String {
        self.labels
            .get(&repo)
            .cloned()
            .unwrap_or_else(|| format!("repo#{}", repo % 1000))
    }

    /// The first `k` files of `order`, located and labelled.
    pub fn top(
        &self,
        files: &FileIndex,
        k: usize,
        rows_per_file: &BTreeMap<u32, usize>,
    ) -> Vec<RankedFile> {
        self.order
            .iter()
            .take(k)
            .enumerate()
            .filter(|&(_, &f)| (f as usize) < files.len())
            .map(|(i, &f)| {
                let base = self.base_part(f);
                let (ppr, depth) = match self.ranking.base {
                    RankBase::Ppr | RankBase::PprWide => (base.map(|b| b[0]), None),
                    RankBase::Bfs => {
                        let d = base
                            .map(|b| b[0])
                            .filter(|&v| v != UNREACHED)
                            .map(|v| (-v) as usize);
                        (None, d)
                    }
                };
                RankedFile {
                    rank: i + 1,
                    file: files.path(f).to_string(),
                    repo: self.repo_label(files.repo(f)),
                    cochange: self.c0.get(f as usize).copied().unwrap_or(0.0),
                    ppr,
                    depth,
                    rows: rows_per_file.get(&f).copied().unwrap_or(0),
                }
            })
            .collect()
    }
}

/// rank_eval's `better`, verbatim: `a` (file `ia`) ranks above `b` (file
/// `ib`).
fn better(a: &[f64; 3], ia: usize, b: &[f64; 3], ib: usize) -> bool {
    for i in 0..3 {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    ia < ib
}

/// The PPR config of a PPR base: rank_eval's `base_cfg` (and its `base,d85`
/// grid entry for PT*).
fn ppr_config(base: RankBase) -> Option<glia_activation::ActivationConfig> {
    let damping = base.damping()?;
    let mut cfg = CODE_TABLES.activation_config(None);
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    cfg.damping = damping;
    Some(cfg)
}

/// The cached index of a PPR base and whether the lookup built it.
fn ppr_index(
    merged: &MergedGraph,
    cache: &RankCache,
    base: RankBase,
) -> Option<(Arc<PprIndex>, &'static str)> {
    let cfg = ppr_config(base)?;
    let before = cache.stats().built;
    let ix = cache.ppr_index(merged, SourceKey::Merged, &cfg);
    let how = if cache.stats().built > before {
        "built"
    } else {
        "hit"
    };
    Some((ix, how))
}

/// rank_eval `file_pool` over an index's vector: max (or sum) over each
/// file's nodes in order.
fn pool(files: &FileIndex, ix: &PprIndex, v: &[f64], sum: bool) -> Vec<f64> {
    (0..files.len() as u32)
        .map(|f| {
            let mut acc = 0.0f64;
            for &u in files.nodes(f) {
                let s = ix
                    .index_of(u)
                    .map_or(0.0, |i| v.get(i).copied().unwrap_or(0.0));
                if sum {
                    acc += s;
                } else {
                    acc = acc.max(s);
                }
            }
            acc
        })
        .collect()
}

/// rank_eval `bfs_key`: `[-depth or -1e9, degree, 0]` per file, depth the
/// least over the file's reached nodes (a seed node is not reached).
fn bfs_keys(
    merged: &MergedGraph,
    cache: &RankCache,
    files: &FileIndex,
    seeds: &[NodeId],
) -> Vec<[f64; 3]> {
    let adj = cache.carry(merged);
    let b = reach::bfs(&adj, seeds, Walk::Both, usize::MAX);
    let mut depth = vec![f64::INFINITY; files.len()];
    for r in &b.reached {
        if let Some(f) = files.file_of_node(r.id) {
            let d = &mut depth[f as usize];
            *d = d.min(r.depth as f64);
        }
    }
    (0..files.len() as u32)
        .map(|f| {
            let d = depth[f as usize];
            [
                if d.is_finite() { -d } else { UNREACHED },
                files.degree(f),
                0.0,
            ]
        })
        .collect()
}

/// The structural key per candidate file under `base` from a weighted
/// personalisation (no co-change): `P1` / `PT*` through
/// [`PprIndex::vector_weighted`] and `file_pool`, `[score, 0, 0]`; `B0`
/// walks from the nodes of `pers` in order, ignoring the weights. The
/// calibration replay (EC.4, EC.13) runs the answers' arithmetic through this.
pub fn base_keys(
    merged: &MergedGraph,
    cache: &RankCache,
    pers: &[(NodeId, f64)],
    base: RankBase,
) -> Vec<[f64; 3]> {
    let files = cache.files(merged);
    match ppr_index(merged, cache, base) {
        Some((ix, _)) => ppr_keys(&files, &ix, pers, base),
        None => {
            let nodes: Vec<NodeId> = pers.iter().map(|&(id, _)| id).collect();
            bfs_keys(merged, cache, &files, &nodes)
        }
    }
}

fn ppr_keys(
    files: &FileIndex,
    ix: &PprIndex,
    pers: &[(NodeId, f64)],
    base: RankBase,
) -> Vec<[f64; 3]> {
    let (v, _) = ix.vector_weighted(pers);
    pool(files, ix, &v, base.sum_pooled())
        .into_iter()
        .map(|s| [s, 0.0, 0.0])
        .collect()
}

/// The measured ranking of every candidate file for `seed_files` (and
/// `extra_seed_nodes`, seeds whose own file is no candidate), C0 read from
/// `cache`'s co-change index.
pub fn rank_files(
    merged: &MergedGraph,
    cache: &RankCache,
    seed_files: &[u32],
    extra_seed_nodes: &[NodeId],
    ranking: Ranking,
) -> FileRanking {
    rank_files_with_c0(merged, cache, seed_files, extra_seed_nodes, ranking, None)
}

/// [`rank_files`] with a caller's dense C0 (one slot per candidate file;
/// a shorter vector reads 0 past its end) in place of the co-change index's.
/// It is read only when co-change ranks first.
pub fn rank_files_with_c0(
    merged: &MergedGraph,
    cache: &RankCache,
    seed_files: &[u32],
    extra_seed_nodes: &[NodeId],
    ranking: Ranking,
    c0_given: Option<&[f64]>,
) -> FileRanking {
    let t0 = Instant::now();
    let files = cache.files(merged);
    let n = files.len();

    // a. seed nodes: each seed file's nodes in order, then the extras, first
    // occurrence kept.
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut seeds: Vec<NodeId> = Vec::new();
    for &f in seed_files {
        if (f as usize) < n {
            for &u in files.nodes(f) {
                if seen.insert(u) {
                    seeds.push(u);
                }
            }
        }
    }
    for &u in extra_seed_nodes {
        if seen.insert(u) {
            seeds.push(u);
        }
    }

    // b. the structural key.
    let (base, how) = match ppr_index(merged, cache, ranking.base) {
        Some((ix, how)) => {
            // Ppr::uniform: 1 / count over the seeds the index holds, as
            // PprIndex::vector weighs them.
            let held = seeds.iter().filter(|&&u| ix.index_of(u).is_some()).count();
            let w = if held == 0 { 0.0 } else { 1.0 / held as f64 };
            let pers: Vec<(NodeId, f64)> = seeds.iter().map(|&u| (u, w)).collect();
            (ppr_keys(&files, &ix, &pers, ranking.base), how)
        }
        None => (bfs_keys(merged, cache, &files, &seeds), "bfs"),
    };

    // c. C0.
    let c0v: Vec<f64> = if ranking.cochange_first {
        match c0_given {
            Some(g) => (0..n).map(|f| g.get(f).copied().unwrap_or(0.0)).collect(),
            None => c0(&*cache.cochange(merged), seed_files, n),
        }
    } else {
        vec![0.0; n]
    };

    // d. the key.
    let keys: Vec<[f64; 3]> = if ranking.cochange_first {
        base.iter()
            .zip(&c0v)
            .map(|(k, &c)| match ranking.base {
                RankBase::Bfs => [c, k[0], k[1]],
                RankBase::Ppr | RankBase::PprWide => [c, k[0], 0.0],
            })
            .collect()
    } else {
        base
    };

    // e. the order: every file but the seeds, by `better`.
    let is_seed = |f: u32| seed_files.contains(&f);
    let mut order: Vec<u32> = (0..n as u32).filter(|&f| !is_seed(f)).collect();
    order.sort_by(|&a, &b| {
        let (ia, ib) = (a as usize, b as usize);
        if better(&keys[ia], ia, &keys[ib], ib) {
            Ordering::Less
        } else if a == b {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    });

    let cochange_files = (0..n)
        .filter(|&f| c0v[f] > 0.0 && !is_seed(f as u32))
        .count();
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "[rank-files] rank ranking={} seed_files={} extra_seeds={} candidates={n} cochange_files={cochange_files} index={how} ms={:.1}",
        ranking.harness_name(),
        seed_files.len(),
        extra_seed_nodes.len(),
        (ms * 10.0).trunc() / 10.0,
    );
    FileRanking {
        ranking,
        seed_files: seed_files.to_vec(),
        order,
        keys,
        c0: c0v,
        labels: cache.repo_labels().clone(),
    }
}

/// The permutation an answer applies to its rows (`row_files[i]`: the
/// candidate file of row i, or None), or None when it is the identity.
///
/// - Tier 0 (lifted), only when co-change ranks first: a row whose file has
///   C0 > 0 and is no seed file, by the file's place in `ranking.order`.
/// - Tier 1, every other row. Base `P1`: input order (the answer's own order
///   stands, so `Ranking::head()` and a history-free repo are unchanged).
///   Base `PT*` / `B0`: a seed file's rows first, then candidate files' rows
///   by place, then rows with no candidate file.
///
/// Ties keep the input order. Positions are integers: no float is compared.
pub fn order_rows(
    row_files: &[Option<u32>],
    seed_files: &[u32],
    ranking: &FileRanking,
) -> Option<Vec<usize>> {
    let mut pos: Vec<usize> = vec![usize::MAX; ranking.keys.len().max(ranking.c0.len())];
    for (p, &f) in ranking.order.iter().enumerate() {
        if let Some(slot) = pos.get_mut(f as usize) {
            *slot = p;
        }
    }
    let place = |f: u32| pos.get(f as usize).copied().filter(|&p| p != usize::MAX);
    let grouped = !matches!(ranking.ranking.base, RankBase::Ppr);
    let key = |f: Option<u32>| -> (u8, u8, usize) {
        let seed = f.is_some_and(|f| seed_files.contains(&f));
        if ranking.ranking.cochange_first
            && let Some(f) = f
            && !seed
            && ranking.c0.get(f as usize).is_some_and(|&c| c > 0.0)
            && let Some(p) = place(f)
        {
            return (0, 0, p);
        }
        if !grouped {
            return (1, 0, 0);
        }
        match f {
            Some(_) if seed => (1, 0, 0),
            Some(f) => match place(f) {
                Some(p) => (1, 1, p),
                None => (1, 2, 0),
            },
            None => (1, 2, 0),
        }
    };
    let mut perm: Vec<(u8, u8, usize, usize)> = row_files
        .iter()
        .enumerate()
        .map(|(i, &f)| {
            let (t, g, p) = key(f);
            (t, g, p, i)
        })
        .collect();
    perm.sort_unstable();
    let out: Vec<usize> = perm.into_iter().map(|(_, _, _, i)| i).collect();
    if out.iter().enumerate().all(|(i, &j)| i == j) {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built ranking over 6 files: order [3, 1, 4, 2, 5], seed 0.
    fn fr(ranking: Ranking, c0: Vec<f64>) -> FileRanking {
        FileRanking {
            ranking,
            seed_files: vec![0],
            order: vec![3, 1, 4, 2, 5],
            keys: vec![[0.0; 3]; 6],
            c0,
            labels: BTreeMap::new(),
        }
    }

    #[test]
    fn better_is_the_harness_rule() {
        assert!(better(&[1.0, 0.0, 0.0], 5, &[0.5, 9.0, 9.0], 0));
        assert!(better(&[1.0, 2.0, 0.0], 5, &[1.0, 1.0, 9.0], 0));
        assert!(
            better(&[1.0, 1.0, 1.0], 2, &[1.0, 1.0, 1.0], 3),
            "a tie goes to the smaller index"
        );
        assert!(!better(&[1.0, 1.0, 1.0], 3, &[1.0, 1.0, 1.0], 2));
        assert!(!better(&[-1e9, 0.0, 0.0], 0, &[-1.0, 0.0, 0.0], 1));
    }

    #[test]
    fn head_is_the_identity() {
        let r = fr(Ranking::head(), vec![0.0; 6]);
        assert_eq!(
            order_rows(&[Some(5), None, Some(3), Some(0), Some(1)], &[0], &r),
            None
        );
        assert_eq!(order_rows(&[], &[0], &r), None);
    }

    #[test]
    fn no_cochange_under_h10_is_the_identity() {
        let r = fr(Ranking::default(), vec![0.0; 6]);
        assert_eq!(
            order_rows(&[Some(5), None, Some(3), Some(1)], &[0], &r),
            None
        );
    }

    #[test]
    fn h10_lifts_cochanged_rows_by_place() {
        // files 2 and 5 co-changed (place 3 and 4), 1 not; seed 0 co-changed
        // with another seed but is never lifted.
        let r = fr(Ranking::default(), vec![0.7, 0.0, 0.1, 0.0, 0.0, 0.2]);
        let rows = [Some(1), Some(5), None, Some(0), Some(2), Some(5), Some(3)];
        assert_eq!(order_rows(&rows, &[0], &r), Some(vec![4, 1, 5, 0, 2, 3, 6]));
    }

    #[test]
    fn wide_groups_seed_rows_first_and_fileless_rows_last() {
        let r = fr(Ranking::new(RankBase::PprWide, false), vec![0.0; 6]);
        let rows = [None, Some(2), Some(0), Some(3), Some(9), Some(1), Some(0)];
        // seed rows (2, 6), then 3 (place 0), 1 (place 1), 2 (place 3), then
        // no-file rows (0, and 4: file 9 is no candidate).
        assert_eq!(order_rows(&rows, &[0], &r), Some(vec![2, 6, 3, 5, 1, 0, 4]));
        let bfs = fr(
            Ranking::new(RankBase::Bfs, true),
            vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
        );
        let rows = [Some(3), Some(5), Some(0)];
        assert_eq!(order_rows(&rows, &[0], &bfs), Some(vec![1, 2, 0]));
    }

    #[test]
    fn ties_keep_input_order() {
        let r = fr(Ranking::new(RankBase::Bfs, false), vec![0.0; 6]);
        let rows = [Some(1), Some(3), Some(1), Some(3), None, None];
        assert_eq!(order_rows(&rows, &[0], &r), Some(vec![1, 3, 0, 2, 4, 5]));
    }

    #[test]
    fn position_skips_seeds() {
        let r = fr(Ranking::head(), vec![0.0; 6]);
        assert_eq!(r.position(3), Some(0));
        assert_eq!(r.position(5), Some(4));
        assert_eq!(r.position(0), None);
    }
}
