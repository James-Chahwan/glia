//! EA.6 / EA.6b: pack / pack_ids ranked by the selected `Ranking`.
//!
//! The tree: `svc/core.py` defines `core(x)`; `svc/a.py`, `b.py`, `c.py`
//! each define one function calling it; `svc/d.py` defines `d1` and `d2`,
//! both calling it; `web/page.ts` links to nothing. The history snapshot
//! (EA.3's shape) co-changes core with c.py 5 times, with page.ts 4 times,
//! with b.py twice, and alone 3 times. Seed: `core`'s FUNCTION.
//!
//! Without history, or under `Ranking::head()`, the pack is the 0.5.2 pack
//! byte for byte (the PPR scores come from a cached `PprIndex`, bit-identical,
//! and every neighbour keeps its 0.5.2 weight). Under the default `H10` with a
//! cache that knows the repo root, each co-changed file's best row comes
//! first, in the file ranking's order (c.py, then b.py; page.ts is never
//! reached by the PPR so adds no node), then every other row in the 0.5.2
//! order (EA.6b, ruling EA-2b: pack lifts FILES, not all their rows).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use glia_activation::Direction;
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_core::NodeId;
use glia_engine::generate_one;
use glia_engine::pack::{Pack, PackArgs, pack_ids};
use glia_engine::rank::{RankBase, RankCache, Ranking, order_rows, rank_files};
use glia_graph::MergedGraph;

const T0: i64 = 1_767_225_600;
const CORE: &str = "svc/core.py";
const A: &str = "svc/a.py";
const B: &str = "svc/b.py";
const C: &str = "svc/c.py";
const D: &str = "svc/d.py";
const PAGE: &str = "web/page.ts";

fn tree() -> Vec<(&'static str, String)> {
    let caller =
        |f: &str| format!("from svc.core import core\n\n\ndef {f}():\n    return core(1)\n");
    vec![
        (CORE, "def core(x):\n    return x\n".to_string()),
        (A, caller("a")),
        (B, caller("b")),
        (C, caller("c")),
        (
            D,
            "from svc.core import core\n\n\ndef d1():\n    return core(1)\n\n\ndef d2():\n    return core(2) + 1\n"
                .to_string(),
        ),
        (
            PAGE,
            "export function renderPage(t: string): string {\n  return t;\n}\n".to_string(),
        ),
    ]
}

fn file(p: &str) -> HistoryFile {
    HistoryFile {
        p: p.to_string(),
        a: Some(1),
        d: Some(0),
        from: None,
    }
}

/// Commits from `groups` given oldest first, returned newest first.
fn history() -> Vec<HistoryCommit> {
    commits(vec![
        (vec![CORE, C], 5),
        (vec![CORE, PAGE], 4),
        (vec![CORE, B], 2),
        (vec![CORE], 3),
    ])
}

/// Commits from `groups` given oldest first, returned newest first.
fn commits(groups: Vec<(Vec<&str>, usize)>) -> Vec<HistoryCommit> {
    let mut out = Vec::new();
    let mut i = 0usize;
    for (paths, n) in groups {
        for _ in 0..n {
            i += 1;
            out.push(HistoryCommit {
                c: format!("{i:04}{}", "0".repeat(36)),
                t: T0 + i64::try_from(i).expect("small") * 3_600,
                files: paths.iter().map(|p| file(p)).collect(),
            });
        }
    }
    out.reverse();
    out
}

struct Built {
    _dir: tempfile::TempDir,
    merged: MergedGraph,
    roots: BTreeMap<u64, String>,
    labels: BTreeMap<u64, String>,
}

fn build(with_history: bool) -> Built {
    build_with(tree(), if with_history { Some(history()) } else { None })
}

fn build_with(files: Vec<(&'static str, String)>, hist: Option<Vec<HistoryCommit>>) -> Built {
    let d = tempfile::tempdir().expect("tempdir");
    let root: &Path = d.path();
    for (p, text) in files {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    if let Some(h) = hist {
        let head = h.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
        write_history(root, HistoryMeta::new(head, 2000, None, String::new()), &h, &[])
            .expect("write snapshot");
    }
    let r = generate_one(&root.to_string_lossy()).expect("build");
    Built {
        merged: r.merged,
        roots: r.repo_roots,
        labels: r.repo_labels,
        _dir: d,
    }
}

impl Built {
    fn cache(&self) -> Arc<RankCache> {
        Arc::new(RankCache::new("answer", &self.roots, &self.labels))
    }

    fn core(&self) -> NodeId {
        self.merged
            .node_id_by_qname("svc::core::core")
            .expect("core's FUNCTION")
    }
}

/// candidates 6 and a budget every candidate fits in at its top rung.
fn args(ranking: Ranking, cache: Option<Arc<RankCache>>, candidates: usize) -> PackArgs {
    let mut a = PackArgs::default();
    a.candidates = candidates;
    a.budget_tokens = 20_000;
    a.ranking = ranking;
    a.rank_cache = cache;
    a
}

fn json(p: &Pack) -> String {
    serde_json::to_string(p).expect("pack json")
}

/// (rank, qname, fidelity) per packed node.
fn manifest(p: &Pack) -> Vec<(usize, String, &'static str)> {
    p.nodes
        .iter()
        .map(|n| (n.rank, n.qname.clone(), n.fidelity))
        .collect()
}

fn files_of(p: &Pack) -> Vec<String> {
    p.nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| n.file.clone().unwrap_or_default())
        .collect()
}

/// The 0.5.2 pack of this tree from `core` at candidates 6, captured at
/// HEAD 7a37a8d5 before EA.6, as score tiers: the seed, then d.py's two
/// functions, then a / b / c's (tied PPR scores order by NodeId, which hashes
/// the tempdir's RepoId, so a tier is a set here and its order is checked
/// against the 0.5.2 rule, score desc then id asc, below).
const HEAD_TIERS: &[&[&str]] = &[
    &["svc::core::core"],
    &["svc::d::d1", "svc::d::d2"],
    &["svc::a::a", "svc::b::b", "svc::c::c"],
];

/// The 0.5.2 step 2 verbatim: `MergedGraph::activate` (no index), sorted
/// (score desc, id asc), seeds and zero scores dropped.
fn head_neighbours(m: &MergedGraph, seeds: &[NodeId]) -> Vec<u64> {
    let mut cfg = CODE_TABLES.activation_config(None);
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    let mut scored = m.activate(seeds, &cfg).scores;
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.0.cmp(&b.0.0)));
    scored
        .into_iter()
        .filter(|(id, s)| *s > 0.0 && !seeds.contains(id))
        .map(|(id, _)| id.0)
        .collect()
}

#[test]
fn head_ranking_without_a_cache_is_the_0_5_2_pack() {
    let b = build(true);
    let p = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, 6));
    let got: Vec<(usize, String, &str)> = manifest(&p);
    let mut at = 0usize;
    for tier in HEAD_TIERS {
        let mut names: Vec<&str> = got[at..at + tier.len()].iter().map(|(_, q, _)| q.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, tier.to_vec(), "{got:?}");
        at += tier.len();
    }
    assert_eq!(at, got.len());
    assert!(got.iter().enumerate().all(|(i, (r, _, f))| *r == i + 1 && *f == "full"), "{got:?}");
    let ids: Vec<u64> = p.nodes.iter().skip(1).map(|n| n.id).collect();
    let mut want = head_neighbours(&b.merged, &[b.core()]);
    want.truncate(5);
    assert_eq!(ids, want, "the 0.5.2 order");
    // With the snapshot cache too: head is co-change-blind and P1 keeps the
    // input order, so the JSON is the same.
    let with_cache = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), Some(b.cache()), 6));
    assert_eq!(json(&with_cache), json(&p));
}

/// (file, qname) per neighbour row.
fn neighbours(p: &Pack) -> Vec<(String, String)> {
    p.nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| (n.file.clone().unwrap_or_default(), n.qname.clone()))
        .collect()
}

/// The 0.5.2 order with each lifted file's first row moved to the front, in
/// `lift` order (EA.6b, ruling EA-2b: pack lifts FILES, one best row each).
fn file_lift(head: &[(String, String)], lift: &[&str]) -> Vec<String> {
    let mut front: Vec<usize> = Vec::new();
    for f in lift {
        if let Some(i) = head.iter().position(|(hf, _)| hf == f) {
            front.push(i);
        }
    }
    let mut want: Vec<String> = front.iter().map(|&i| head[i].1.clone()).collect();
    want.extend(
        head.iter()
            .enumerate()
            .filter(|(i, _)| !front.contains(i))
            .map(|(_, (_, q))| q.clone()),
    );
    want
}

/// The most neighbour rows one file holds.
fn max_share(p: &Pack) -> usize {
    let mut per: BTreeMap<String, usize> = BTreeMap::new();
    for f in files_of(p) {
        *per.entry(f).or_insert(0) += 1;
    }
    per.values().copied().max().unwrap_or(0)
}

#[test]
fn cochange_first_lifts_the_cochanged_files_best_rows() {
    let b = build(true);
    let head_all = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, usize::MAX));
    let ranked = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), Some(b.cache()), 6));

    // The head order of every neighbour, by file.
    let all = files_of(&head_all);
    assert!(all.iter().any(|f| f == C) && all.iter().any(|f| f == B), "{all:?}");
    assert!(!all.iter().any(|f| f == PAGE), "page.ts is not PPR-reached: {all:?}");
    // c.py's best row, then b.py's best row, then every other row (c.py's and
    // b.py's later rows included) in head order.
    let mut want = file_lift(&neighbours(&head_all), &[C, B]);
    want.truncate(5);

    let got: Vec<String> = ranked
        .nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| n.qname.clone())
        .collect();
    assert_eq!(got, want, "{}", json(&ranked));
    assert_eq!(ranked.nodes[0].reason, "seed");
    let ranks: Vec<usize> = ranked.nodes.iter().map(|n| n.rank).collect();
    assert_eq!(ranks, (1..=ranks.len()).collect::<Vec<_>>(), "every candidate packs");
    let f = files_of(&ranked);
    assert_eq!(&f[..2], [C, B], "{f:?}");
    assert_eq!(got[..2], ["svc::c::c", "svc::b::b"], "measured at landing");
    assert_ne!(json(&ranked), json(&pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, 6))));
}

#[test]
fn without_a_snapshot_the_default_is_the_head_pack() {
    let b = build(false);
    let head = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, 6));
    let def = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), Some(b.cache()), 6));
    assert_eq!(json(&def), json(&head));
    let def_nocache = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), None, 6));
    assert_eq!(json(&def_nocache), json(&head));
}

#[test]
fn a_second_call_on_one_cache_hits_the_index() {
    let b = build(true);
    let cache = b.cache();
    let first = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), Some(cache.clone()), 6));
    let built = cache.stats().built;
    let hits = cache.stats().hits;
    let second = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), Some(cache.clone()), 6));
    assert_eq!(cache.stats().built, built, "no index rebuilt");
    assert!(cache.stats().hits > hits, "the second call hits");
    assert_eq!(json(&first), json(&second));
}

#[test]
fn ppr_wide_groups_neighbours_by_the_file_order() {
    let b = build(false);
    let cache = b.cache();
    let wide = Ranking::new(RankBase::PprWide, false);
    let head_all = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, usize::MAX));
    let got = pack_ids(&b.merged, &[b.core()], &args(wide, Some(cache.clone()), usize::MAX));

    let files = cache.files(&b.merged);
    let core_file = files.file_of_node(b.core()).expect("core's file");
    let fr = rank_files(&b.merged, &cache, &[core_file], &[], wide);
    let neigh: Vec<NodeId> = head_all
        .nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| NodeId(n.id))
        .collect();
    let row_files: Vec<Option<u32>> = neigh.iter().map(|&id| files.file_of_node(id)).collect();
    let perm = order_rows(&row_files, &[core_file], &fr).unwrap_or_else(|| (0..neigh.len()).collect());
    let want: Vec<u64> = perm.iter().map(|&i| neigh[i].0).collect();
    let got_ids: Vec<u64> = got
        .nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| n.id)
        .collect();
    assert_eq!(got_ids, want);
    // Each file's rows are contiguous.
    let fs: Vec<Option<u32>> = got_ids.iter().map(|&id| files.file_of_node(NodeId(id))).collect();
    let mut seen: Vec<Option<u32>> = Vec::new();
    for w in fs.iter() {
        if seen.last() != Some(w) {
            assert!(!seen.contains(w), "file rows split: {fs:?}");
            seen.push(*w);
        }
    }
}

const BIG: &str = "svc/big.py";

/// (6): one co-changed file holds more rows than the candidate cut (its
/// rows past big0 sit two hops from the seed). EA.6 put
/// every one of its rows first and crowded the other files out; the file lift
/// moves only its best row, so every file the 0.5.2 pack reaches stays, and no
/// file takes more of the cut than under PPR alone.
#[test]
fn a_big_cochanged_file_does_not_crowd_out_the_others() {
    let mut files = tree();
    // big0 calls core; big1..big8 call big0, two hops out, so PPR ranks them
    // below a / b / c / d.
    let mut big = String::from("from svc.core import core\n\n\ndef big0():\n    return core(0)\n");
    for i in 1..9 {
        big.push_str(&format!("\n\ndef big{i}():\n    return big0() + {i}\n"));
    }
    files.push((BIG, big));
    let hist = commits(vec![(vec![CORE, BIG], 5), (vec![CORE], 2)]);
    let b = build_with(files, Some(hist));
    let cut = 6;
    let head = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, cut));
    let head_all = pack_ids(&b.merged, &[b.core()], &args(Ranking::head(), None, usize::MAX));
    let ranked = pack_ids(&b.merged, &[b.core()], &args(Ranking::default(), Some(b.cache()), cut));

    let big_rows = files_of(&head_all).iter().filter(|f| *f == BIG).count();
    assert!(big_rows > cut, "big.py holds more rows than the cut: {big_rows}");

    let mut want = file_lift(&neighbours(&head_all), &[BIG]);
    want.truncate(cut - 1);
    let got: Vec<String> = neighbours(&ranked).into_iter().map(|(_, q)| q).collect();
    assert_eq!(got, want, "{}", json(&ranked));
    assert_eq!(files_of(&ranked)[0], BIG);

    let kept = files_of(&ranked);
    for f in files_of(&head) {
        assert!(kept.contains(&f), "{f} dropped: head {:?} ranked {kept:?}", files_of(&head));
    }
    assert!(
        max_share(&ranked) <= max_share(&head),
        "max_file_share {} > PPR's {}",
        max_share(&ranked),
        max_share(&head)
    );

    // EA.6's row lift (order_rows' tier 0) on the same rows would have given
    // big.py the whole cut: the fixture tells the two rules apart.
    let cache = b.cache();
    let fi = cache.files(&b.merged);
    let core_file = fi.file_of_node(b.core()).expect("core's file");
    let fr = rank_files(&b.merged, &cache, &[core_file], &[], Ranking::default());
    let neigh: Vec<NodeId> = head_all
        .nodes
        .iter()
        .filter(|n| n.reason == "neighbour")
        .map(|n| NodeId(n.id))
        .collect();
    let row_files: Vec<Option<u32>> = neigh.iter().map(|&id| fi.file_of_node(id)).collect();
    let perm = order_rows(&row_files, &[core_file], &fr).expect("EA.6 lifts");
    let big_file = fi.file_of_node(neigh[perm[0]]);
    assert!(perm.iter().take(cut - 1).all(|&i| row_files[i] == big_file), "EA.6 crowds");
}
