//! Ranking experiment, 2026-10-18 (write-up: dev-notes/ranking-eval-2026-10-18.md).
//!
//! The question: which file ranking should the change / find / pack answers use?
//! Today they order by Personalised PageRank (damping 0.5), never measured
//! against a baseline. This builds ONE repo's graph in memory (no layout, cache
//! or file is written), replays that repo's git history read-only, and scores
//! every ranker on one task: given one changed file of a commit, rank every
//! other code file so the commit's other changed files come first.
//!
//! ```text
//! GLIA_NO_PERSIST=1 cargo run --release -p glia-engine --example rank_eval -- \
//!     --repo <git-archive copy> --git <live checkout> --rev <sha> --label <name> \
//!     --out <result.json> --md <tables.md> [--max-commits 300] [--max-seeds 5] \
//!     [--timing 40] [--boot 1000] [--push-eps 1e-6]
//! ```
//!
//! Leakage: co-change counts, edge co-change support (U*) and the BM25 past-task
//! corpus (T*) for a test commit are built from commits strictly older than it
//! (oldest -> newest replay over `git log --topo-order`). The graph is the
//! snapshot's for every ranker alike.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::process::Command;
use std::time::Instant;

use glia_activation::algo::community::{self as comm, CommunityOptions, WeightedGraph};
use glia_activation::algo::ppr::PushPpr;
use glia_activation::algo::reach;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_activation::{ActivationConfig, ActivationPlan, Direction};
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::{cell_type, edge_category as ec, node_kind};
use glia_core::{CellPayload, Edge, EdgeCategoryId, NodeId};
use glia_engine::{Locator, default_keying, generate_one, service_of};
use glia_graph::MergedGraph;
use rayon::prelude::*;
use serde_json::json;

/// A ranker's sort key per file: compared lexicographically, descending; an
/// exact tie goes to the smaller path.
type Key = [f64; 3];
const NONE: u32 = u32::MAX;
/// A commit with more code files than this is neither a test commit nor
/// training data (co-change or past task).
const MAX_FILES: usize = 30;
const TOP_TASKS: usize = 20;
const RRF_K: f64 = 60.0;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
const U_BETAS: [f64; 2] = [1.0, 5.0];
const ALPHAS: [f64; 3] = [0.25, 0.5, 0.75];

/// Extensions a MODULE node may sit on that are not code (docs, data,
/// lockfiles, manifests). Everything else with a MODULE node is a candidate.
const NOT_CODE: &[&str] = &[
    "md", "mdx", "markdown", "rst", "txt", "json", "jsonc", "lock", "yaml", "yml", "toml", "csv", "tsv",
    "map", "svg", "png", "jpg", "gif", "ico", "xml", "ini", "cfg", "conf", "env", "example", "sum", "mod",
    "gitkeep", "pdf", "snap", "golden", "out", "log", "html", "css", "scss",
];

const STOP: &[&str] = &[
    "about", "above", "after", "again", "against", "all", "also", "and", "any", "are", "because", "been",
    "before", "being", "below", "between", "both", "but", "can", "could", "did", "does", "doing", "down",
    "during", "each", "few", "for", "from", "further", "had", "has", "have", "having", "her", "here", "hers",
    "him", "his", "how", "into", "its", "itself", "just", "more", "most", "not", "now", "off", "once", "only",
    "other", "our", "ours", "out", "over", "own", "same", "she", "should", "some", "such", "than", "that",
    "the", "their", "them", "then", "there", "these", "they", "this", "those", "through", "too", "under",
    "until", "very", "was", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will",
    "with", "would", "you", "your", "yours", "yet", "via", "per", "one", "two", "new", "use", "used",
    "uses", "using", "get", "got", "let", "may", "might", "must", "shall", "upon", "within", "without",
    "onto", "etc",
];

// ============================================================================
// Args
// ============================================================================

struct Args {
    repo: String,
    git: String,
    rev: String,
    label: String,
    out: String,
    md: String,
    max_commits: usize,
    max_seeds: usize,
    timing: usize,
    boot: usize,
    push_eps: f64,
    /// Second run: `kina`, `glia` or `quokka` adds the vertical (upper-layer) rankers.
    vertical: String,
    /// Kina only: the wiki clone (its own git history), the doc layer after the move out.
    wiki: String,
    /// The vertical tables (markdown).
    vmd: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        repo: String::new(),
        git: String::new(),
        rev: "HEAD".into(),
        label: "repo".into(),
        out: "rank_eval.json".into(),
        md: "rank_eval.md".into(),
        max_commits: 300,
        max_seeds: 5,
        timing: 40,
        boot: 1000,
        push_eps: 1e-6,
        vertical: String::new(),
        wiki: String::new(),
        vmd: "rank_eval_vertical.md".into(),
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let v = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--repo" => a.repo = v,
            "--git" => a.git = v,
            "--rev" => a.rev = v,
            "--label" => a.label = v,
            "--out" => a.out = v,
            "--md" => a.md = v,
            "--max-commits" => a.max_commits = v.parse().expect("--max-commits"),
            "--max-seeds" => a.max_seeds = v.parse().expect("--max-seeds"),
            "--timing" => a.timing = v.parse().expect("--timing"),
            "--boot" => a.boot = v.parse().expect("--boot"),
            "--push-eps" => a.push_eps = v.parse().expect("--push-eps"),
            "--vertical" => a.vertical = v,
            "--wiki" => a.wiki = v,
            "--vmd" => a.vmd = v,
            other => panic!("unknown argument {other}"),
        }
        i += 2;
    }
    assert!(!a.repo.is_empty() && !a.git.is_empty(), "--repo and --git are required");
    a
}

// ============================================================================
// Small helpers
// ============================================================================

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn h64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    mix(h)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }
}

fn ext_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn top2(path: &str) -> String {
    let segs: Vec<&str> = dir_of(path).split('/').filter(|s| !s.is_empty()).collect();
    segs.iter().take(2).copied().collect::<Vec<_>>().join("/")
}

fn shared_dir_segments(a: &str, b: &str) -> usize {
    dir_of(a).split('/').zip(dir_of(b).split('/')).take_while(|(x, y)| x == y && !x.is_empty()).count()
}

fn tokens(msg: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in msg.lines() {
        let l = line.trim_start().to_ascii_lowercase();
        if l.starts_with("co-authored-by:") || l.starts_with("claude-session:") || l.starts_with("signed-off-by:") {
            continue;
        }
        for t in l.split(|c: char| !c.is_alphanumeric()) {
            if t.chars().count() >= 3 && !STOP.contains(&t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn payload_text(p: &CellPayload) -> &str {
    match p {
        CellPayload::Text(s) | CellPayload::Json(s) => s,
        CellPayload::Bytes(_) => "",
    }
}

fn better(a: &Key, ia: usize, b: &Key, ib: usize) -> bool {
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

/// File indices other than `seed`, best first.
fn order(key: &[Key], seed: usize) -> Vec<usize> {
    let mut ix: Vec<usize> = (0..key.len()).filter(|&f| f != seed).collect();
    ix.sort_by(|&a, &b| {
        if better(&key[a], a, &key[b], b) {
            std::cmp::Ordering::Less
        } else if a == b {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ix
}

fn scalar(v: &[f64]) -> Vec<Key> {
    v.iter().map(|&s| [s, 0.0, 0.0]).collect()
}

/// Min-max over the candidates (every file but the seed); a flat vector is 0.
fn minmax(v: &[f64], seed: usize) -> Vec<f64> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (f, &s) in v.iter().enumerate() {
        if f != seed {
            lo = lo.min(s);
            hi = hi.max(s);
        }
    }
    if !(hi > lo) {
        return vec![0.0; v.len()];
    }
    v.iter().map(|&s| (s - lo) / (hi - lo)).collect()
}

/// Rank normalisation: the best candidate 1.0, the worst 0.0.
fn ranknorm(key: &[Key], seed: usize) -> Vec<f64> {
    let ord = order(key, seed);
    let m = ord.len().max(2) as f64 - 1.0;
    let mut out = vec![0.0; key.len()];
    for (pos, &f) in ord.iter().enumerate() {
        out[f] = (m - pos as f64) / m;
    }
    out
}

/// Reciprocal-rank fusion: each list contributes 1 / (60 + rank) for the
/// files it ranks positively (`positive[f]`), nothing for the rest.
fn rrf(lists: &[(&[Key], &[bool])], seed: usize) -> Vec<f64> {
    let n = lists.first().map_or(0, |l| l.0.len());
    let mut out = vec![0.0; n];
    for (key, pos) in lists {
        let mut r = 0usize;
        for f in order(key, seed) {
            if pos[f] {
                r += 1;
                out[f] += 1.0 / (RRF_K + r as f64);
            }
        }
    }
    out
}

fn positive(v: &[f64]) -> Vec<bool> {
    v.iter().map(|&s| s > 0.0).collect()
}

fn blend(a: f64, x: &[f64], y: &[f64]) -> Vec<f64> {
    x.iter().zip(y).map(|(p, q)| a * p + (1.0 - a) * q).collect()
}

/// recall@5, recall@10, recall@20, MRR, nDCG@20 for one query.
fn evaluate(key: &[Key], seed: usize, targets: &[u32]) -> [f64; 5] {
    let mut ranks: Vec<usize> = targets
        .iter()
        .map(|&t| {
            let t = t as usize;
            let kt = key[t];
            let mut r = 1;
            for (f, kf) in key.iter().enumerate() {
                if f != seed && f != t && better(kf, f, &kt, t) {
                    r += 1;
                }
            }
            r
        })
        .collect();
    ranks.sort_unstable();
    let nt = targets.len() as f64;
    let rec = |k: usize| ranks.iter().filter(|&&r| r <= k).count() as f64 / nt;
    let mrr = 1.0 / ranks[0] as f64;
    let dcg: f64 = ranks.iter().filter(|&&r| r <= 20).map(|&r| 1.0 / ((r + 1) as f64).log2()).sum();
    let idcg: f64 = (1..=targets.len().min(20)).map(|i| 1.0 / ((i + 1) as f64).log2()).sum();
    [rec(5), rec(10), rec(20), mrr, dcg / idcg]
}

// ============================================================================
// Graph index
// ============================================================================

struct Graph {
    paths: Vec<String>,
    file_nodes: Vec<Vec<u32>>,
    file_deg: Vec<f64>,
    ids: Vec<NodeId>,
    node_file: Vec<u32>,
    /// The node's index in `merged.node_ids()` that PPR reads (last occurrence).
    node_ppr: Vec<usize>,
    node_by_id: HashMap<NodeId, u32>,
    deg_all: Vec<f64>,
    excluded_modules: BTreeMap<String, usize>,
    generated_modules: usize,
}

fn index_graph(merged: &MergedGraph, all_ids: &[NodeId], id_to_idx: &HashMap<NodeId, usize>, edges: &[&Edge]) -> Graph {
    let loc = Locator::new(merged);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut files: Vec<Option<String>> = Vec::new();
    let mut module_files: BTreeSet<String> = BTreeSet::new();
    let mut generated: BTreeSet<String> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !seen.insert(n.id) {
                continue;
            }
            let file = loc.locate(n.id).file;
            if g.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::MODULE)
                && let Some(f) = &file
            {
                let gen_origin = n
                    .cells
                    .iter()
                    .any(|c| c.kind == cell_type::ORIGIN && payload_text(&c.payload).contains("generated"));
                if gen_origin {
                    generated.insert(f.clone());
                } else {
                    module_files.insert(f.clone());
                }
            }
            ids.push(n.id);
            files.push(file);
        }
    }
    let mut excluded_modules: BTreeMap<String, usize> = BTreeMap::new();
    let mut paths: Vec<String> = Vec::new();
    for f in &module_files {
        let e = ext_of(f);
        if generated.contains(f) {
            continue;
        }
        if NOT_CODE.contains(&e.as_str()) || e.is_empty() {
            *excluded_modules.entry(if e.is_empty() { "(none)".into() } else { e }).or_default() += 1;
        } else {
            paths.push(f.clone());
        }
    }
    paths.sort();
    let pidx: HashMap<&str, u32> = paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut file_nodes = vec![Vec::new(); paths.len()];
    let mut node_file = Vec::with_capacity(ids.len());
    for (u, f) in files.iter().enumerate() {
        let fi = f.as_deref().and_then(|p| pidx.get(p).copied()).unwrap_or(NONE);
        if fi != NONE {
            file_nodes[fi as usize].push(u as u32);
        }
        node_file.push(fi);
    }
    let node_by_id: HashMap<NodeId, u32> = ids.iter().enumerate().map(|(u, &id)| (id, u as u32)).collect();
    let node_ppr: Vec<usize> = ids.iter().map(|id| id_to_idx[id]).collect();
    let mut deg_all = vec![0.0; ids.len()];
    for e in edges {
        if let Some(&u) = node_by_id.get(&e.from) {
            deg_all[u as usize] += 1.0;
        }
        if let Some(&u) = node_by_id.get(&e.to) {
            deg_all[u as usize] += 1.0;
        }
    }
    let file_deg: Vec<f64> = file_nodes.iter().map(|ns| ns.iter().map(|&u| deg_all[u as usize]).sum()).collect();
    let _ = all_ids;
    Graph {
        paths,
        file_nodes,
        file_deg,
        ids,
        node_file,
        node_ppr,
        node_by_id,
        deg_all,
        excluded_modules,
        generated_modules: generated.len(),
    }
}

// ============================================================================
// PPR: the production power iteration, replicated over a prebuilt index
// ============================================================================

/// `glia_activation::ppr_vector`'s arithmetic in its operation order over an
/// index built once per (direction, weights), so a query costs the iterations
/// only. Verified against `ActivationPlan::run` in the timing pass.
struct Ppr {
    n: usize,
    damping: f64,
    max_iter: usize,
    eps: f64,
    start: Vec<usize>,
    inc: Vec<(usize, f64)>,
    /// Per incidence: the inter-file pair of its edge, or NONE.
    pair: Vec<u32>,
    out_w: Vec<f64>,
    dangling: Vec<usize>,
}

impl Ppr {
    fn build(n: usize, id_to_idx: &HashMap<NodeId, usize>, edges: &[&Edge], edge_pair: &[u32], cfg: &ActivationConfig) -> Self {
        let mut events: Vec<(usize, usize, f64, u32)> = Vec::new();
        let mut out_w = vec![0.0; n];
        for (k, e) in edges.iter().enumerate() {
            let w = cfg.edge_weights.get(&e.category).copied().unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            let (Some(&fi), Some(&ti)) = (id_to_idx.get(&e.from), id_to_idx.get(&e.to)) else {
                continue;
            };
            let p = edge_pair[k];
            match cfg.direction {
                Direction::Forward => {
                    events.push((ti, fi, w, p));
                    out_w[fi] += w;
                }
                Direction::Backward => {
                    events.push((fi, ti, w, p));
                    out_w[ti] += w;
                }
                Direction::Undirected => {
                    events.push((ti, fi, w, p));
                    events.push((fi, ti, w, p));
                    out_w[fi] += w;
                    out_w[ti] += w;
                }
            }
        }
        let mut start = vec![0usize; n + 1];
        for e in &events {
            start[e.0 + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut cursor = start.clone();
        let mut inc = vec![(0usize, 0.0f64); events.len()];
        let mut pair = vec![NONE; events.len()];
        for e in &events {
            let c = cursor[e.0];
            inc[c] = (e.1, e.2);
            pair[c] = e.3;
            cursor[e.0] += 1;
        }
        let dangling = (0..n).filter(|&j| out_w[j] == 0.0).collect();
        Ppr { n, damping: cfg.damping, max_iter: cfg.max_iterations, eps: cfg.epsilon, start, inc, pair, out_w, dangling }
    }

    fn uniform(seeds: &[usize]) -> Vec<(usize, f64)> {
        let w = 1.0 / seeds.len() as f64;
        seeds.iter().map(|&s| (s, w)).collect()
    }

    fn run(&self, pers: &[(usize, f64)]) -> Vec<f64> {
        self.power(pers, &self.inc, &self.out_w)
    }

    /// The incidences reweighted by `w * (1 + beta * s(pair))`, and the
    /// out-weights they sum to.
    fn reweight(&self, s: &[f32], beta: f64) -> (Vec<(usize, f64)>, Vec<f64>) {
        let mut out_w = vec![0.0; self.n];
        let inc: Vec<(usize, f64)> = self
            .inc
            .iter()
            .zip(&self.pair)
            .map(|(&(j, w), &p)| {
                let w2 = if p == NONE { w } else { w * (1.0 + beta * f64::from(s[p as usize])) };
                out_w[j] += w2;
                (j, w2)
            })
            .collect();
        (inc, out_w)
    }

    fn power(&self, pers: &[(usize, f64)], inc: &[(usize, f64)], out_w: &[f64]) -> Vec<f64> {
        let n = self.n;
        if n == 0 || pers.is_empty() {
            return vec![0.0; n];
        }
        let mut personalization = vec![0.0; n];
        for &(i, w) in pers {
            personalization[i] = w;
        }
        let d = self.damping.clamp(0.01, 0.99);
        let mut scores = personalization.clone();
        let mut new_scores = vec![0.0; n];
        for _ in 0..self.max_iter {
            let dangling_sum: f64 = self.dangling.iter().map(|&j| scores[j]).sum();
            for i in 0..n {
                let propagated: f64 = inc[self.start[i]..self.start[i + 1]]
                    .iter()
                    .map(|&(j, w)| if out_w[j] > 0.0 { scores[j] * w / out_w[j] } else { 0.0 })
                    .sum();
                new_scores[i] = (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
            }
            let diff: f64 = scores.iter().zip(new_scores.iter()).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut scores, &mut new_scores);
            if diff < self.eps {
                break;
            }
        }
        scores
    }
}

// ============================================================================
// History replay
// ============================================================================

struct Commit {
    sha: String,
    msg: String,
    files: Vec<u32>,
}

struct Query {
    tc: usize,
    seed: u32,
    targets: Vec<u32>,
    c0: Vec<(u32, f64)>,
    t2: Vec<(u32, f64)>,
}

struct TestCommit {
    t1: Vec<(u32, f64)>,
    t0: Vec<(u32, f64)>,
    /// Edge-pair co-change support (Jaccard) over strictly older commits.
    s_pairs: Vec<(u32, f32)>,
}

#[derive(Default)]
struct Bm25 {
    post: HashMap<String, Vec<(u32, u32)>>,
    dl: Vec<u32>,
    total: u64,
    files: Vec<Vec<u32>>,
    toks: Vec<Vec<String>>,
}

impl Bm25 {
    fn add(&mut self, toks: Vec<String>, files: Vec<u32>) {
        let d = self.dl.len() as u32;
        let mut tf: BTreeMap<&str, u32> = BTreeMap::new();
        for t in &toks {
            *tf.entry(t.as_str()).or_default() += 1;
        }
        for (t, c) in tf {
            self.post.entry(t.to_string()).or_default().push((d, c));
        }
        self.dl.push(toks.len() as u32);
        self.total += toks.len() as u64;
        self.files.push(files);
        self.toks.push(toks);
    }

    fn scores(&self, q: &[String]) -> Vec<f64> {
        let n = self.dl.len();
        let mut s = vec![0.0; n];
        if n == 0 {
            return s;
        }
        let avg = (self.total as f64 / n as f64).max(1e-9);
        let uniq: BTreeSet<&String> = q.iter().collect();
        for t in uniq {
            let Some(pl) = self.post.get(t) else { continue };
            let df = pl.len() as f64;
            let idf = (1.0 + (n as f64 - df + 0.5) / (df + 0.5)).ln();
            for &(d, tf) in pl {
                let tf = f64::from(tf);
                let dl = f64::from(self.dl[d as usize]);
                s[d as usize] += idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg));
            }
        }
        s
    }

    /// The top tasks (score > 0, `keep`), by score then newest first, as
    /// file scores: the sum over those tasks of the task's score.
    fn file_scores(&self, s: &[f64], keep: impl Fn(usize) -> bool) -> Vec<(u32, f64)> {
        let mut top: Vec<(usize, f64)> = s.iter().copied().enumerate().filter(|&(d, v)| v > 0.0 && keep(d)).collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        top.truncate(TOP_TASKS);
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for (d, v) in top {
            for &f in &self.files[d] {
                *acc.entry(f).or_default() += v;
            }
        }
        acc.into_iter().collect()
    }
}

fn git_log(git: &str, rev: &str, pidx: &HashMap<&str, u32>) -> (Vec<Commit>, usize) {
    let out = Command::new("git")
        .args([
            "-C",
            git,
            "log",
            "--topo-order",
            "--no-merges",
            "--no-renames",
            "--name-only",
            "--format=%x1e%H%x1f%B%x1f",
            rev,
        ])
        .output()
        .expect("git log");
    assert!(out.status.success(), "git log failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    let mut raw = 0;
    for rec in text.split('\x1e').skip(1) {
        raw += 1;
        let mut parts = rec.splitn(3, '\x1f');
        let sha = parts.next().unwrap_or("").trim().to_string();
        let msg = parts.next().unwrap_or("").to_string();
        let names = parts.next().unwrap_or("");
        let mut files: Vec<u32> = names.lines().filter_map(|l| pidx.get(l.trim()).copied()).collect();
        files.sort_unstable();
        files.dedup();
        commits.push(Commit { sha, msg, files });
    }
    commits.reverse();
    (commits, raw)
}

// ============================================================================
// Rankers per query
// ============================================================================

struct Ctx<'a> {
    merged: &'a MergedGraph,
    g: &'a Graph,
    ppr: Vec<(&'static str, Ppr)>,
    upr: Ppr,
    adj_carry: Adjacency,
    adj_imports: Adjacency,
    adj_push: Adjacency,
    push_w: HashMap<EdgeCategoryId, f64>,
    clusters: Vec<(&'static str, Vec<u32>, usize)>,
    path_hash: Vec<f64>,
    hubs: Vec<bool>,
    n_pairs: usize,
    push_eps: f64,
    /// Inter-file pairs joined by any edge (the vertical pair diagnostics).
    direct: HashSet<(u32, u32)>,
}

#[derive(Default)]
struct Out {
    names: Vec<String>,
    m: Vec<[f64; 5]>,
    hub_share: Vec<(String, f64)>,
    /// Per vertical signal: [linked targets, targets, linked targets with no direct edge, targets with no direct
    /// edge, linked other candidates, other candidates].
    vdiag: Vec<(String, [f64; 6])>,
}

impl Out {
    fn push(&mut self, name: impl Into<String>, key: &[Key], seed: usize, targets: &[u32]) {
        self.names.push(name.into());
        self.m.push(evaluate(key, seed, targets));
    }
}

fn dense(nf: usize, sparse: &[(u32, f64)], seed: usize) -> Vec<f64> {
    let mut v = vec![0.0; nf];
    for &(f, s) in sparse {
        if f as usize != seed {
            v[f as usize] = s;
        }
    }
    v
}

/// File score = max (or sum) over the file's nodes of `score(node)`.
fn file_pool(g: &Graph, score: impl Fn(usize) -> f64, sum: bool) -> Vec<f64> {
    g.file_nodes
        .iter()
        .map(|ns| {
            let mut acc = 0.0f64;
            for &u in ns {
                let s = score(u as usize);
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

fn hub_share(v: &[f64], hubs: &[bool], seed: usize) -> f64 {
    let ord = order(&scalar(v), seed);
    let top: Vec<usize> = ord.into_iter().take(20).collect();
    let mass: f64 = top.iter().map(|&f| v[f]).sum();
    if mass <= 0.0 {
        return 0.0;
    }
    top.iter().filter(|&&f| hubs[f]).map(|&f| v[f]).sum::<f64>() / mass
}

fn bfs_key(ctx: &Ctx, adj: &Adjacency, seed_ids: &[NodeId]) -> (Vec<Key>, Vec<f64>) {
    let g = ctx.g;
    let nf = g.paths.len();
    let b = reach::bfs(adj, seed_ids, Walk::Both, usize::MAX);
    let mut depth = vec![f64::INFINITY; nf];
    for r in &b.reached {
        if let Some(&u) = g.node_by_id.get(&r.id) {
            let f = g.node_file[u as usize];
            if f != NONE {
                let d = &mut depth[f as usize];
                *d = d.min(r.depth as f64);
            }
        }
    }
    let key = (0..nf).map(|f| [if depth[f].is_finite() { -depth[f] } else { -1e9 }, g.file_deg[f], 0.0]).collect();
    (key, depth)
}

fn push_scores(ctx: &Ctx, push: &mut PushPpr, seed_ids: &[NodeId]) -> Vec<f64> {
    let g = ctx.g;
    let mut acc = vec![0.0; g.ids.len()];
    let k = seed_ids.len().max(1) as f64;
    for &s in seed_ids {
        let (scores, _) = push.run(s, 0.5, ctx.push_eps);
        for (id, p) in scores {
            if let Some(&u) = g.node_by_id.get(&id) {
                acc[u as usize] += p / k;
            }
        }
    }
    file_pool(g, |u| acc[u], false)
}

fn pooled(base: &[f64], cl: &[u32], m: usize) -> Vec<Key> {
    let mut cs = vec![0.0; m];
    for (f, &s) in base.iter().enumerate() {
        cs[cl[f] as usize] += s;
    }
    base.iter().enumerate().map(|(f, &s)| [cs[cl[f] as usize], s, 0.0]).collect()
}

fn run_query(ctx: &Ctx, q: &Query, tc: &TestCommit, push: &mut PushPpr, vl: Option<&VLayer>) -> Out {
    let g = ctx.g;
    let nf = g.paths.len();
    let seed = q.seed as usize;
    let t = &q.targets;
    let seed_nodes: Vec<u32> = g.file_nodes[seed].clone();
    let seed_ids: Vec<NodeId> = seed_nodes.iter().map(|&u| g.ids[u as usize]).collect();
    let seed_ppr: Vec<usize> = seed_nodes.iter().map(|&u| g.node_ppr[u as usize]).collect();
    let mut out = Out::default();

    // ---- structural rankers (graph or path only) ----
    // (name, key, is_scalar)
    let mut structural: Vec<(String, Vec<Key>, bool)> = Vec::new();
    let (b0, depth) = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
    structural.push(("B0".into(), b0, false));
    let (b0i, _) = bfs_key(ctx, &ctx.adj_imports, &seed_ids);
    structural.push(("B0i".into(), b0i, false));
    structural.push(("B1".into(), scalar(&g.file_deg), true));
    let b3: Vec<Key> =
        (0..nf).map(|f| [shared_dir_segments(&g.paths[seed], &g.paths[f]) as f64, g.file_deg[f], 0.0]).collect();
    structural.push(("B3".into(), b3, false));

    let pers = Ppr::uniform(&seed_ppr);
    let mut p1 = Vec::new();
    let mut p1_vec = Vec::new();
    for (name, idx) in &ctx.ppr {
        let v = idx.run(&pers);
        let fs = file_pool(g, |u| v[g.node_ppr[u]], false);
        if *name == "P1" {
            let fsum = file_pool(g, |u| v[g.node_ppr[u]], true);
            structural.push(("P1".into(), scalar(&fs), true));
            structural.push(("P1sum".into(), scalar(&fsum), true));
            p1 = fs;
            p1_vec = v;
        } else {
            structural.push(((*name).into(), scalar(&fs), true));
        }
    }
    let p2 = file_pool(g, |u| p1_vec[g.node_ppr[u]] / (1.0 + g.deg_all[u]), false);
    let p2i = file_pool(g, |u| p1_vec[g.node_ppr[u]] * (1.0 + g.deg_all[u]).ln_1p(), false);
    structural.push(("P2".into(), scalar(&p2), true));
    structural.push(("P2i".into(), scalar(&p2i), true));
    let p3 = push_scores(ctx, push, &seed_ids);
    structural.push(("P3".into(), scalar(&p3), true));
    for (cname, cl, m) in &ctx.clusters {
        structural.push((format!("K1{cname}"), pooled(&p1, cl, *m), false));
    }
    for (cname, cl, m) in &ctx.clusters {
        structural.push((format!("K2{cname}"), pooled(&p2, cl, *m), false));
    }
    structural.push(("B2".into(), scalar(&ctx.path_hash), true));
    for (name, key, _) in &structural {
        out.push(name.clone(), key, seed, t);
    }

    // ---- co-change and its late-fusion hybrids ----
    let c0 = dense(nf, &q.c0, seed);
    out.push("C0", &scalar(&c0), seed, t);
    let mm_p1 = minmax(&p1, seed);
    let mm_p2 = minmax(&p2, seed);
    let mm_c0 = minmax(&c0, seed);
    let rn_p1 = ranknorm(&scalar(&p1), seed);
    let rn_c0 = ranknorm(&scalar(&c0), seed);
    for a in ALPHAS {
        out.push(format!("H1a{}", (a * 100.0) as u32), &scalar(&blend(a, &mm_p1, &mm_c0)), seed, t);
    }
    for a in ALPHAS {
        out.push(format!("H1r{}", (a * 100.0) as u32), &scalar(&blend(a, &rn_p1, &rn_c0)), seed, t);
    }
    for a in ALPHAS {
        out.push(format!("H2a{}", (a * 100.0) as u32), &scalar(&blend(a, &mm_p2, &mm_c0)), seed, t);
    }
    let k_p1 = scalar(&p1);
    let k_c0 = scalar(&c0);
    let pos_p1 = positive(&p1);
    let pos_c0 = positive(&c0);
    out.push("H3", &scalar(&rrf(&[(&k_p1, &pos_p1), (&k_c0, &pos_c0)], seed)), seed, t);
    let h4: Vec<Key> = (0..nf)
        .map(|f| if depth[f] <= 3.0 { [1.0, c0[f], p1[f]] } else { [0.0, 0.0, p1[f]] })
        .collect();
    out.push("H4", &h4, seed, t);
    for (name, key, _) in structural.iter().filter(|(n, _, _)| n.starts_with('K')) {
        let pos: Vec<bool> = key.iter().map(|k| k[0] > 0.0 || k[1] > 0.0).collect();
        out.push(format!("H5[{name}]"), &scalar(&rrf(&[(key, &pos), (&k_c0, &pos_c0)], seed)), seed, t);
    }
    let h10: Vec<Key> = (0..nf).map(|f| [c0[f], p1[f], 0.0]).collect();
    out.push("H10", &h10, seed, t);

    // ---- past tasks (intent) ----
    let t1 = dense(nf, &tc.t1, seed);
    let t0 = dense(nf, &tc.t0, seed);
    let t2 = dense(nf, &q.t2, seed);
    out.push("T0", &scalar(&t0), seed, t);
    out.push("T1", &scalar(&t1), seed, t);
    out.push("T2", &scalar(&t2), seed, t);
    let k_t1 = scalar(&t1);
    let pos_t1 = positive(&t1);
    out.push("H6", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_p1, &pos_p1)], seed)), seed, t);
    out.push("H7", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0)], seed)), seed, t);
    out.push(
        "H8",
        &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (&k_p1, &pos_p1)], seed)),
        seed,
        t,
    );
    let mm_t1 = minmax(&t1, seed);
    for (name, key, is_scalar) in &structural {
        if name == "B2" {
            continue;
        }
        let norm = if *is_scalar { minmax(&key.iter().map(|k| k[0]).collect::<Vec<_>>(), seed) } else { ranknorm(key, seed) };
        for a in ALPHAS {
            out.push(format!("H9a{}[{name}]", (a * 100.0) as u32), &scalar(&blend(a, &mm_t1, &norm)), seed, t);
        }
    }
    // Post-hoc: equal-weight min-max blend of T1, C0 and P1.
    let h18: Vec<f64> = (0..nf).map(|f| (mm_t1[f] + mm_c0[f] + mm_p1[f]) / 3.0).collect();
    out.push("H18", &scalar(&h18), seed, t);
    // T1 then C0 then P1, lexicographic; and the three-way RRF with T2.
    let h11: Vec<Key> = (0..nf).map(|f| [t1[f], c0[f], p1[f]]).collect();
    out.push("H11", &h11, seed, t);
    let k_t2 = scalar(&t2);
    let pos_t2 = positive(&t2);
    out.push(
        "H12",
        &scalar(&rrf(&[(&k_t2, &pos_t2), (&k_c0, &pos_c0), (&k_p1, &pos_p1)], seed)),
        seed,
        t,
    );

    // ---- use-weighted PPR (early fusion) ----
    let mut s = vec![0.0f32; ctx.n_pairs];
    for &(p, v) in &tc.s_pairs {
        s[p as usize] = v;
    }
    let mut t1_top: Vec<usize> = order(&k_t1, seed).into_iter().filter(|&f| t1[f] > 0.0).take(10).collect();
    t1_top.sort_unstable();
    // U2: every seed node at 1, every node of a top-10 T1 file at 0.25.
    let mut pers_node: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0)).collect();
    // U2f: the seed file at 1 and each top-10 T1 file at 0.25, spread over its nodes.
    let mut pers_file: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0 / seed_ppr.len() as f64)).collect();
    for &f in &t1_top {
        let ns = &g.file_nodes[f];
        for &u in ns {
            pers_node.push((g.node_ppr[u as usize], 0.25));
            pers_file.push((g.node_ppr[u as usize], 0.25 / ns.len() as f64));
        }
    }
    let norm_pers = |p: &[(usize, f64)]| -> Vec<(usize, f64)> {
        let tot: f64 = p.iter().map(|x| x.1).sum();
        p.iter().map(|&(i, w)| (i, w / tot)).collect()
    };
    let pers_node = norm_pers(&pers_node);
    let pers_file = norm_pers(&pers_file);
    let mut u_scores: Vec<(String, Vec<f64>)> = Vec::new();
    for beta in U_BETAS {
        let (inc, ow) = ctx.upr.reweight(&s, beta);
        let b = beta as u32;
        let v1 = ctx.upr.power(&pers, &inc, &ow);
        u_scores.push((format!("U1b{b}"), file_pool(g, |u| v1[g.node_ppr[u]], false)));
        let v2 = ctx.upr.power(&pers_node, &inc, &ow);
        u_scores.push((format!("U2b{b}"), file_pool(g, |u| v2[g.node_ppr[u]], false)));
        let v3 = ctx.upr.power(&pers_file, &inc, &ow);
        u_scores.push((format!("U2fb{b}"), file_pool(g, |u| v3[g.node_ppr[u]], false)));
    }
    // U3 (post-hoc, after the first run): U1b5 with the top-10 T1 files in the
    // restart set weighted by their T1 score (0.25 x t1 / max t1, spread over
    // the file's nodes; the seed file at 1, spread), max- and sum-pooled.
    {
        let t1_max = t1_top.iter().map(|&f| t1[f]).fold(0.0f64, f64::max);
        let mut pers_t: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0 / seed_ppr.len() as f64)).collect();
        for &f in &t1_top {
            let ns = &g.file_nodes[f];
            for &u in ns {
                pers_t.push((g.node_ppr[u as usize], 0.25 * t1[f] / t1_max / ns.len() as f64));
            }
        }
        let pers_t = norm_pers(&pers_t);
        let (inc, ow) = ctx.upr.reweight(&s, 5.0);
        let v = ctx.upr.power(&pers_t, &inc, &ow);
        u_scores.push(("U3b5".into(), file_pool(g, |u| v[g.node_ppr[u]], false)));
        u_scores.push(("U3sb5".into(), file_pool(g, |u| v[g.node_ppr[u]], true)));
    }
    for (name, v) in &u_scores {
        out.push(name.clone(), &scalar(v), seed, t);
    }
    // Post-hoc late fusions: RRF(T1, C0, X) for stronger structural X.
    for (hname, base) in [("H13", "P1sum"), ("H14", "K1svc"), ("H16", "B0"), ("H17", "P2")] {
        let (_, key, _) = structural.iter().find(|(n, _, _)| n == base).expect("base");
        let pos: Vec<bool> = if base == "B0" {
            depth.iter().map(|d| d.is_finite()).collect()
        } else {
            key.iter().map(|k| k[0] > 0.0 || k[1] > 0.0).collect()
        };
        out.push(hname, &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (key, &pos)], seed)), seed, t);
    }
    if let Some((_, v)) = u_scores.iter().find(|(n, _)| n == "U1b5") {
        let k = scalar(v);
        let pos = positive(v);
        out.push("H15", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (&k, &pos)], seed)), seed, t);
    }

    // ---- vertical layers (second run; appended, so every ranker above is unchanged) ----
    if let Some(vl) = vl {
        vertical_rankers(&mut out, vl, seed, t, &p1, &t1, &ctx.direct);
    }

    // ---- hub flooding ----
    out.hub_share.push(("P1".into(), hub_share(&p1, &ctx.hubs, seed)));
    out.hub_share.push(("P2".into(), hub_share(&p2, &ctx.hubs, seed)));
    for (name, key, _) in &structural {
        if name == "P1f" || name == "P1b" || name == "P1review" {
            out.hub_share.push((name.clone(), hub_share(&key.iter().map(|k| k[0]).collect::<Vec<_>>(), &ctx.hubs, seed)));
        }
    }
    if let Some((_, v)) = u_scores.iter().find(|(n, _)| n == "U1b5") {
        out.hub_share.push(("U1b5".into(), hub_share(v, &ctx.hubs, seed)));
    }
    let _ = ctx.merged;
    out
}

// ============================================================================
// Paired statistics
// ============================================================================

struct Paired {
    mean: f64,
    lo: f64,
    hi: f64,
    p: f64,
    wins: usize,
    losses: usize,
}

fn ln_fact_table(n: usize) -> Vec<f64> {
    let mut t = vec![0.0; n + 1];
    for i in 1..=n {
        t[i] = t[i - 1] + (i as f64).ln();
    }
    t
}

fn sign_test(w: usize, l: usize, lf: &[f64]) -> f64 {
    let n = w + l;
    if n == 0 {
        return 1.0;
    }
    let k = w.min(l);
    let ln2n = n as f64 * std::f64::consts::LN_2;
    let tail: f64 = (0..=k).map(|i| (lf[n] - lf[i] - lf[n - i] - ln2n).exp()).sum();
    (2.0 * tail).min(1.0)
}

fn paired(a: &[f64], b: &[f64], commit_of: &[usize], n_commits: usize, boot: usize, lf: &[f64]) -> Paired {
    let diffs: Vec<f64> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let wins = diffs.iter().filter(|&&d| d > 1e-12).count();
    let losses = diffs.iter().filter(|&&d| d < -1e-12).count();
    let mut csum = vec![0.0; n_commits];
    let mut ccnt = vec![0usize; n_commits];
    for (q, &d) in diffs.iter().enumerate() {
        csum[commit_of[q]] += d;
        ccnt[commit_of[q]] += 1;
    }
    let mut rng = Rng(0x5eed_2026_1018);
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let (mut s, mut c) = (0.0, 0usize);
        for _ in 0..n_commits {
            let k = (rng.next() % n_commits as u64) as usize;
            s += csum[k];
            c += ccnt[k];
        }
        means.push(if c > 0 { s / c as f64 } else { 0.0 });
    }
    means.sort_by(f64::total_cmp);
    let lo = means[((boot as f64) * 0.025) as usize];
    let hi = means[(((boot as f64) * 0.975) as usize).min(boot - 1)];
    Paired { mean, lo, hi, p: sign_test(wins, losses, lf), wins, losses }
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let args = parse_args();
    let t_start = Instant::now();
    eprintln!("[rank_eval] {}: building {}", args.label, args.repo);
    let t_build = Instant::now();
    let res = generate_one(&args.repo).expect("build");
    let build_s = t_build.elapsed().as_secs_f64();
    let merged = &res.merged;
    let all_ids = merged.node_ids();
    let id_to_idx: HashMap<NodeId, usize> = all_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();
    let g = index_graph(merged, &all_ids, &id_to_idx, &edges);
    let nf = g.paths.len();
    eprintln!(
        "[rank_eval] {}: built in {build_s:.1}s nodes={} edges={} candidate_files={nf}",
        args.label,
        g.ids.len(),
        edges.len()
    );
    let pidx: HashMap<&str, u32> = g.paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut ext_hist: BTreeMap<String, usize> = BTreeMap::new();
    for p in &g.paths {
        *ext_hist.entry(ext_of(p)).or_default() += 1;
    }

    // ---- inter-file pairs joined by an edge ----
    let mut pair_index: HashMap<(u32, u32), u32> = HashMap::new();
    let mut pair_list: Vec<(u32, u32)> = Vec::new();
    let mut edge_pair: Vec<u32> = Vec::with_capacity(edges.len());
    let mut cat_pairs: BTreeMap<u32, HashSet<u32>> = BTreeMap::new();
    for e in &edges {
        let fa = g.node_by_id.get(&e.from).map_or(NONE, |&u| g.node_file[u as usize]);
        let fb = g.node_by_id.get(&e.to).map_or(NONE, |&u| g.node_file[u as usize]);
        if fa == NONE || fb == NONE || fa == fb {
            edge_pair.push(NONE);
            continue;
        }
        let key = (fa.min(fb), fa.max(fb));
        let next = pair_list.len() as u32;
        let p = *pair_index.entry(key).or_insert_with(|| {
            pair_list.push(key);
            next
        });
        edge_pair.push(p);
        cat_pairs.entry(e.category.0).or_default().insert(p);
    }

    // ---- PPR indexes ----
    let n_all = all_ids.len();
    let undirected = |preset: Option<&str>, dir: Direction| {
        let mut c = CODE_TABLES.activation_config(preset);
        c.direction = dir;
        c.top_k = usize::MAX;
        c
    };
    let base_cfg = undirected(None, Direction::Undirected);
    let ppr: Vec<(&'static str, Ppr)> = vec![
        ("P1", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &base_cfg)),
        ("P1f", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(None, Direction::Forward))),
        ("P1b", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(None, Direction::Backward))),
        ("P1repair", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("repair"), Direction::Undirected))),
        ("P1review", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("review"), Direction::Undirected))),
        ("P1onboard", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("onboard"), Direction::Undirected))),
    ];
    let upr = Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &base_cfg);
    let adj_carry = Adjacency::carry(merged, &CODE_TABLES);
    let mut carry_imp: Vec<EdgeCategoryId> = CODE_TABLES.carry_edges.to_vec();
    carry_imp.push(ec::IMPORTS);
    let adj_imports = Adjacency::build(merged, &CategorySet::of(&carry_imp));
    let push_w: HashMap<EdgeCategoryId, f64> = base_cfg.edge_weights.clone();
    let push_cats: Vec<EdgeCategoryId> =
        ec::ALL.iter().map(|(c, _)| *c).filter(|c| push_w.get(c).copied().unwrap_or(1.0) > 0.0).collect();
    let adj_push = Adjacency::build(merged, &CategorySet::of(&push_cats));

    // ---- clusters ----
    let t_leiden = Instant::now();
    let view = WeightedGraph::from_source(merged, CODE_TABLES.community_weights);
    let mut opts = CommunityOptions::default();
    opts.seed = 42;
    let part = comm::communities(&view, &opts);
    let leiden_s = t_leiden.elapsed().as_secs_f64();
    let mut votes: Vec<BTreeMap<u32, usize>> = vec![BTreeMap::new(); nf];
    for ix in 0..view.len() as u32 {
        if view.strength(ix) == 0 {
            continue;
        }
        if let Some(&u) = g.node_by_id.get(&view.id(ix)) {
            let f = g.node_file[u as usize];
            if f != NONE {
                *votes[f as usize].entry(part.membership[ix as usize]).or_default() += 1;
            }
        }
    }
    let mut leiden_cl: Vec<u32> = Vec::with_capacity(nf);
    let mut next_single = part.communities as u32;
    for v in &votes {
        let best = v.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(c, _)| *c);
        leiden_cl.push(best.unwrap_or_else(|| {
            next_single += 1;
            next_single - 1
        }));
    }
    let keying = default_keying(merged);
    let repo = merged.graphs.first().map_or(0, |g| g.repo.0);
    let label_ids = |labels: Vec<String>| -> (Vec<u32>, usize) {
        let mut ix: BTreeMap<String, u32> = BTreeMap::new();
        let v = labels
            .into_iter()
            .map(|l| {
                let n = ix.len() as u32;
                *ix.entry(l).or_insert(n)
            })
            .collect();
        (v, ix.len())
    };
    let (dir_cl, dir_m) = label_ids(g.paths.iter().map(|p| dir_of(p).to_string()).collect());
    let (top_cl, top_m) = label_ids(g.paths.iter().map(|p| top2(p)).collect());
    let (svc_cl, svc_m) =
        label_ids(g.paths.iter().map(|p| service_of(p, repo, &keying, &res.repo_labels)).collect());
    let leiden_m = next_single as usize;
    let n_leiden_used = leiden_cl.iter().collect::<BTreeSet<_>>().len();
    let clusters = vec![
        ("dir", dir_cl, dir_m),
        ("top2", top_cl, top_m),
        ("leiden", leiden_cl, leiden_m),
        ("svc", svc_cl, svc_m),
    ];
    let path_hash: Vec<f64> = g.paths.iter().map(|p| (h64(p) >> 11) as f64).collect();
    let mut by_deg: Vec<usize> = (0..nf).collect();
    by_deg.sort_by(|&a, &b| g.file_deg[b].total_cmp(&g.file_deg[a]).then_with(|| a.cmp(&b)));
    let mut hubs = vec![false; nf];
    for &f in by_deg.iter().take(5) {
        hubs[f] = true;
    }
    let hub_files: Vec<String> = by_deg.iter().take(5).map(|&f| format!("{} ({})", g.paths[f], g.file_deg[f])).collect();

    // ---- history replay ----
    let (commits, raw_commits) = git_log(&args.git, &args.rev, &pidx);
    let train_ok = |c: &Commit| !c.files.is_empty() && c.files.len() <= MAX_FILES;
    let test_ok = |c: &Commit| c.files.len() >= 2 && c.files.len() <= MAX_FILES;
    let eligible: Vec<usize> = (0..commits.len()).filter(|&k| test_ok(&commits[k])).collect();
    let test_idx: Vec<usize> = eligible[eligible.len().saturating_sub(args.max_commits)..].to_vec();
    let mut test_ord: HashMap<usize, usize> = HashMap::new();
    for (o, &k) in test_idx.iter().enumerate() {
        test_ord.insert(k, o);
    }
    let mut cnt = vec![0u32; nf];
    let mut co: Vec<HashMap<u32, u32>> = vec![HashMap::new(); nf];
    let mut bm = Bm25::default();
    let mut queries: Vec<Query> = Vec::new();
    let mut tcs: Vec<TestCommit> = Vec::new();
    let mut t_task = 0.0f64;
    let mut n_train = 0usize;
    for (k, c) in commits.iter().enumerate() {
        if let Some(&o) = test_ord.get(&k) {
            let t0i = Instant::now();
            let qtok = tokens(&c.msg);
            let sc = bm.scores(&qtok);
            let t1 = bm.file_scores(&sc, |_| true);
            let t0 = if bm.dl.is_empty() {
                Vec::new()
            } else {
                let pick = (h64(&format!("{}:t0", c.sha)) % bm.dl.len() as u64) as usize;
                let s0 = bm.scores(&bm.toks[pick].clone());
                bm.file_scores(&s0, |_| true)
            };
            let mut s_pairs = Vec::new();
            for (p, &(a, b)) in pair_list.iter().enumerate() {
                if let Some(&both) = co[a as usize].get(&b) {
                    let either = cnt[a as usize] + cnt[b as usize] - both;
                    s_pairs.push((p as u32, both as f32 / either as f32));
                }
            }
            let mut seeds: Vec<u32> = c.files.clone();
            seeds.sort_by_key(|&f| h64(&format!("{}:{}", c.sha, g.paths[f as usize])));
            seeds.truncate(args.max_seeds);
            for &sd in &seeds {
                let targets: Vec<u32> = c.files.iter().copied().filter(|&f| f != sd).collect();
                let denom = f64::from(cnt[sd as usize]) + 1.0;
                let mut c0: Vec<(u32, f64)> = co[sd as usize].iter().map(|(&f, &n)| (f, f64::from(n) / denom)).collect();
                c0.sort_unstable_by_key(|x| x.0);
                let t2 = bm.file_scores(&sc, |d| bm.files[d].binary_search(&sd).is_ok());
                queries.push(Query { tc: o, seed: sd, targets, c0, t2 });
            }
            tcs.push(TestCommit { t1, t0, s_pairs });
            t_task += t0i.elapsed().as_secs_f64();
        }
        if train_ok(c) {
            n_train += 1;
            for &a in &c.files {
                cnt[a as usize] += 1;
                for &b in &c.files {
                    if a != b {
                        *co[a as usize].entry(b).or_default() += 1;
                    }
                }
            }
            bm.add(tokens(&c.msg), c.files.clone());
        }
    }
    eprintln!(
        "[rank_eval] {}: commits={} train={} test_eligible={} test={} queries={}",
        args.label,
        commits.len(),
        n_train,
        eligible.len(),
        test_idx.len(),
        queries.len()
    );

    // ---- vertical layers (second run), read at each test commit's parent ----
    let vmode = VMode::parse(&args.vertical);
    let layers: Vec<VLayer> = match vmode {
        Some(m) => {
            let t_layers = Instant::now();
            let tests: Vec<(String, String)> =
                test_idx.iter().map(|&k| (commits[k].sha.clone(), commits[k].msg.clone())).collect();
            let l = build_layers(m, &args, &g.paths, &pidx, &tests);
            eprintln!("[rank_eval] {}: vertical layers ({m:?}) for {} test commits in {:.1}s", args.label, l.len(), t_layers.elapsed().as_secs_f64());
            // GLIA_RANK_VDUMP=<file>: the newest test commit's layer, file -> nodes per signal (inspection only).
            if let (Ok(path), Some(last)) = (std::env::var("GLIA_RANK_VDUMP"), l.last()) {
                let mut d = serde_json::Map::new();
                for (n, sig) in last.sigs.iter().chain(last.ctrls.iter()) {
                    let mut per = serde_json::Map::new();
                    for (f, set) in sig.sets.iter().enumerate() {
                        if !set.is_empty() {
                            per.insert(g.paths[f].clone(), json!(set.iter().map(|&x| sig.names[x as usize].clone()).collect::<Vec<_>>()));
                        }
                    }
                    d.insert(n.clone(), serde_json::Value::Object(per));
                }
                d.insert("_sha".into(), json!(tests.last().map(|t| t.0.clone())));
                d.insert("_vocab".into(), json!(last.vocab));
                std::fs::write(path, serde_json::to_string_pretty(&serde_json::Value::Object(d)).unwrap()).expect("vdump");
            }
            l
        }
        None => Vec::new(),
    };

    let ctx = Ctx {
        merged,
        g: &g,
        ppr,
        upr,
        adj_carry,
        adj_imports,
        adj_push,
        push_w,
        clusters,
        path_hash,
        hubs,
        n_pairs: pair_list.len(),
        direct: pair_list.iter().copied().collect(),
        push_eps: args.push_eps,
    };

    // ---- timing pass (sequential) + replica check against production ----
    let timing = timing_pass(&ctx, &queries, args.timing, &base_cfg);

    // ---- the evaluation (parallel over queries) ----
    let t_eval = Instant::now();
    let wf = |c: EdgeCategoryId| ctx.push_w.get(&c).copied().unwrap_or(1.0);
    let results: Vec<Out> = queries
        .par_iter()
        .map_init(|| PushPpr::new(&ctx.adj_push, &wf), |push, q| run_query(&ctx, q, &tcs[q.tc], push, layers.get(q.tc)))
        .collect();
    let eval_s = t_eval.elapsed().as_secs_f64();
    let names = results[0].names.clone();
    for r in &results {
        assert_eq!(r.names, names, "ranker order differs between queries");
    }
    let nq = results.len();
    let nr = names.len();
    let mean = |ri: usize, mi: usize| results.iter().map(|r| r.m[ri][mi]).sum::<f64>() / nq as f64;
    let metrics: Vec<[f64; 5]> = (0..nr).map(|ri| [0, 1, 2, 3, 4].map(|mi| mean(ri, mi))).collect();
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    let commit_of: Vec<usize> = queries.iter().map(|q| q.tc).collect();
    let lf = ln_fact_table(nq + 1);
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap_or_else(|| panic!("no ranker {n}"));
    let baselines = ["B0", "P1", "C0"];
    // (ranker, baseline, metric idx) -> Paired
    let mut paired_all: BTreeMap<(String, String, usize), Paired> = BTreeMap::new();
    for (ri, name) in names.iter().enumerate() {
        for bl in baselines {
            for mi in [1usize, 3] {
                let p = paired(&col(ri, mi), &col(pos(bl), mi), &commit_of, tcs.len(), args.boot, &lf);
                paired_all.insert((name.clone(), bl.to_string(), mi), p);
            }
        }
    }

    // ---- inefficiency map (full history to the snapshot) ----
    let mut cochanged: HashSet<(u32, u32)> = HashSet::new();
    for (a, m) in co.iter().enumerate() {
        for (&b, &n) in m {
            if (a as u32) < b && n > 0 {
                cochanged.insert((a as u32, b));
            }
        }
    }
    let all_pairs = (nf as f64) * (nf as f64 - 1.0) / 2.0;
    let base_rate = cochanged.len() as f64 / all_pairs;
    let mut ineff: Vec<serde_json::Value> = Vec::new();
    for (cat, ps) in &cat_pairs {
        let cid = EdgeCategoryId(*cat);
        if cid == ec::CONTAINS || cid == ec::DEFINES {
            continue;
        }
        let hit = ps.iter().filter(|&&p| cochanged.contains(&pair_list[p as usize])).count();
        let rate = hit as f64 / ps.len() as f64;
        ineff.push(json!({
            "category": ec::name(cid),
            "pairs": ps.len(),
            "cochanged_pairs": hit,
            "p_cochange": rate,
            "lift": if base_rate > 0.0 { rate / base_rate } else { 0.0 },
            "coverage": if cochanged.is_empty() { 0.0 } else { hit as f64 / cochanged.len() as f64 },
            "activation_weight": base_cfg.edge_weights.get(&cid).copied().unwrap_or(1.0),
            "carry": CODE_TABLES.carries(cid),
        }));
    }
    ineff.sort_by(|a, b| b["lift"].as_f64().unwrap().total_cmp(&a["lift"].as_f64().unwrap()));
    let mut hub: BTreeMap<String, f64> = BTreeMap::new();
    for r in &results {
        for (n, v) in &r.hub_share {
            *hub.entry(n.clone()).or_default() += v / nq as f64;
        }
    }

    // ---- stats ----
    let mean_targets = queries.iter().map(|q| q.targets.len()).sum::<usize>() as f64 / nq as f64;
    let mean_seed_nodes = queries.iter().map(|q| g.file_nodes[q.seed as usize].len()).sum::<usize>() as f64 / nq as f64;
    let chance_r10 = 10.0 / (nf as f64 - 1.0);
    let cold = queries.iter().filter(|q| q.c0.is_empty()).count();
    let t1_empty = tcs.iter().filter(|t| t.t1.is_empty()).count();
    let stats = json!({
        "label": args.label,
        "rev": args.rev,
        "nodes": g.ids.len(),
        "edges": edges.len(),
        "candidate_files": nf,
        "candidate_ext": ext_hist,
        "excluded_module_files_by_ext": g.excluded_modules,
        "generated_module_files": g.generated_modules,
        "inter_file_pairs_with_edge": pair_list.len(),
        "git_nonmerge_commits": raw_commits,
        "train_commits": n_train,
        "test_eligible_commits": eligible.len(),
        "test_commits": tcs.len(),
        "queries": nq,
        "mean_targets": mean_targets,
        "mean_seed_nodes": mean_seed_nodes,
        "chance_recall10": chance_r10,
        "queries_without_cochange_history": cold,
        "test_commits_without_past_task_hit": t1_empty,
        "leiden_communities_used": n_leiden_used,
        "services": svc_m,
        "service_keying": keying.name(),
        "hub_files": hub_files,
        "build_s": build_s,
        "leiden_s": leiden_s,
        "eval_s": eval_s,
        "task_replay_ms_per_test_commit": 1000.0 * t_task / tcs.len().max(1) as f64,
        "total_s": t_start.elapsed().as_secs_f64(),
        "cochanged_pairs": cochanged.len(),
        "random_pair_cochange_rate": base_rate,
    });
    let metrics_json: BTreeMap<String, serde_json::Value> = names
        .iter()
        .enumerate()
        .map(|(ri, n)| {
            let m = metrics[ri];
            (n.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}))
        })
        .collect();
    let paired_json: Vec<serde_json::Value> = paired_all
        .iter()
        .map(|((r, b, mi), p)| {
            json!({"ranker": r, "vs": b, "metric": if *mi == 1 { "r10" } else { "mrr" },
                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses})
        })
        .collect();
    let mut doc = json!({
        "stats": stats,
        "rankers": names,
        "metrics": metrics_json,
        "paired": paired_json,
        "inefficiency": ineff,
        "hub_share_top20_mass_on_top5_degree_files": hub,
        "timing_ms_per_query": timing,
    });
    if let Some(m) = vmode {
        let bi = BootIn { commit_of: &commit_of, boot: args.boot, lf: &lf };
        let (vj, vmd) = vertical_report(&args.label, m, &names, &results, &queries, &layers, &bi);
        doc["vertical"] = vj;
        std::fs::write(&args.vmd, vmd).expect("write vertical md");
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&doc).unwrap()).expect("write json");
    let md = render_md(&args.label, &doc, &names, &metrics, &paired_all);
    std::fs::write(&args.md, md).expect("write md");
    eprintln!("[rank_eval] {}: done in {:.1}s -> {} {}", args.label, t_start.elapsed().as_secs_f64(), args.out, args.md);
}

// ============================================================================
// Timing pass
// ============================================================================

fn timing_pass(ctx: &Ctx, queries: &[Query], n: usize, cfg: &ActivationConfig) -> serde_json::Value {
    let g = ctx.g;
    let wf = |c: EdgeCategoryId| ctx.push_w.get(&c).copied().unwrap_or(1.0);
    let mut push = PushPpr::new(&ctx.adj_push, &wf);
    let mut acc: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut max_diff = 0.0f64;
    let mut bit_identical = true;
    let mut taken = 0usize;
    let s0 = vec![0.0f32; ctx.n_pairs];
    for q in queries.iter().step_by((queries.len() / n.max(1)).max(1)).take(n) {
        taken += 1;
        let seed_nodes = &g.file_nodes[q.seed as usize];
        let seed_ids: Vec<NodeId> = seed_nodes.iter().map(|&u| g.ids[u as usize]).collect();
        let seed_ppr: Vec<usize> = seed_nodes.iter().map(|&u| g.node_ppr[u as usize]).collect();
        let pers = Ppr::uniform(&seed_ppr);

        let t = Instant::now();
        let _ = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
        *acc.entry("B0 (carry BFS + file depth)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let v = ctx.ppr[0].1.run(&pers);
        let fs = file_pool(g, |u| v[g.node_ppr[u]], false);
        *acc.entry("P1 (power iteration over a prebuilt index)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let view = ActivationPlan::<MergedGraph>::new(cfg.clone()).run(ctx.merged, &seed_ids);
        let prod: HashMap<NodeId, f64> = view.scores.into_iter().collect();
        let fs_prod = file_pool(g, |u| prod.get(&g.ids[u]).copied().unwrap_or(0.0), false);
        *acc.entry("P1 production (ActivationPlan::run, as answers call it)").or_default() += t.elapsed().as_secs_f64();
        for (u, id) in g.ids.iter().enumerate() {
            let a = v[g.node_ppr[u]];
            let b = prod.get(id).copied().unwrap_or(0.0);
            let b = if b > 0.0 { b } else { 0.0 };
            let a = if a > 0.0 { a } else { 0.0 };
            if a.to_bits() != b.to_bits() {
                bit_identical = false;
            }
            max_diff = max_diff.max((a - b).abs());
        }
        let _ = (fs, fs_prod);

        let t = Instant::now();
        let _ = push_scores(ctx, &mut push, &seed_ids);
        *acc.entry("P3 (push PPR, per seed, averaged)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let (inc, ow) = ctx.upr.reweight(&s0, 5.0);
        let v = ctx.upr.power(&pers, &inc, &ow);
        let _ = file_pool(g, |u| v[g.node_ppr[u]], false);
        *acc.entry("U1 (reweight edges + power iteration)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let k = pooled(&fs_or_zero(&v, g), &ctx.clusters[0].1, ctx.clusters[0].2);
        let _ = evaluate(&k, q.seed as usize, &q.targets);
        *acc.entry("K pooling given P1 + one metric evaluation").or_default() += t.elapsed().as_secs_f64();
    }
    let mut out = serde_json::Map::new();
    for (k, v) in acc {
        out.insert(k.into(), json!(1000.0 * v / taken.max(1) as f64));
    }
    out.insert("queries_timed".into(), json!(taken));
    out.insert("replica_vs_production_max_abs_diff".into(), json!(max_diff));
    out.insert("replica_vs_production_bit_identical".into(), json!(bit_identical));
    serde_json::Value::Object(out)
}

fn fs_or_zero(v: &[f64], g: &Graph) -> Vec<f64> {
    file_pool(g, |u| v[g.node_ppr[u]], false)
}

// ============================================================================
// Markdown
// ============================================================================

fn render_md(
    label: &str,
    doc: &serde_json::Value,
    names: &[String],
    metrics: &[[f64; 5]],
    paired_all: &BTreeMap<(String, String, usize), Paired>,
) -> String {
    let pos = |n: &str| names.iter().position(|x| x == n);
    let r10 = |n: &str| pos(n).map_or(f64::NEG_INFINITY, |i| metrics[i][1]);
    let structural = [
        "B0", "B0i", "B1", "B3", "P1", "P1sum", "P1f", "P1b", "P1repair", "P1review", "P1onboard", "P2", "P2i", "P3",
        "K1dir", "K1top2", "K1leiden", "K1svc", "K2dir", "K2top2", "K2leiden", "K2svc",
    ];
    let best_struct = structural.iter().copied().max_by(|a, b| r10(a).total_cmp(&r10(b))).unwrap();
    let best_k = structural.iter().copied().filter(|n| n.starts_with('K')).max_by(|a, b| r10(a).total_cmp(&r10(b))).unwrap();
    let mut rows: Vec<String> = [
        "B0", "B0i", "B1", "B2", "B3", "C0", "P1", "P1sum", "P1f", "P1b", "P1repair", "P1review", "P1onboard", "P2",
        "P2i", "P3", "K1dir", "K1top2", "K1leiden", "K1svc", "K2dir", "K2top2", "K2leiden", "K2svc", "H1a25",
        "H1a50", "H1a75", "H1r25", "H1r50", "H1r75", "H2a25", "H2a50", "H2a75", "H3", "H4",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    rows.push(format!("H5[{best_k}]"));
    rows.extend(["H10", "T0", "T1", "T2", "H6", "H7", "H8"].iter().map(|s| s.to_string()));
    for base in ["P1", "P1sum", "P2"] {
        for a in [25, 50, 75] {
            rows.push(format!("H9a{a}[{base}]"));
        }
    }
    let best_h9 = names
        .iter()
        .filter(|n| n.starts_with("H9a"))
        .max_by(|a, b| r10(a).total_cmp(&r10(b)))
        .cloned()
        .unwrap_or_default();
    if !rows.contains(&best_h9) {
        rows.push(best_h9.clone());
    }
    rows.extend(
        ["H18", "H11", "H12", "H13", "H14", "H15", "H16", "H17", "U1b1", "U1b5", "U2b1", "U2b5", "U2fb1", "U2fb5", "U3b5", "U3sb5"]
            .iter()
            .map(|s| s.to_string()),
    );
    let rows: Vec<String> = rows.into_iter().filter(|r| pos(r).is_some()).collect();
    let mut best = [f64::NEG_INFINITY; 5];
    for r in &rows {
        let m = metrics[pos(r).unwrap()];
        for i in 0..5 {
            best[i] = best[i].max(m[i]);
        }
    }
    let s = &doc["stats"];
    let mut md = String::new();
    md.push_str(&format!("### {label}\n\n"));
    md.push_str(&format!(
        "Graph: {} nodes, {} edges, {} candidate code files. History: {} non-merge commits, {} training-eligible (1..=30 code files), {} test-eligible (2..=30), {} test commits, {} queries (mean {:.2} targets, mean {:.1} seed nodes). Chance recall@10 = {:.4}. Queries with no co-change history for the seed: {}. Test commits whose message matched no past task: {}.\n\n",
        s["nodes"], s["edges"], s["candidate_files"], s["git_nonmerge_commits"], s["train_commits"],
        s["test_eligible_commits"], s["test_commits"], s["queries"], s["mean_targets"].as_f64().unwrap(),
        s["mean_seed_nodes"].as_f64().unwrap(), s["chance_recall10"].as_f64().unwrap(),
        s["queries_without_cochange_history"], s["test_commits_without_past_task_hit"]
    ));
    md.push_str(&format!(
        "Best structural ranker by recall@10: {best_struct}. Best K* (H5's base): {best_k}. Best H9 over every structural base (selected on this test set): {best_h9}.\n\n"
    ));
    md.push_str("| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n");
    for r in &rows {
        let m = metrics[pos(r).unwrap()];
        let cells: Vec<String> = (0..5)
            .map(|i| if (m[i] - best[i]).abs() < 1e-12 { format!("**{:.3}**", m[i]) } else { format!("{:.3}", m[i]) })
            .collect();
        md.push_str(&format!("| {r} | {} |\n", cells.join(" | ")));
    }
    for (mi, mname) in [(1usize, "recall@10"), (3, "MRR")] {
        md.push_str(&format!(
            "\nPaired {mname} differences (ranker minus baseline; mean, 95% commit-cluster bootstrap CI, sign-test p over queries, wins/losses):\n\n| ranker | vs B0 | vs P1 | vs C0 |\n|---|---|---|---|\n"
        ));
        let key_rows = [
            "B0", "B0i", "B1", "B3", "C0", "P1", "P1sum", "P1f", "P1b", "P1onboard", "P2", "P3", "K1svc", "K2svc",
            "H1a25", "H2a50", "H3", "H4", "H10", "T0", "T1", "T2", "H6", "H7", "H8", "H11", "H12", "H13", "H14", "H15",
            "H16", "H17", "U1b1", "U1b5", "U2b5", "U2fb5", "U3b5", "U3sb5",
        ];
        let mut prow: Vec<String> = key_rows.iter().map(|s| s.to_string()).collect();
        prow.push(format!("H5[{best_k}]"));
        for extra in ["H9a25[P1]", "H9a50[P1]", "H9a25[P2]", "H9a50[P2]", "H9a50[P1sum]", "H18"] {
            prow.push(extra.to_string());
        }
        if !prow.contains(&best_h9) {
            prow.push(best_h9.clone());
        }
        for r in prow.iter().filter(|r| pos(r).is_some()) {
            let cell = |bl: &str| {
                paired_all.get(&(r.clone(), bl.to_string(), mi)).map_or("-".to_string(), |p| {
                    if p.wins + p.losses == 0 {
                        "=".to_string()
                    } else {
                        format!("{:+.3} [{:+.3}, {:+.3}] p={:.2e} {}/{}", p.mean, p.lo, p.hi, p.p, p.wins, p.losses)
                    }
                })
            };
            md.push_str(&format!("| {r} | {} | {} | {} |\n", cell("B0"), cell("P1"), cell("C0")));
        }
    }
    md.push_str("\nInefficiency map (file pairs joined by an edge of the category; lift = P(pair co-changed in >= 1 commit) / P(random pair co-changed)):\n\n| category | weight | carry | pairs | co-changed | P(co-change) | lift | coverage of co-changed pairs |\n|---|---|---|---|---|---|---|---|\n");
    for row in doc["inefficiency"].as_array().unwrap() {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.3} | {:.1} | {:.3} |\n",
            row["category"].as_str().unwrap(),
            row["activation_weight"],
            row["carry"],
            row["pairs"],
            row["cochanged_pairs"],
            row["p_cochange"].as_f64().unwrap(),
            row["lift"].as_f64().unwrap(),
            row["coverage"].as_f64().unwrap()
        ));
    }
    md.push_str(&format!(
        "\nRandom-pair co-change rate: {:.4} ({} co-changed pairs). Hub flooding (mean share of the top-20 files' score mass on the 5 highest-degree files {}): {}\n",
        s["random_pair_cochange_rate"].as_f64().unwrap(),
        s["cochanged_pairs"],
        s["hub_files"],
        doc["hub_share_top20_mass_on_top5_degree_files"]
    ));
    md.push_str(&format!("\nTiming (ms per query, sequential): {}\n", doc["timing_ms_per_query"]));
    md
}

// ============================================================================
// Vertical layers (second run, 2026-10-18)
//
// The question: do files that change together hang under the same upper-layer
// node (a requirement, a spec, a law, a packet, a ruling) even when no code edge
// joins them? Every upper layer is read from the TEST COMMIT'S PARENT revision
// (code with `git grep <parent>`, docs from the same revision), so nothing the
// commit itself writes is visible. Kina's specs moved into its wiki (its own git
// repository) at c643306; after that the doc layer is the wiki commit current at
// the parent's committer time. The `w` variants read the wiki at its HEAD
// instead (a snapshot, later than every test commit: flagged).
// ============================================================================

#[derive(Clone, Copy, PartialEq, Debug)]
enum VMode {
    Kina,
    Glia,
    Quokka,
}

impl VMode {
    fn parse(s: &str) -> Option<VMode> {
        match s {
            "" => None,
            "kina" => Some(VMode::Kina),
            "glia" => Some(VMode::Glia),
            "quokka" => Some(VMode::Quokka),
            other => panic!("--vertical {other}: expected kina, glia or quokka"),
        }
    }
}

/// One set-valued upper-layer signal at one revision: candidate file -> the
/// upper-layer nodes it links to; score(f) = sum over nodes shared with the seed
/// of idf(node) = ln(1 + files / files linking the node).
#[derive(Clone, Default)]
struct Sig {
    sets: Vec<Vec<u32>>,
    post: Vec<Vec<u32>>,
    idf: Vec<f64>,
    names: Vec<String>,
}

impl Sig {
    fn build(keyed: &[BTreeSet<String>]) -> Sig {
        let nf = keyed.len();
        let mut all: BTreeSet<&String> = BTreeSet::new();
        for s in keyed {
            all.extend(s.iter());
        }
        let names: Vec<String> = all.into_iter().cloned().collect();
        let ix: HashMap<&str, u32> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i as u32)).collect();
        let sets: Vec<Vec<u32>> = keyed.iter().map(|s| s.iter().map(|k| ix[k.as_str()]).collect()).collect();
        Sig::from_sets(nf, sets, names)
    }

    fn from_sets(nf: usize, sets: Vec<Vec<u32>>, names: Vec<String>) -> Sig {
        let mut post = vec![Vec::new(); names.len()];
        for (f, s) in sets.iter().enumerate() {
            for &x in s {
                post[x as usize].push(f as u32);
            }
        }
        let idf = post.iter().map(|p| if p.is_empty() { 0.0 } else { (1.0 + nf as f64 / p.len() as f64).ln() }).collect();
        Sig { sets, post, idf, names }
    }

    fn score(&self, seed: usize) -> Vec<f64> {
        let mut s = vec![0.0; self.sets.len()];
        for &x in &self.sets[seed] {
            for &f in &self.post[x as usize] {
                s[f as usize] += self.idf[x as usize];
            }
        }
        s[seed] = 0.0;
        s
    }

    /// The control: each file keeps its NUMBER of upper-layer links, but they are
    /// drawn at random (deterministically) from the nodes in use at this revision,
    /// uniformly or in proportion to how many files link each node.
    fn shuffled(&self, key: &str, weighted: bool) -> Sig {
        let m = self.names.len();
        let w: Vec<f64> = (0..m).map(|x| if weighted { self.post[x].len() as f64 } else { 1.0 }).collect();
        let total: f64 = w.iter().sum();
        let mut cum = Vec::with_capacity(m);
        let mut acc = 0.0;
        for &x in &w {
            acc += x;
            cum.push(acc);
        }
        let sets: Vec<Vec<u32>> = self
            .sets
            .iter()
            .enumerate()
            .map(|(f, s)| {
                if s.is_empty() || m == 0 {
                    return Vec::new();
                }
                let want = s.len().min(m);
                let mut rng = Rng(h64(&format!("{key}:{f}")));
                let mut got: BTreeSet<u32> = BTreeSet::new();
                let mut tries = 0;
                while got.len() < want && tries < 1000 {
                    tries += 1;
                    let r = (rng.next() >> 11) as f64 / (1u64 << 53) as f64 * total;
                    let x = cum.partition_point(|&c| c <= r).min(m - 1);
                    got.insert(x as u32);
                }
                got.into_iter().collect()
            })
            .collect();
        Sig::from_sets(self.sets.len(), sets, self.names.clone())
    }
}

struct VLayer {
    /// The signals, in display order (V1, V2, ...).
    sigs: Vec<(String, Sig)>,
    /// Controls: V0 (uniform random ids, as specified), V0df[X] (popularity-preserving
    /// random nodes for each signal), V4perm (glia: the ruling -> packet map permuted).
    ctrls: Vec<(String, Sig)>,
    /// The commit message's own ids routed through the layer (seed-independent).
    vm: Option<Vec<f64>>,
    vm_cov: bool,
    info: BTreeMap<String, f64>,
    vocab: Vec<String>,
}

// ---- git ----

fn git_text(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git grep -n -I` at `rev`, as (path, line) in file order; `pat` None = every line.
fn git_grep(dir: &str, rev: &str, pat: Option<&str>, specs: &[&str]) -> Vec<(String, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(["grep", "-n", "-I", "--no-color"]);
    match pat {
        Some(p) => {
            cmd.args(["-E", "-e", p]);
        }
        None => {
            cmd.args(["-e", ""]);
        }
    }
    cmd.arg(rev);
    if !specs.is_empty() {
        cmd.arg("--");
        cmd.args(specs);
    }
    let out = cmd.output().expect("git grep");
    let text = String::from_utf8_lossy(&out.stdout);
    let pre = format!("{rev}:");
    let mut v = Vec::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix(&pre) else { continue };
        let Some((path, rest)) = rest.split_once(':') else { continue };
        let Some((_, txt)) = rest.split_once(':') else { continue };
        v.push((path.to_string(), txt.to_string()));
    }
    v
}

/// sha -> (first parent, committer time) for every commit reachable from `rev`.
fn git_parents(dir: &str, rev: &str) -> HashMap<String, (Option<String>, i64)> {
    let text = git_text(dir, &["log", "--format=%H%x1f%P%x1f%ct", rev]).expect("git log parents");
    text.lines()
        .filter_map(|l| {
            let mut p = l.split('\x1f');
            let sha = p.next()?.to_string();
            let parent = p.next()?.split_whitespace().next().map(str::to_string);
            let ct = p.next()?.trim().parse().ok()?;
            Some((sha, (parent, ct)))
        })
        .collect()
}

// ---- scanners (no regex crate in glia-engine's dependencies) ----

fn wb(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
}

fn we(b: &[u8], j: usize) -> bool {
    j >= b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'_')
}

fn digits_at(b: &[u8], k: usize, max: usize) -> usize {
    let mut e = k;
    while e < b.len() && e - k < max && b[e].is_ascii_digit() {
        e += 1;
    }
    e
}

/// Hyphenated ids `[A-Z][A-Z0-9]{1,6}-[0-9]{1,4}[a-z]?` as (prefix, id), with the
/// `P-01/02` and `P-01..06` shorthands expanded.
fn hyphen_ids(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !(b[i].is_ascii_uppercase() && wb(b, i) && (i == 0 || b[i - 1] != b'-')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && j - i < 8 && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
            j += 1;
        }
        if (2..=7).contains(&(j - i)) && j < n && b[j] == b'-' {
            let k0 = j + 1;
            let k = digits_at(b, k0, 4);
            if k > k0 {
                let mut e = k;
                if e < n && b[e].is_ascii_lowercase() && we(b, e + 1) {
                    e += 1;
                }
                if we(b, e) && !(e + 1 < n && b[e] == b'-' && b[e + 1].is_ascii_digit()) {
                    let prefix = &s[i..j];
                    out.push((prefix.to_string(), s[i..e].to_string()));
                    let width = k - k0;
                    let lo: u32 = s[k0..k].parse().unwrap_or(0);
                    let mut m = e;
                    loop {
                        if m + 1 < n && b[m] == b'/' && b[m + 1].is_ascii_digit() {
                            let d = digits_at(b, m + 1, 4);
                            if we(b, d) {
                                out.push((prefix.to_string(), format!("{prefix}-{}", &s[m + 1..d])));
                                m = d;
                                continue;
                            }
                        }
                        if m + 2 < n && b[m] == b'.' && b[m + 1] == b'.' && b[m + 2].is_ascii_digit() {
                            let d = digits_at(b, m + 2, 4);
                            if we(b, d) {
                                let hi: u32 = s[m + 2..d].parse().unwrap_or(0);
                                if hi > lo && hi - lo <= 30 {
                                    for x in lo + 1..=hi {
                                        out.push((prefix.to_string(), format!("{prefix}-{x:0width$}")));
                                    }
                                }
                                m = d;
                                continue;
                            }
                        }
                        break;
                    }
                    i = m.max(i + 1);
                    continue;
                }
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Spec-kit's hyphen-less ids: `T###` (tasks), `R#` / `C#` / `D#` (research,
/// analysis and decision rows), for the kinds in `kinds`.
fn bare_ids(s: &str, kinds: &BTreeSet<u8>) -> Vec<(u8, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !kinds.contains(&b[i]) || !wb(b, i) {
            continue;
        }
        let e = digits_at(b, i + 1, 4);
        let len = e - (i + 1);
        let ok = if b[i] == b'T' { len == 3 } else { (1..=2).contains(&len) };
        if !ok || !we(b, e) {
            continue;
        }
        if e + 1 < n && (b[e] == b'.' || b[e] == b'-') && b[e + 1].is_ascii_digit() {
            continue;
        }
        out.push((b[i], s[i..e].to_string()));
    }
    out
}

/// Packet ids `[A-Z][A-Z0-9]{0,4}.[0-9]{1,3}[a-z]{0,2}` as (prefix, id), with the
/// letter shorthands `LA.25a/b`, `LA.30a-c` and `DT.5h-DT.5l` expanded.
fn dot_ids(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !(b[i].is_ascii_uppercase() && wb(b, i) && (i == 0 || b[i - 1] != b'.')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && j - i < 6 && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
            j += 1;
        }
        if (1..=5).contains(&(j - i)) && j + 1 < n && b[j] == b'.' && b[j + 1].is_ascii_digit() {
            let k = digits_at(b, j + 1, 3);
            let mut e = k;
            while e < n && e - k < 2 && b[e].is_ascii_lowercase() {
                e += 1;
            }
            if we(b, e) && !(e + 1 < n && b[e] == b'.' && b[e + 1].is_ascii_digit()) {
                let prefix = s[i..j].to_string();
                let base = s[i..k].to_string();
                out.push((prefix.clone(), s[i..e].to_string()));
                let last = if e > k { b[e - 1] } else { 0 };
                let mut m = e;
                if last != 0 {
                    // a/b
                    while m + 1 < n && b[m] == b'/' && b[m + 1].is_ascii_lowercase() && we(b, m + 2) {
                        out.push((prefix.clone(), format!("{base}{}", b[m + 1] as char)));
                        m += 2;
                    }
                    // a-c / a–c / a-DT.5l
                    let (dash, dl) = if m < n && b[m] == b'-' {
                        (true, 1)
                    } else if s[m..].starts_with('\u{2013}') {
                        (true, 3)
                    } else {
                        (false, 0)
                    };
                    if dash {
                        let r = m + dl;
                        let to = if r < n && b[r].is_ascii_lowercase() && we(b, r + 1) {
                            Some(b[r])
                        } else if s[r..].starts_with(&base) {
                            let q = r + base.len();
                            (q < n && b[q].is_ascii_lowercase() && we(b, q + 1)).then(|| b[q])
                        } else {
                            None
                        };
                        if let Some(to) = to
                            && to > last
                            && to - last <= 20
                        {
                            for c in last + 1..=to {
                                out.push((prefix.clone(), format!("{base}{}", c as char)));
                            }
                        }
                    }
                }
                i = m.max(i + 1);
                continue;
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Kina spec numbers a line names: `spec 003`, `Spec 003`, `specs/003-..`,
/// `003 T014`, `003 FR-057`, `003-US2`.
fn spec_refs(s: &str) -> Vec<u32> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !b[i].is_ascii_digit() || !wb(b, i) {
            continue;
        }
        let e = digits_at(b, i, 4);
        if e - i != 3 || (e < n && b[e].is_ascii_digit()) {
            continue;
        }
        let before = s[..i].to_ascii_lowercase();
        let after = &s[e..];
        let pre = before.ends_with("spec ") || before.ends_with("specs/") || before.ends_with("spec-") || before.ends_with("specs ");
        let ab = after.as_bytes();
        let post = after.starts_with(" FR-")
            || after.starts_with(" SC-")
            || after.starts_with("-US")
            || after.starts_with(" US")
            || (ab.len() >= 5 && ab[0] == b' ' && ab[1] == b'T' && ab[2..5].iter().all(u8::is_ascii_digit))
            || (ab.len() >= 3 && ab[0] == b' ' && matches!(ab[1], b'R' | b'C' | b'D') && ab[2].is_ascii_digit());
        if pre || post {
            out.push(s[i..e].parse().unwrap_or(0));
        }
    }
    out
}

const LAW_HEADS: &[&str] = &["Act", "Acts", "Rules", "Regulations", "Regulation", "Directives", "Directive"];
const LAW_CONNECT: &[&str] = &["and", "of", "the", "for", "on", "to", "&", "in"];
const LAW_DROP: &[&str] = &["the", "au", "png", "us", "cth", "(cth)", "australia", "vanuatu"];
/// Sentence fillers a capitalised run can start with ("Nothing in this Act", "Reach of the AML/CTF Act").
const LAW_LEAD: &[&str] = &[
    "neither", "nothing", "none", "no", "add", "adds", "amend", "amends", "amending", "enacted", "assent", "principal",
    "statute", "explanatory", "commentary", "reach", "division", "part", "under", "see", "per", "with", "from", "by",
    "as", "at", "or", "if", "is", "every", "when", "where", "why", "how", "not", "both", "either", "only", "also",
    "s", "section", "schedule", "item", "items", "chapter", "reg", "rule", "subsection", "paragraph",
];
const LAW_GENERIC: &[&str] = &[
    "this", "that", "each", "any", "an", "a", "our", "its", "their", "these", "those", "which", "under", "see", "per",
    "new", "same", "other", "relevant", "both", "all",
];

/// Law keys a line names: a run of capitalised words, acronyms and connectors
/// ending in Act / Rules / Regulations / Directive(s). Normalised: lower-case,
/// alphanumerics only, jurisdiction words dropped, the AML/CTF spellings unified;
/// the year is not part of the key (so the AU 2006 and PNG 2015 AML/CTF Acts are
/// one key). LegalSources-style file names (`AU_AMLCTF_Act_2006_..pdf`) read the same.
fn law_keys(s: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for raw in s.split_whitespace() {
        let t = raw.trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == '*');
        let lower = t.to_ascii_lowercase();
        if lower.ends_with(".pdf") || lower.ends_with(".txt") || lower.ends_with(".pdf)") {
            let stem = &t[..t.to_ascii_lowercase().rfind('.').unwrap_or(t.len())];
            words.extend(stem.split(['_', '-', '/']).filter(|w| !w.is_empty()).map(str::to_string));
            words.push(",".into());
        } else {
            for w in t.split('_') {
                if !w.is_empty() {
                    words.push(w.to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..words.len() {
        let core = words[k].trim_matches(|c: char| !c.is_alphanumeric());
        if !LAW_HEADS.contains(&core) || !words[k].starts_with(core.chars().next().unwrap_or(' ')) && !words[k].starts_with('(') {
            continue;
        }
        let mut name: Vec<String> = Vec::new();
        let mut m = k;
        while m > 0 && name.len() < 12 {
            let prev = &words[m - 1];
            if prev.ends_with(',') || prev.ends_with(';') || prev.ends_with(':') || prev.ends_with('.') || prev == "," {
                break;
            }
            let c = prev.trim_matches(|c: char| c == '(' || c == ')' || c == '[' || c == ']');
            let ok = match c.chars().next() {
                Some(ch) if ch.is_uppercase() => true,
                _ => LAW_CONNECT.contains(&c),
            };
            if !ok || c.is_empty() {
                break;
            }
            name.push(c.to_string());
            m -= 1;
        }
        name.reverse();
        for w in name.iter_mut() {
            for suf in ["'s", "\u{2019}s"] {
                if let Some(x) = w.strip_suffix(suf) {
                    *w = x.to_string();
                }
            }
        }
        while name.first().is_some_and(|w| {
            let l = w.to_ascii_lowercase();
            LAW_CONNECT.contains(&l.as_str()) || LAW_LEAD.contains(&l.as_str()) || LAW_DROP.contains(&l.as_str())
        }) {
            name.remove(0);
        }
        let kept: Vec<&String> = name.iter().filter(|w| !LAW_DROP.contains(&w.to_ascii_lowercase().as_str())).collect();
        if kept.is_empty() {
            continue;
        }
        if kept.len() == 1 && LAW_GENERIC.contains(&kept[0].to_ascii_lowercase().as_str()) {
            continue;
        }
        let acronym = kept.iter().any(|w| w.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2);
        if core != "Act" && core != "Acts" && !acronym && kept.len() < 2 {
            continue;
        }
        let mut compact: String =
            kept.iter().flat_map(|w| w.chars()).filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        for pat in [
            "antimoneylaunderingandcounterterrorismfinancing",
            "antimoneylaunderingandcounterterroristfinancing",
            "antimoneylaunderingcounterterroristfinancing",
            "antimoneylaunderingcounterterrorismfinancing",
            "amlctf",
        ] {
            if compact.contains(pat) {
                compact = compact.replace(pat, "amlctf");
                break;
            }
        }
        compact = compact.replace("papuanewguinea", "");
        if compact.len() < 3 {
            continue;
        }
        let head = match core {
            "Acts" => "act".to_string(),
            "Regulation" => "regulations".to_string(),
            "Directive" => "directives".to_string(),
            h => h.to_ascii_lowercase(),
        };
        out.push(format!("{compact}_{head}"));
    }
    out
}

/// Candidate file paths resolved from path-like tokens in a line: an exact path,
/// or a path suffix (or a bare file name) that names exactly one candidate.
struct PathRes {
    exts: HashSet<String>,
    suffix: HashMap<String, Vec<u32>>,
}

impl PathRes {
    fn new(paths: &[String]) -> PathRes {
        let mut suffix: HashMap<String, Vec<u32>> = HashMap::new();
        let mut exts = HashSet::new();
        for (f, p) in paths.iter().enumerate() {
            exts.insert(ext_of(p));
            let segs: Vec<&str> = p.split('/').collect();
            for k in 0..segs.len() {
                suffix.entry(segs[k..].join("/")).or_default().push(f as u32);
            }
        }
        PathRes { exts, suffix }
    }

    fn files(&self, s: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for raw in s.split(|c: char| {
            c.is_whitespace() || matches!(c, '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '|' | '*' | '{' | '}' | '=')
        }) {
            let mut t = raw.trim_end_matches(['.', ':', '!', '?']);
            t = t.trim_start_matches("./").trim_start_matches('/');
            if let Some((a, _)) = t.split_once(':') {
                t = a;
            }
            if t.len() < 4 || !t.contains('.') {
                continue;
            }
            if !self.exts.contains(&ext_of(t)) {
                continue;
            }
            if let Some(v) = self.suffix.get(t)
                && v.len() == 1
            {
                out.push(v[0]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The ids a markdown line DEFINES: the first token of a heading, of a table
/// row's first cell, or of a (checkbox) list item, possibly bold or quoted.
fn line_head(line: &str) -> &str {
    let t = line.trim_start();
    let t = if let Some(h) = t.strip_prefix('#') {
        h.trim_start_matches('#').trim_start()
    } else if let Some(r) = t.strip_prefix('|') {
        r.split('|').next().unwrap_or("").trim()
    } else {
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
            .unwrap_or_else(|| {
                let d = t.bytes().take_while(u8::is_ascii_digit).count();
                if d > 0 && t[d..].starts_with(". ") { &t[d + 2..] } else { t }
            });
        let t = t.trim_start();
        t.strip_prefix("[ ] ").or_else(|| t.strip_prefix("[x] ")).or_else(|| t.strip_prefix("[X] ")).unwrap_or(t)
    };
    t.trim_start_matches(['*', '`', '_', '~', ' '])
}

// ---- Kina ----

const KINA_STANDARDS: &[&str] = &[
    "SHA", "AES", "ERC", "UTF", "ISO", "RFC", "EIP", "BIP", "CVE", "MIT", "BSD", "GPL", "LGPL", "AGPL", "MPL", "CC0",
];
const KINA_BARE: &[u8] = b"TRCD";
const KINA_CODE_PAT: &str = "[A-Z][A-Z0-9]{1,6}-[0-9]|[TRCD][0-9]|[Ss][Pp][Ee][Cc]|[0-9]{3}[ -]|Act|Rules|Regulation|Directive";

#[derive(Default)]
struct DocSet {
    units: Vec<String>,
    is_spec: Vec<bool>,
    spec_unit: BTreeMap<u32, u32>,
    defs: BTreeMap<String, BTreeSet<u32>>,
    laws: Vec<BTreeSet<String>>,
    law_vocab: BTreeSet<String>,
    /// (unit, ids the line defines, files the line names, the line is an unchecked `- [ ]` task)
    mentions: Vec<(u32, Vec<String>, Vec<u32>, bool)>,
    vocab_hyphen: BTreeMap<String, usize>,
    vocab_bare: BTreeMap<u8, usize>,
}

fn kina_unit(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() >= 3 && segs[0] == "specs" { format!("specs/{}", segs[1]) } else { path.to_string() }
}

fn build_docset(lines: &[(String, String)], pr: &PathRes) -> DocSet {
    let mut d = DocSet::default();
    let mut uix: HashMap<String, u32> = HashMap::new();
    let bare: BTreeSet<u8> = KINA_BARE.iter().copied().collect();
    for (path, line) in lines {
        let un = kina_unit(path);
        let u = *uix.entry(un.clone()).or_insert_with(|| {
            d.units.push(un.clone());
            let spec = un.starts_with("specs/");
            d.is_spec.push(spec);
            d.laws.push(BTreeSet::new());
            if spec {
                let num: String = un["specs/".len()..].chars().take_while(char::is_ascii_digit).collect();
                if num.len() == 3 {
                    d.spec_unit.insert(num.parse().unwrap_or(0), (d.units.len() - 1) as u32);
                }
            }
            (d.units.len() - 1) as u32
        });
        // definitions
        let head = line_head(line);
        let mut defined: Vec<String> = Vec::new();
        if let Some((p, id)) = hyphen_ids(head).into_iter().next()
            && head.starts_with(&id)
        {
            if !KINA_STANDARDS.contains(&p.as_str()) {
                *d.vocab_hyphen.entry(p).or_default() += 1;
                defined.push(id);
            }
            // `| RGM-01/02 |`: the shorthand siblings are defined too
            for (p2, id2) in hyphen_ids(head).into_iter().skip(1) {
                if head[..head.find([' ', '*', '|', ':']).unwrap_or(head.len())].contains(&id2[p2.len() + 1..]) {
                    defined.push(id2);
                }
            }
        } else if let Some((k, id)) = bare_ids(head, &bare).into_iter().next()
            && head.starts_with(&id)
        {
            *d.vocab_bare.entry(k).or_default() += 1;
            defined.push(id);
        }
        for id in &defined {
            d.defs.entry(id.clone()).or_default().insert(u);
        }
        // laws
        for k in law_keys(line) {
            d.laws[u as usize].insert(k);
        }
        // files named
        let files = pr.files(line);
        if !files.is_empty() {
            let t = line.trim_start();
            let open = t.starts_with("- [ ]") || t.starts_with("* [ ]");
            d.mentions.push((u, defined, files, open));
        }
    }
    // A law key counts only when at least two doc units name it (drops sentence
    // fragments a single page happens to capitalise).
    let mut units_per: BTreeMap<String, usize> = BTreeMap::new();
    for ls in &d.laws {
        for k in ls {
            *units_per.entry(k.clone()).or_default() += 1;
        }
    }
    d.law_vocab = units_per.into_iter().filter(|(_, n)| *n >= 2).map(|(k, _)| k).collect();
    let vocab = d.law_vocab.clone();
    for ls in d.laws.iter_mut() {
        ls.retain(|k| vocab.contains(k));
    }
    d
}

/// One cited id resolved to its upper-layer token and the units that define it.
/// Spec-kit ids (FR-002, T014, ...) are numbered per spec, so a spec-scoped id is
/// qualified `specs/<dir>|<id>` by a spec number on the same line, its only
/// defining spec, or the one spec the file names; global ids (KYC-01, BE-13, ...)
/// keep their bare form.
fn kina_resolve(
    d: &DocSet,
    id: &str,
    is_bare: bool,
    line_specs: &[u32],
    file_specs: &[u32],
    stats: &mut BTreeMap<String, f64>,
) -> Option<(String, Vec<u32>)> {
    let units = |nums: &[u32]| -> Vec<u32> {
        let mut v: Vec<u32> = nums.iter().filter_map(|n| d.spec_unit.get(n).copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let tok = |u: u32| format!("{}|{id}", d.units[u as usize]);
    let mut bump = |k: &str| *stats.entry(format!("resolve_{k}")).or_default() += 1.0;
    let defs: Vec<u32> = d.defs.get(id).map(|s| s.iter().copied().collect()).unwrap_or_default();
    let lu = units(line_specs);
    if defs.is_empty() {
        if lu.len() == 1 {
            bump("undefined_line_spec");
            return Some((tok(lu[0]), lu));
        }
        if is_bare {
            bump("dropped_bare_undefined");
            return None;
        }
        bump("undefined_global");
        return Some((id.to_string(), Vec::new()));
    }
    let spec_defs: Vec<u32> = defs.iter().copied().filter(|&u| d.is_spec[u as usize]).collect();
    let glob_defs: Vec<u32> = defs.iter().copied().filter(|&u| !d.is_spec[u as usize]).collect();
    if spec_defs.is_empty() {
        bump("global");
        return Some((id.to_string(), defs));
    }
    let pick = |c: &[u32]| -> Vec<u32> { c.iter().copied().filter(|u| spec_defs.contains(u)).collect() };
    let q = pick(&lu);
    if q.len() == 1 {
        bump("line_spec");
        return Some((tok(q[0]), q));
    }
    // An id a global doc also defines (Phase B's D6 vs spec 002's D6) is scoped to
    // a spec only by a qualifier on its own line.
    if !glob_defs.is_empty() {
        bump("global_over_spec");
        return Some((id.to_string(), glob_defs));
    }
    if spec_defs.len() == 1 {
        bump("unique_spec");
        return Some((tok(spec_defs[0]), spec_defs));
    }
    let q = pick(&units(file_specs));
    if q.len() == 1 {
        bump("file_spec");
        return Some((tok(q[0]), q));
    }
    if is_bare {
        bump("dropped_bare_ambiguous");
        return None;
    }
    bump("ambiguous");
    Some((format!("*|{id}"), spec_defs))
}

/// Per candidate file: (tokens, units, direct law keys), from the code lines.
type Cites = (Vec<BTreeSet<String>>, Vec<BTreeSet<u32>>, Vec<BTreeSet<String>>);

fn kina_cites(d: &DocSet, code: &BTreeMap<u32, Vec<String>>, nf: usize, stats: &mut BTreeMap<String, f64>) -> Cites {
    let kinds: BTreeSet<u8> = d.vocab_bare.keys().copied().collect();
    let mut toks = vec![BTreeSet::new(); nf];
    let mut units = vec![BTreeSet::new(); nf];
    let mut laws = vec![BTreeSet::new(); nf];
    for (&f, lines) in code {
        let file_specs: Vec<u32> = lines.iter().flat_map(|l| spec_refs(l)).collect();
        for l in lines {
            let ls = spec_refs(l);
            for n in &ls {
                if let Some(&u) = d.spec_unit.get(n) {
                    units[f as usize].insert(u);
                }
            }
            for (p, id) in hyphen_ids(l) {
                if !d.vocab_hyphen.contains_key(&p) {
                    continue;
                }
                if let Some((t, us)) = kina_resolve(d, &id, false, &ls, &file_specs, stats) {
                    toks[f as usize].insert(t);
                    units[f as usize].extend(us);
                }
            }
            for (_, id) in bare_ids(l, &kinds) {
                if let Some((t, us)) = kina_resolve(d, &id, true, &ls, &file_specs, stats) {
                    toks[f as usize].insert(t);
                    units[f as usize].extend(us);
                }
            }
            for k in law_keys(l) {
                if d.law_vocab.contains(&k) {
                    laws[f as usize].insert(k);
                }
            }
        }
    }
    (toks, units, laws)
}

struct KinaSignals {
    i: Vec<BTreeSet<String>>,
    s: Vec<BTreeSet<String>>,
    l: Vec<BTreeSet<String>>,
    ld: Vec<BTreeSet<String>>,
    m: Vec<BTreeSet<String>>,
    m_open: Vec<BTreeSet<String>>,
    m_rest: Vec<BTreeSet<String>>,
}

fn kina_signals(d: &DocSet, code: &BTreeMap<u32, Vec<String>>, nf: usize, stats: &mut BTreeMap<String, f64>) -> KinaSignals {
    let (i, units, direct) = kina_cites(d, code, nf, stats);
    let s: Vec<BTreeSet<String>> = units.iter().map(|us| us.iter().map(|&u| d.units[u as usize].clone()).collect()).collect();
    let l: Vec<BTreeSet<String>> =
        units.iter().map(|us| us.iter().flat_map(|&u| d.laws[u as usize].iter().cloned()).collect()).collect();
    let ld: Vec<BTreeSet<String>> = l.iter().zip(&direct).map(|(a, b)| a.union(b).cloned().collect()).collect();
    let mut m = vec![BTreeSet::new(); nf];
    let mut m_open = vec![BTreeSet::new(); nf];
    let mut m_rest = vec![BTreeSet::new(); nf];
    for (u, ids, files, open) in &d.mentions {
        let un = &d.units[*u as usize];
        let toks: Vec<String> = if ids.is_empty() {
            vec![un.clone()]
        } else {
            ids.iter().map(|id| if d.is_spec[*u as usize] { format!("{un}|{id}") } else { id.clone() }).collect()
        };
        for &f in files {
            m[f as usize].extend(toks.iter().cloned());
            if *open {
                m_open[f as usize].extend(toks.iter().cloned());
            } else {
                m_rest[f as usize].extend(toks.iter().cloned());
            }
        }
    }
    KinaSignals { i, s, l, ld, m, m_open, m_rest }
}

fn group_code(lines: Vec<(String, String)>, pidx: &HashMap<&str, u32>) -> BTreeMap<u32, Vec<String>> {
    let mut code: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (p, l) in lines {
        if let Some(&f) = pidx.get(p.as_str()) {
            code.entry(f).or_default().push(l);
        }
    }
    code
}

fn kina_doc_lines(git: &str, rev: &str) -> Vec<(String, String)> {
    git_grep(git, rev, None, &["*.md", ":!.specify", ":!.claude", ":!**/node_modules/**"])
}

// ---- glia ----

const GLIA_PACKETS: &[&str] = &[
    "dev-notes/wave-packets.json",
    "dev-notes/leap-packets.json",
    "dev-notes/leap-051-packets.json",
    "dev-notes/leap-052-packets.json",
];
const GLIA_SCHEDULES: &[&str] = &[
    "dev-notes/wave-runner/schedule.py",
    "dev-notes/wave-runner/leap_schedule.py",
    "dev-notes/wave-runner/schedule_051.py",
    "dev-notes/wave-runner/schedule_052.py",
];
const GLIA_PLANS: &[&str] =
    &["dev-notes/next-leap-0.5.0.md", "dev-notes/next-leap-0.5.1.md", "dev-notes/next-leap-0.5.2.md", "dev-notes/tweaks-0.5.2.md"];
const GLIA_CORRECTIONS: &[&str] = &[
    "dev-notes/packet-corrections.json",
    "dev-notes/leap-corrections.json",
    "dev-notes/leap-051-corrections.json",
    "dev-notes/leap-052-corrections.json",
];

#[derive(Clone)]
struct Packet {
    id: String,
    batch: String,
    files: Vec<String>,
    /// Q-numbers (next-leap-0.5.2.md §0.1) the packet's text cites; 0.5.2 packets only.
    qs: Vec<u32>,
}

struct BlobCache {
    text: std::sync::Mutex<HashMap<String, std::sync::Arc<String>>>,
    packets: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<Packet>>>>,
}

impl BlobCache {
    fn get(&self, git: &str, blob: &str) -> std::sync::Arc<String> {
        if let Some(t) = self.text.lock().unwrap().get(blob) {
            return t.clone();
        }
        let t = std::sync::Arc::new(git_text(git, &["cat-file", "-p", blob]).unwrap_or_default());
        self.text.lock().unwrap().insert(blob.to_string(), t.clone());
        t
    }

    fn packets(&self, git: &str, blob: &str, with_q: bool) -> std::sync::Arc<Vec<Packet>> {
        if let Some(p) = self.packets.lock().unwrap().get(blob) {
            return p.clone();
        }
        let text = self.get(git, blob);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        let mut out = Vec::new();
        if let Some(ps) = v.get("packets").and_then(|p| p.as_array()) {
            for p in ps {
                let Some(id) = p.get("id").and_then(|x| x.as_str()) else { continue };
                let batch = p.get("batch").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let files = p
                    .get("files_touched")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let mut qs = Vec::new();
                if with_q {
                    let t = p.to_string();
                    let b = t.as_bytes();
                    for i in 0..b.len() {
                        if b[i] == b'Q' && wb(b, i) {
                            let e = digits_at(b, i + 1, 2);
                            if e > i + 1 && we(b, e) {
                                let q: u32 = t[i + 1..e].parse().unwrap_or(0);
                                if (1..=25).contains(&q) {
                                    qs.push(q);
                                }
                            }
                        }
                    }
                    qs.sort_unstable();
                    qs.dedup();
                }
                out.push(Packet { id: id.to_string(), batch, files, qs });
            }
        }
        let a = std::sync::Arc::new(out);
        self.packets.lock().unwrap().insert(blob.to_string(), a.clone());
        a
    }
}

fn ls_tree(git: &str, rev: &str, paths: &[&str]) -> BTreeMap<String, String> {
    let mut args = vec!["ls-tree", rev, "--"];
    args.extend_from_slice(paths);
    let text = git_text(git, &args).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (meta, path) = l.split_once('\t')?;
            let blob = meta.split_whitespace().nth(2)?;
            Some((path.to_string(), blob.to_string()))
        })
        .collect()
}

fn family(id: &str) -> String {
    id.trim_end_matches(|c: char| c.is_ascii_lowercase()).to_string()
}

struct GliaDocs {
    packets: BTreeMap<String, Packet>,
    prefixes: BTreeSet<String>,
    wave: BTreeMap<String, String>,
    /// ruling -> packets
    rulings: BTreeMap<String, BTreeSet<String>>,
}

fn landed_waves(name: &str, text: &str, into: &mut BTreeMap<String, String>) {
    let mut on = false;
    for l in text.lines() {
        if l.starts_with("LANDED = {") {
            on = true;
            continue;
        }
        if on {
            if l.trim_start().starts_with('}') {
                break;
            }
            let Some((n, rest)) = l.trim().split_once(':') else { continue };
            let Ok(n) = n.trim().parse::<u32>() else { continue };
            let ids = rest.trim().trim_end_matches(',').trim_matches('"');
            for id in ids.split_whitespace() {
                into.entry(id.to_string()).or_insert_with(|| format!("{name}:W{n}"));
            }
        }
    }
}

/// Ruling items in a plan doc: inside any section whose heading says "decision"
/// or "ruling", each numbered item and each table row is one ruling; the packets
/// it names (ids, `P.n` families written `P-n`, and, in tweaks-0.5.2.md, `T<n>`
/// for the DT.<n> family and the T1-T13 gap rows) are its packets.
fn plan_rulings(
    doc: &str,
    text: &str,
    prefixes: &BTreeSet<String>,
    fams: &BTreeMap<String, BTreeSet<String>>,
    out: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let tweaks = doc.ends_with("tweaks-0.5.2.md");
    let mut heads: Vec<(usize, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let close = |cur: &mut Option<(String, String)>, out: &mut BTreeMap<String, BTreeSet<String>>| {
        let Some((key, t)) = cur.take() else { return };
        let mut ps: BTreeSet<String> = BTreeSet::new();
        for (p, id) in dot_ids(&t) {
            if prefixes.contains(&p) {
                ps.insert(id);
            }
        }
        for (p, id) in hyphen_ids(&t) {
            if prefixes.contains(&p) {
                let fam = format!("{p}.{}", &id[p.len() + 1..]);
                ps.extend(fams.get(&fam).into_iter().flatten().cloned());
            }
        }
        if tweaks {
            let b = t.as_bytes();
            for i in 0..b.len() {
                if b[i] == b'T' && wb(b, i) {
                    let e = digits_at(b, i + 1, 2);
                    if e > i + 1 && we(b, e) {
                        ps.extend(fams.get(&format!("DT.{}", &t[i + 1..e])).into_iter().flatten().cloned());
                    }
                }
            }
        }
        if !ps.is_empty() {
            out.entry(format!("{doc}#{key}")).or_default().extend(ps);
        }
    };
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            close(&mut cur, out);
            let level = t.bytes().take_while(|&c| c == b'#').count();
            heads.retain(|(l, _)| *l < level);
            heads.push((level, t.trim_start_matches('#').trim().to_string()));
            continue;
        }
        let in_rulings = heads.iter().any(|(_, h)| {
            let h = h.to_ascii_lowercase();
            h.contains("decision") || h.contains("ruling")
        });
        let section = heads.last().map_or(String::new(), |(_, h)| h.chars().take(40).collect());
        let gap_row = tweaks && t.starts_with("| T") && t[3..].starts_with(|c: char| c.is_ascii_digit());
        let gap_para = tweaks && t.starts_with('T') && t[1..].starts_with(|c: char| c.is_ascii_digit()) && t.contains(" (James");
        if gap_row || gap_para {
            close(&mut cur, out);
            let n: String = t.trim_start_matches(['|', ' ', 'T']).chars().take_while(char::is_ascii_digit).collect();
            cur = Some((format!("T{n}"), t.to_string()));
            close(&mut cur, out);
            continue;
        }
        if !in_rulings {
            close(&mut cur, out);
            continue;
        }
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && t[d..].starts_with(". ") {
            close(&mut cur, out);
            cur = Some((format!("{section}/{}", &t[..d]), t.to_string()));
        } else if t.starts_with('|') {
            close(&mut cur, out);
            let cell = t.trim_start_matches('|').split('|').next().unwrap_or("").trim().to_string();
            if !cell.is_empty() && !cell.starts_with("---") && cell != "#" && cell != "decision" {
                cur = Some((format!("{section}/{cell}"), t.to_string()));
                close(&mut cur, out);
            }
        } else if t.is_empty() {
            close(&mut cur, out);
        } else if let Some((_, buf)) = cur.as_mut() {
            buf.push(' ');
            buf.push_str(t);
        }
    }
    close(&mut cur, out);
}

fn glia_docs(git: &str, rev: &str, cache: &BlobCache) -> GliaDocs {
    let mut all: Vec<&str> = Vec::new();
    for g in [GLIA_PACKETS, GLIA_SCHEDULES, GLIA_PLANS, GLIA_CORRECTIONS] {
        all.extend_from_slice(g);
    }
    let blobs = ls_tree(git, rev, &all);
    let mut packets: BTreeMap<String, Packet> = BTreeMap::new();
    for &p in GLIA_PACKETS {
        if let Some(b) = blobs.get(p) {
            for pk in cache.packets(git, b, p.ends_with("leap-052-packets.json")).iter() {
                packets.entry(pk.id.clone()).or_insert_with(|| pk.clone());
            }
        }
    }
    let prefixes: BTreeSet<String> = packets.keys().map(|id| id.split('.').next().unwrap_or("").to_string()).collect();
    let mut fams: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in packets.keys() {
        fams.entry(family(id)).or_default().insert(id.clone());
    }
    let mut wave = BTreeMap::new();
    for &s in GLIA_SCHEDULES {
        if let Some(b) = blobs.get(s) {
            let name = s.rsplit('/').next().unwrap_or(s).trim_end_matches(".py");
            landed_waves(name, &cache.get(git, b), &mut wave);
        }
    }
    let mut rulings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for &d in GLIA_PLANS {
        if let Some(b) = blobs.get(d) {
            let name = d.rsplit('/').next().unwrap_or(d);
            plan_rulings(name, &cache.get(git, b), &prefixes, &fams, &mut rulings);
        }
    }
    for pk in packets.values() {
        for q in &pk.qs {
            rulings.entry(format!("next-leap-0.5.2.md#Q{q}")).or_default().insert(pk.id.clone());
        }
    }
    for &c in GLIA_CORRECTIONS {
        let Some(b) = blobs.get(c) else { continue };
        let v: serde_json::Value = serde_json::from_str(&cache.get(git, b)).unwrap_or(serde_json::Value::Null);
        let Some(obj) = v.get("corrections").and_then(|x| x.as_object()) else { continue };
        let name = c.rsplit('/').next().unwrap_or(c);
        for (k, val) in obj {
            let text = val.as_str().map_or_else(|| val.to_string(), str::to_string);
            if !text.contains("James") {
                continue;
            }
            let mut ps: BTreeSet<String> = BTreeSet::new();
            if packets.contains_key(k) {
                ps.insert(k.clone());
            }
            for (p, id) in dot_ids(&text) {
                if prefixes.contains(&p) {
                    ps.insert(id);
                }
            }
            if !ps.is_empty() {
                rulings.entry(format!("{name}#{k}")).or_default().extend(ps);
            }
        }
    }
    GliaDocs { packets, prefixes, wave, rulings }
}

// ---- layer assembly ----

/// (path, line) of every doc line at one revision, shared between test commits.
type DocLines = std::sync::Arc<Vec<(String, String)>>;

struct VEnv<'a> {
    mode: VMode,
    git: &'a str,
    wiki: &'a str,
    paths: &'a [String],
    pidx: &'a HashMap<&'a str, u32>,
    pr: PathRes,
    parents: HashMap<String, (Option<String>, i64)>,
    /// wiki commits (committer time, sha), oldest first
    wiki_revs: Vec<(i64, String)>,
    wiki_head_docs: Vec<(String, String)>,
    wiki_lines: std::sync::Mutex<HashMap<String, DocLines>>,
    cache: BlobCache,
}

fn controls(name: &str, sig: &Sig, sha: &str, ctrls: &mut Vec<(String, Sig)>) {
    ctrls.push((format!("V0df[{name}]"), sig.shuffled(&format!("{sha}:{name}:df"), true)));
}

fn msg_route(r: &Sig, msg_toks: &BTreeSet<String>, nf: usize) -> (Vec<f64>, bool) {
    let mut v = vec![0.0; nf];
    let mut cov = false;
    for (x, name) in r.names.iter().enumerate() {
        if msg_toks.contains(name) {
            cov = true;
            for &f in &r.post[x] {
                v[f as usize] += r.idf[x];
            }
        }
    }
    (v, cov)
}

fn build_layer(env: &VEnv, sha: &str, msg: &str) -> VLayer {
    let nf = env.paths.len();
    let mut info: BTreeMap<String, f64> = BTreeMap::new();
    let parent = env.parents.get(sha).and_then(|p| p.0.clone());
    let empty = vec![BTreeSet::<String>::new(); nf];
    let Some(p) = parent else {
        info.insert("no_parent".into(), 1.0);
        let sigs: Vec<(String, Sig)> = vertical_names(env.mode).iter().map(|n| (n.to_string(), Sig::build(&empty))).collect();
        let mut ctrls = Vec::new();
        if env.mode != VMode::Quokka {
            ctrls.push(("V0".to_string(), Sig::build(&empty)));
        }
        for (n, s) in &sigs {
            controls(n, s, sha, &mut ctrls);
        }
        if env.mode == VMode::Glia {
            for n in ["V4perm", "V4pperm", "V14perm"] {
                ctrls.push((n.into(), Sig::build(&empty)));
            }
        }
        let vm = (env.mode != VMode::Quokka).then(|| vec![0.0; nf]);
        return VLayer { sigs, ctrls, vm, vm_cov: false, info, vocab: Vec::new() };
    };
    match env.mode {
        VMode::Kina => {
            let pct = env.parents.get(&p).map_or(0, |x| x.1);
            let mut docs = kina_doc_lines(env.git, &p);
            let wrev = env.wiki_revs.iter().rev().find(|(ct, _)| *ct <= pct).map(|(_, s)| s.clone());
            if let Some(w) = &wrev {
                let cached = env.wiki_lines.lock().unwrap().get(w).cloned();
                let lines = cached.unwrap_or_else(|| {
                    let l = std::sync::Arc::new(git_grep(env.wiki, w, None, &["*.md"]));
                    env.wiki_lines.lock().unwrap().insert(w.clone(), l.clone());
                    l
                });
                docs.extend(lines.iter().cloned());
                info.insert("wiki_aligned".into(), 1.0);
            }
            let d = build_docset(&docs, &env.pr);
            let code = group_code(git_grep(env.git, &p, Some(KINA_CODE_PAT), &[]), env.pidx);
            let ks = kina_signals(&d, &code, nf, &mut info);
            let dw = build_docset(&env.wiki_head_docs, &env.pr);
            let mut junk = BTreeMap::new();
            let kw = kina_signals(&dw, &code, nf, &mut junk);
            let r: Vec<BTreeSet<String>> = ks.i.iter().zip(&ks.m).map(|(a, b)| a.union(b).cloned().collect()).collect();
            let sigs = vec![
                ("V1".to_string(), Sig::build(&ks.i)),
                ("V2".to_string(), Sig::build(&ks.s)),
                ("V3".to_string(), Sig::build(&ks.l)),
                ("V3d".to_string(), Sig::build(&ks.ld)),
                ("V2m".to_string(), Sig::build(&ks.m)),
                ("V1m".to_string(), Sig::build(&r)),
                ("V2w".to_string(), Sig::build(&kw.s)),
                ("V3w".to_string(), Sig::build(&kw.l)),
                ("V2mw".to_string(), Sig::build(&kw.m)),
                ("V2mP".to_string(), Sig::build(&ks.m_open)),
                ("V2mD".to_string(), Sig::build(&ks.m_rest)),
            ];
            let mut ctrls = vec![("V0".to_string(), sigs[0].1.shuffled(&format!("{sha}:V1:uniform"), false))];
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            // the message's own ids
            let mut msg_toks = BTreeSet::new();
            let kinds: BTreeSet<u8> = d.vocab_bare.keys().copied().collect();
            let file_specs: Vec<u32> = msg.lines().flat_map(spec_refs).collect();
            let mut ms = BTreeMap::new();
            for l in msg.lines() {
                let ls = spec_refs(l);
                for (pfx, id) in hyphen_ids(l) {
                    if d.vocab_hyphen.contains_key(&pfx)
                        && let Some((t, _)) = kina_resolve(&d, &id, false, &ls, &file_specs, &mut ms)
                    {
                        msg_toks.insert(t);
                    }
                }
                for (_, id) in bare_ids(l, &kinds) {
                    if let Some((t, _)) = kina_resolve(&d, &id, true, &ls, &file_specs, &mut ms) {
                        msg_toks.insert(t);
                    }
                }
            }
            let (vm, vm_cov) = msg_route(&sigs[5].1, &msg_toks, nf);
            info.insert("code_files_citing".into(), ks.i.iter().filter(|s| !s.is_empty()).count() as f64);
            info.insert("doc_units".into(), d.units.len() as f64);
            info.insert("spec_units".into(), d.spec_unit.len() as f64);
            info.insert("law_keys".into(), d.law_vocab.len() as f64);
            info.insert("defined_ids".into(), d.defs.len() as f64);
            let mut vocab: Vec<String> = d.vocab_hyphen.iter().map(|(k, v)| format!("{k}-* ({v} defs)")).collect();
            vocab.extend(d.vocab_bare.iter().map(|(k, v)| format!("{}### ({v} defs)", *k as char)));
            vocab.push(format!("laws: {}", d.law_vocab.iter().cloned().collect::<Vec<_>>().join(", ")));
            VLayer { sigs, ctrls, vm: Some(vm), vm_cov, info, vocab }
        }
        VMode::Glia => {
            let gd = glia_docs(env.git, &p, &env.cache);
            let code = group_code(git_grep(env.git, &p, Some("[A-Z][A-Z0-9]{0,4}[.][0-9]"), &[]), env.pidx);
            let mut i = vec![BTreeSet::new(); nf];
            for (&f, lines) in &code {
                for l in lines {
                    for (pfx, id) in dot_ids(l) {
                        if gd.prefixes.contains(&pfx) {
                            i[f as usize].insert(id);
                        }
                    }
                }
            }
            let fam: Vec<BTreeSet<String>> = i.iter().map(|s| s.iter().map(|x| family(x)).collect()).collect();
            let bat: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| {
                    s.iter()
                        .map(|x| {
                            gd.packets.get(x).filter(|p| !p.batch.is_empty()).map_or_else(
                                || x.split('.').next().unwrap_or("").to_string(),
                                |p| p.batch.clone(),
                            )
                        })
                        .collect()
                })
                .collect();
            let wav: Vec<BTreeSet<String>> = i.iter().map(|s| s.iter().filter_map(|x| gd.wave.get(x).cloned()).collect()).collect();
            let mut m = vec![BTreeSet::new(); nf];
            // split by the schedule at the parent: a packet with no LANDED wave yet is pending
            let mut m_pend = vec![BTreeSet::new(); nf];
            let mut m_done = vec![BTreeSet::new(); nf];
            for pk in gd.packets.values() {
                let pending = !gd.wave.contains_key(&pk.id);
                for entry in &pk.files {
                    for f in env.pr.files(entry) {
                        m[f as usize].insert(pk.id.clone());
                        if pending {
                            m_pend[f as usize].insert(pk.id.clone());
                        } else {
                            m_done[f as usize].insert(pk.id.clone());
                        }
                    }
                }
            }
            let r: Vec<BTreeSet<String>> = i.iter().zip(&m).map(|(a, b)| a.union(b).cloned().collect()).collect();
            let mut by_packet: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for (ru, ps) in &gd.rulings {
                for pk in ps {
                    by_packet.entry(pk.as_str()).or_default().insert(ru.as_str());
                }
            }
            let rul: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| s.iter().flat_map(|x| by_packet.get(x.as_str()).into_iter().flatten().map(|r| r.to_string())).collect())
                .collect();
            // V4p: the plan docs' rulings only (decision sections, Q-citations, the T1-T13 rows), no corrections.
            let plan_only = |set: &BTreeSet<String>| -> BTreeSet<String> {
                set.iter().filter(|r| !r.contains("corrections.json#")).cloned().collect()
            };
            let rul_p: Vec<BTreeSet<String>> = rul.iter().map(plan_only).collect();
            // control: the ruling -> packet map under a random permutation of the defined packets
            let defined: Vec<&str> = gd.packets.keys().map(String::as_str).collect();
            let mut perm: Vec<usize> = (0..defined.len()).collect();
            let mut rng = Rng(h64(&format!("{sha}:V4perm")));
            for k in (1..perm.len()).rev() {
                let j = (rng.next() % (k as u64 + 1)) as usize;
                perm.swap(k, j);
            }
            let pos: HashMap<&str, usize> = defined.iter().enumerate().map(|(k, d)| (*d, k)).collect();
            let rul_perm: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| {
                    s.iter()
                        .filter_map(|x| pos.get(x.as_str()).map(|&k| defined[perm[k]]))
                        .flat_map(|x| by_packet.get(x).into_iter().flatten().map(|r| r.to_string()))
                        .collect()
                })
                .collect();
            let rul_p_perm: Vec<BTreeSet<String>> = rul_perm.iter().map(plan_only).collect();
            // V14: the cited packets AND their rulings (packet ids and ruling keys are disjoint names).
            let union = |a: &[BTreeSet<String>], b: &[BTreeSet<String>]| -> Vec<BTreeSet<String>> {
                a.iter().zip(b).map(|(x, y)| x.union(y).cloned().collect()).collect()
            };
            let sigs = vec![
                ("V1".to_string(), Sig::build(&i)),
                ("V2fam".to_string(), Sig::build(&fam)),
                ("V2bat".to_string(), Sig::build(&bat)),
                ("V2wav".to_string(), Sig::build(&wav)),
                ("V2m".to_string(), Sig::build(&m)),
                ("V1m".to_string(), Sig::build(&r)),
                ("V4".to_string(), Sig::build(&rul)),
                ("V4p".to_string(), Sig::build(&rul_p)),
                ("V14".to_string(), Sig::build(&union(&i, &rul))),
                ("V2mP".to_string(), Sig::build(&m_pend)),
                ("V2mD".to_string(), Sig::build(&m_done)),
            ];
            let mut ctrls = vec![("V0".to_string(), sigs[0].1.shuffled(&format!("{sha}:V1:uniform"), false))];
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            ctrls.push(("V4perm".into(), Sig::build(&rul_perm)));
            ctrls.push(("V4pperm".into(), Sig::build(&rul_p_perm)));
            ctrls.push(("V14perm".into(), Sig::build(&union(&i, &rul_perm))));
            let msg_toks: BTreeSet<String> =
                dot_ids(msg).into_iter().filter(|(pfx, _)| gd.prefixes.contains(pfx)).map(|(_, id)| id).collect();
            let (vm, vm_cov) = msg_route(&sigs[5].1, &msg_toks, nf);
            info.insert("code_files_citing".into(), i.iter().filter(|s| !s.is_empty()).count() as f64);
            info.insert("packets_defined".into(), gd.packets.len() as f64);
            info.insert("rulings".into(), gd.rulings.len() as f64);
            info.insert("rulings_packets".into(), gd.rulings.values().map(BTreeSet::len).sum::<usize>() as f64);
            info.insert("packets_with_wave".into(), gd.wave.len() as f64);
            let mut vocab: Vec<String> = gd.prefixes.iter().cloned().collect();
            let mut rk: Vec<String> = gd.rulings.iter().map(|(k, v)| format!("{k} -> {}", v.len())).collect();
            rk.truncate(400);
            vocab.push(format!("rulings: {}", rk.join("; ")));
            VLayer { sigs, ctrls, vm: Some(vm), vm_cov, info, vocab }
        }
        VMode::Quokka => {
            let lines = git_grep(env.git, &p, None, &["features", "docs", ".ai", "*.md", ":!**/node_modules/**"]);
            let mut m = vec![BTreeSet::new(); nf];
            let mut units: BTreeSet<String> = BTreeSet::new();
            for (path, l) in &lines {
                let segs: Vec<&str> = path.split('/').collect();
                let unit = if segs.len() >= 3 && segs[0] == "features" { format!("features/{}", segs[1]) } else { path.clone() };
                for f in env.pr.files(l) {
                    m[f as usize].insert(unit.clone());
                    units.insert(unit.clone());
                }
            }
            let sigs = vec![("V2m".to_string(), Sig::build(&m))];
            let mut ctrls = Vec::new();
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            info.insert("doc_units_naming_files".into(), units.len() as f64);
            VLayer { sigs, ctrls, vm: None, vm_cov: false, info, vocab: units.into_iter().collect() }
        }
    }
}

fn vertical_names(mode: VMode) -> &'static [&'static str] {
    match mode {
        VMode::Kina => &["V1", "V2", "V3", "V3d", "V2m", "V1m", "V2w", "V3w", "V2mw", "V2mP", "V2mD"],
        VMode::Glia => &["V1", "V2fam", "V2bat", "V2wav", "V2m", "V1m", "V4", "V4p", "V14", "V2mP", "V2mD"],
        VMode::Quokka => &["V2m"],
    }
}

fn build_layers(mode: VMode, args: &Args, paths: &[String], pidx: &HashMap<&str, u32>, tests: &[(String, String)]) -> Vec<VLayer> {
    let mut wiki_revs = Vec::new();
    let mut wiki_head_docs = Vec::new();
    if mode == VMode::Kina {
        assert!(!args.wiki.is_empty(), "--vertical kina needs --wiki");
        let text = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
        for l in text.lines() {
            if let Some((s, ct)) = l.split_once('\x1f') {
                wiki_revs.push((ct.trim().parse().unwrap_or(0), s.to_string()));
            }
        }
        wiki_revs.sort();
        let head = git_text(&args.wiki, &["rev-parse", "HEAD"]).expect("wiki head").trim().to_string();
        wiki_head_docs = git_grep(&args.wiki, &head, None, &["*.md"]);
        eprintln!("[rank_eval] vertical: wiki {} commits, HEAD {head}, {} doc lines", wiki_revs.len(), wiki_head_docs.len());
    }
    let env = VEnv {
        mode,
        git: &args.git,
        wiki: &args.wiki,
        paths,
        pidx,
        pr: PathRes::new(paths),
        parents: git_parents(&args.git, &args.rev),
        wiki_revs,
        wiki_head_docs,
        wiki_lines: std::sync::Mutex::new(HashMap::new()),
        cache: BlobCache { text: std::sync::Mutex::new(HashMap::new()), packets: std::sync::Mutex::new(HashMap::new()) },
    };
    tests.par_iter().map(|(sha, msg)| build_layer(&env, sha, msg)).collect()
}

// ---- rankers per query ----

/// HT grid: (a T1, b V, c P1).
const HT_GRID: [(&str, f64, f64, f64); 4] =
    [("a", 0.5, 0.25, 0.25), ("b", 0.25, 0.5, 0.25), ("c", 0.25, 0.25, 0.5), ("d", 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0)];

fn lex2(a: &[f64], b: &[f64]) -> Vec<Key> {
    a.iter().zip(b).map(|(&x, &y)| [x, y, 0.0]).collect()
}

fn vertical_rankers(out: &mut Out, vl: &VLayer, seed: usize, t: &[u32], p1: &[f64], t1: &[f64], direct: &HashSet<(u32, u32)>) {
    let nf = p1.len();
    let mm_p1 = minmax(p1, seed);
    let mm_t1 = minmax(t1, seed);
    let h9 = blend(0.5, &mm_t1, &mm_p1);
    let mm_h9 = minmax(&h9, seed);
    for (name, sig) in &vl.ctrls {
        out.push(name.clone(), &scalar(&sig.score(seed)), seed, t);
    }
    let is_t: Vec<bool> = {
        let mut v = vec![false; nf];
        for &x in t {
            v[x as usize] = true;
        }
        v
    };
    for (name, sig) in &vl.sigs {
        let v = sig.score(seed);
        out.push(name.clone(), &scalar(&v), seed, t);
        let mm_v = minmax(&v, seed);
        for a in [0.25, 0.5] {
            out.push(format!("HP{}[{name}]", (a * 100.0) as u32), &scalar(&blend(a, &mm_v, &mm_p1)), seed, t);
        }
        out.push(format!("HPlex[{name}]"), &lex2(&v, p1), seed, t);
        for b in [0.25, 0.5] {
            out.push(format!("HR{}[{name}]", (b * 100.0) as u32), &scalar(&blend(b, &mm_v, &mm_h9)), seed, t);
        }
        out.push(format!("HRlex[{name}]"), &lex2(&v, &h9), seed, t);
        for (tag, a, b, c) in HT_GRID {
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1[f] + b * mm_v[f] + c * mm_p1[f]).collect();
            out.push(format!("HT{tag}[{name}]"), &scalar(&s), seed, t);
        }
        let mut d = [0.0; 6];
        for f in 0..nf {
            if f == seed {
                continue;
            }
            let linked = v[f] > 0.0;
            let key = ((seed as u32).min(f as u32), (seed as u32).max(f as u32));
            if is_t[f] {
                d[1] += 1.0;
                if linked {
                    d[0] += 1.0;
                }
                if !direct.contains(&key) {
                    d[3] += 1.0;
                    if linked {
                        d[2] += 1.0;
                    }
                }
            } else {
                d[5] += 1.0;
                if linked {
                    d[4] += 1.0;
                }
            }
        }
        out.vdiag.push((name.clone(), d));
    }
    if let Some(vm) = &vl.vm {
        out.push("VM", &scalar(vm), seed, t);
        let mm_vm = minmax(vm, seed);
        for b in [0.25, 0.5] {
            out.push(format!("HM{}", (b * 100.0) as u32), &scalar(&blend(b, &mm_vm, &mm_h9)), seed, t);
        }
        out.push("HMlex", &lex2(vm, &h9), seed, t);
    }
}

// ---- the vertical report ----

fn paired_sub(a: &[f64], b: &[f64], idx: &[usize], commit_of: &[usize], boot: usize, lf: &[f64]) -> Option<Paired> {
    if idx.is_empty() {
        return None;
    }
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut co = Vec::with_capacity(idx.len());
    let (mut aa, mut bb) = (Vec::with_capacity(idx.len()), Vec::with_capacity(idx.len()));
    for &q in idx {
        let n = map.len();
        co.push(*map.entry(commit_of[q]).or_insert(n));
        aa.push(a[q]);
        bb.push(b[q]);
    }
    Some(paired(&aa, &bb, &co, map.len(), boot, lf))
}

/// The signal whose covered subset a vertical ranker is judged on.
fn family_of(name: &str) -> String {
    if name == "V0" {
        return "V1".into();
    }
    if let Some(x) = name.strip_suffix("perm") {
        return x.into();
    }
    if name == "VM" || name.starts_with("HM") {
        return "VM".into();
    }
    if let Some(i) = name.find('[') {
        return name[i + 1..name.len() - 1].to_string();
    }
    name.to_string()
}

/// The paired-bootstrap inputs: each query's test commit, the resample count, ln n! table.
struct BootIn<'a> {
    commit_of: &'a [usize],
    boot: usize,
    lf: &'a [f64],
}

fn vertical_report(
    label: &str,
    mode: VMode,
    names: &[String],
    results: &[Out],
    queries: &[Query],
    layers: &[VLayer],
    bi: &BootIn,
) -> (serde_json::Value, String) {
    let (commit_of, boot, lf) = (bi.commit_of, bi.boot, bi.lf);
    let nq = queries.len();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    let sig_names: Vec<String> = vertical_names(mode).iter().map(|s| s.to_string()).collect();
    // coverage per family
    let mut cov: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (k, n) in sig_names.iter().enumerate() {
        let idx: Vec<usize> =
            (0..nq).filter(|&q| !layers[queries[q].tc].sigs[k].1.sets[queries[q].seed as usize].is_empty()).collect();
        cov.insert(n.clone(), idx);
    }
    if mode != VMode::Quokka {
        cov.insert("VM".into(), (0..nq).filter(|&q| layers[queries[q].tc].vm_cov).collect());
    }
    let all: Vec<usize> = (0..nq).collect();
    let baselines = ["B0", "P1", "P1sum", "C0", "H10", "T1", "H9a25[P1]", "H9a50[P1]"];
    let vnames: Vec<String> = names
        .iter()
        .filter(|n| {
            n.starts_with('V') || n.starts_with("HP") || n.starts_with("HR") || n.starts_with("HT") || n.starts_with("HM")
        })
        .cloned()
        .collect();
    let mean_on = |ri: usize, idx: &[usize]| -> [f64; 5] {
        let mut m = [0.0; 5];
        for &q in idx {
            for (acc, x) in m.iter_mut().zip(results[q].m[ri]) {
                *acc += x;
            }
        }
        m.map(|x| if idx.is_empty() { f64::NAN } else { x / idx.len() as f64 })
    };
    let fmt5 = |m: [f64; 5]| m.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(" | ");
    let mut md = String::new();
    let mut js = serde_json::Map::new();
    md.push_str(&format!("### {label}: vertical layers\n\n"));

    // ---- coverage and pair links ----
    md.push_str("Coverage and pair links. A query is covered when its seed links to at least one upper-layer node at the parent revision. Pair columns: share of (seed, co-changed file) pairs that share a node; the same for pairs with no direct code edge; for (seed, other candidate) pairs; lift = first / third.\n\n");
    md.push_str("| signal | covered queries | share | test commits with a covered seed | P(link \\| co-change) | P(link \\| co-change, no edge) | P(link \\| other) | lift | co-change pairs with no edge but a link (share of all co-change pairs) |\n|---|---|---|---|---|---|---|---|---|\n");
    let mut cov_js = serde_json::Map::new();
    for n in &sig_names {
        let idx = &cov[n];
        let commits: BTreeSet<usize> = idx.iter().map(|&q| queries[q].tc).collect();
        let mut d = [0.0; 6];
        for r in results {
            if let Some((_, x)) = r.vdiag.iter().find(|(m, _)| m == n) {
                for i in 0..6 {
                    d[i] += x[i];
                }
            }
        }
        let p_co = d[0] / d[1].max(1.0);
        let p_ne = d[2] / d[3].max(1.0);
        let p_ot = d[4] / d[5].max(1.0);
        let lift = if p_ot > 0.0 { p_co / p_ot } else { f64::NAN };
        md.push_str(&format!(
            "| {n} | {} | {:.3} | {} / {} | {p_co:.3} | {p_ne:.3} | {p_ot:.4} | {lift:.1} | {:.3} |\n",
            idx.len(),
            idx.len() as f64 / nq as f64,
            commits.len(),
            layers.len(),
            d[2] / d[1].max(1.0)
        ));
        cov_js.insert(
            n.clone(),
            json!({"covered_queries": idx.len(), "share": idx.len() as f64 / nq as f64, "commits": commits.len(),
                "p_link_cochange": p_co, "p_link_cochange_no_edge": p_ne, "p_link_other": p_ot, "lift": lift,
                "cochange_pairs": d[1], "cochange_pairs_no_edge": d[3]}),
        );
    }
    if let Some(idx) = cov.get("VM") {
        let commits: BTreeSet<usize> = idx.iter().map(|&q| queries[q].tc).collect();
        let cm = layers.iter().filter(|l| l.vm_cov).count();
        md.push_str(&format!(
            "| VM (message ids) | {} | {:.3} | {} / {} (messages citing a known id: {cm}) | - | - | - | - | - |\n",
            idx.len(),
            idx.len() as f64 / nq as f64,
            commits.len(),
            layers.len()
        ));
        cov_js.insert("VM".into(), json!({"covered_queries": idx.len(), "commits": commits.len(), "messages_citing": cm}));
    }
    let direct_pairs: f64 = results.iter().filter_map(|r| r.vdiag.first()).map(|(_, x)| x[1] - x[3]).sum();
    let all_pairs: f64 = results.iter().filter_map(|r| r.vdiag.first()).map(|(_, x)| x[1]).sum();
    md.push_str(&format!(
        "\n(seed, co-changed file) pairs: {all_pairs}; joined by a direct code edge: {direct_pairs} ({:.3}).\n\n",
        direct_pairs / all_pairs.max(1.0)
    ));
    js.insert("coverage".into(), serde_json::Value::Object(cov_js));

    // ---- layer facts ----
    let mut info: BTreeMap<String, f64> = BTreeMap::new();
    for l in layers {
        for (k, v) in &l.info {
            *info.entry(k.clone()).or_default() += v;
        }
    }
    let nl = layers.len().max(1) as f64;
    md.push_str("Layer facts (summed over test commits; `resolve_*` count id citations by how they were scoped; per-commit means in brackets):\n\n");
    for (k, v) in &info {
        md.push_str(&format!("- {k}: {v} [{:.1}]\n", v / nl));
    }
    if let Some(l) = layers.last() {
        md.push_str(&format!("\nVocabulary at the last test commit's parent: {}\n\n", l.vocab.join("; ")));
    }
    js.insert("info".into(), json!(info));
    js.insert("vocab_last".into(), json!(layers.last().map(|l| l.vocab.clone()).unwrap_or_default()));

    // ---- metric tables ----
    let mut rows: Vec<String> = baselines.iter().filter(|b| pos(b).is_some()).map(|s| s.to_string()).collect();
    rows.extend(vnames.iter().cloned());
    md.push_str("ALL queries:\n\n| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n");
    let mut all_js = serde_json::Map::new();
    for r in &rows {
        let m = mean_on(pos(r).unwrap(), &all);
        md.push_str(&format!("| {r} | {} |\n", fmt5(m)));
        all_js.insert(r.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
    }
    js.insert("metrics_all".into(), serde_json::Value::Object(all_js));
    let mut covj = serde_json::Map::new();
    for (fam, idx) in &cov {
        md.push_str(&format!(
            "\nCOVERED by {fam} ({} queries, {:.1}%):\n\n| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n",
            idx.len(),
            100.0 * idx.len() as f64 / nq as f64
        ));
        let mut fj = serde_json::Map::new();
        let fam_rows: Vec<String> = baselines
            .iter()
            .filter(|b| pos(b).is_some())
            .map(|s| s.to_string())
            .chain(vnames.iter().filter(|n| &family_of(n) == fam || (fam == "V1" && *n == "V0")).cloned())
            .collect();
        for r in &fam_rows {
            let m = mean_on(pos(r).unwrap(), idx);
            md.push_str(&format!("| {r} | {} |\n", fmt5(m)));
            fj.insert(r.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
        }
        covj.insert(fam.clone(), serde_json::Value::Object(fj));
    }
    js.insert("metrics_covered".into(), serde_json::Value::Object(covj));

    // ---- paired ----
    let mut pj: Vec<serde_json::Value> = Vec::new();
    for (mi, mname) in [(1usize, "recall@10"), (3, "MRR")] {
        for (subset, sname) in [(false, "ALL queries"), (true, "COVERED queries (each row on its own signal's covered subset)")] {
            md.push_str(&format!(
                "\nPaired {mname}, {sname} (ranker minus baseline; mean, 95% commit-cluster bootstrap CI, sign-test p, wins/losses):\n\n| ranker | n | vs P1 | vs H9a50[P1] | vs V0 | vs own control V0df[X] | vs H10 |\n|---|---|---|---|---|---|---|\n"
            ));
            for r in &vnames {
                let fam = family_of(r);
                let idx: &[usize] = if subset { cov.get(&fam).map_or(&[][..], |v| &v[..]) } else { &all };
                let a = col(pos(r).unwrap(), mi);
                let own = format!("V0df[{fam}]");
                let mut cells = Vec::new();
                for bl in ["P1", "H9a50[P1]", "V0", own.as_str(), "H10"] {
                    let Some(bi) = pos(bl) else {
                        cells.push("-".to_string());
                        continue;
                    };
                    if bi == pos(r).unwrap() {
                        cells.push("=".into());
                        continue;
                    }
                    match paired_sub(&a, &col(bi, mi), idx, commit_of, boot, lf) {
                        Some(p) => {
                            cells.push(format!("{:+.3} [{:+.3}, {:+.3}] p={:.1e} {}/{}", p.mean, p.lo, p.hi, p.p, p.wins, p.losses));
                            pj.push(json!({"ranker": r, "vs": bl, "metric": if mi == 1 { "r10" } else { "mrr" },
                                "subset": if subset { fam.as_str() } else { "all" }, "n": idx.len(),
                                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                        }
                        None => cells.push("-".into()),
                    }
                }
                md.push_str(&format!("| {r} | {} | {} |\n", idx.len(), cells.join(" | ")));
            }
        }
    }
    js.insert("paired".into(), json!(pj));

    // ---- contrasts: does an upper layer add to the layer below it? ----
    // (a, b, subset family): a minus b on ALL and on the family's covered subset.
    let mut contrasts: Vec<(String, String, String)> = Vec::new();
    let shapes = ["", "HP25", "HP50", "HPlex", "HR25", "HR50", "HTd"];
    let mut layer_pair = |hi: &str, lo: &str, fam: &str| {
        for sh in shapes {
            let (a, b) = if sh.is_empty() { (hi.to_string(), lo.to_string()) } else { (format!("{sh}[{hi}]"), format!("{sh}[{lo}]")) };
            contrasts.push((a, b, fam.to_string()));
        }
    };
    match mode {
        VMode::Kina => {
            layer_pair("V2", "V1", "V1");
            layer_pair("V3", "V2", "V3");
            layer_pair("V3d", "V2", "V3d");
            layer_pair("V1m", "V1", "V1");
            layer_pair("V2m", "V1", "V2m");
            layer_pair("V2w", "V2", "V2");
            layer_pair("V2mP", "V2mD", "V2m");
        }
        VMode::Glia => {
            layer_pair("V4", "V2fam", "V4");
            layer_pair("V4", "V1", "V4");
            layer_pair("V4p", "V2fam", "V4p");
            layer_pair("V14", "V1", "V1");
            layer_pair("V2fam", "V1", "V1");
            layer_pair("V2wav", "V1", "V2wav");
            layer_pair("V2m", "V1", "V1");
            layer_pair("V2mP", "V2mD", "V2m");
            for (a, b, f) in [("V4", "V4perm", "V4"), ("V4p", "V4pperm", "V4p"), ("V14", "V14perm", "V1")] {
                contrasts.push((a.into(), b.into(), f.into()));
            }
        }
        VMode::Quokka => {}
    }
    if !contrasts.is_empty() {
        md.push_str("\nContrasts (a minus b; recall@10 and MRR; 95% commit-cluster bootstrap CI; on ALL queries and on the named covered subset):\n\n| a | b | subset | n | recall@10 ALL | recall@10 COVERED | MRR ALL | MRR COVERED |\n|---|---|---|---|---|---|---|---|\n");
        let mut cj = Vec::new();
        for (a, b, fam) in &contrasts {
            let (Some(ia), Some(ib)) = (pos(a), pos(b)) else { continue };
            let sub: &[usize] = cov.get(fam).map_or(&[][..], |v| &v[..]);
            let mut cells = Vec::new();
            for mi in [1usize, 3] {
                for idx in [&all[..], sub] {
                    match paired_sub(&col(ia, mi), &col(ib, mi), idx, commit_of, boot, lf) {
                        Some(p) => {
                            cells.push(format!("{:+.3} [{:+.3}, {:+.3}]", p.mean, p.lo, p.hi));
                            cj.push(json!({"a": a, "b": b, "metric": if mi == 1 { "r10" } else { "mrr" },
                                "subset": if idx.len() == all.len() { "all" } else { fam.as_str() }, "n": idx.len(),
                                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                        }
                        None => cells.push("-".into()),
                    }
                }
            }
            md.push_str(&format!("| {a} | {b} | {fam} | {} | {} |\n", sub.len(), cells.join(" | ")));
        }
        js.insert("contrasts".into(), json!(cj));
    }
    (serde_json::Value::Object(js), md)
}
