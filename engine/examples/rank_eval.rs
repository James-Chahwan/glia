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
}

#[derive(Default)]
struct Out {
    names: Vec<String>,
    m: Vec<[f64; 5]>,
    hub_share: Vec<(String, f64)>,
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

fn run_query(ctx: &Ctx, q: &Query, tc: &TestCommit, push: &mut PushPpr) -> Out {
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
        push_eps: args.push_eps,
    };

    // ---- timing pass (sequential) + replica check against production ----
    let timing = timing_pass(&ctx, &queries, args.timing, &base_cfg);

    // ---- the evaluation (parallel over queries) ----
    let t_eval = Instant::now();
    let wf = |c: EdgeCategoryId| ctx.push_w.get(&c).copied().unwrap_or(1.0);
    let results: Vec<Out> = queries
        .par_iter()
        .map_init(|| PushPpr::new(&ctx.adj_push, &wf), |push, q| run_query(&ctx, q, &tcs[q.tc], push))
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
    let doc = json!({
        "stats": stats,
        "rankers": names,
        "metrics": metrics_json,
        "paired": paired_json,
        "inefficiency": ineff,
        "hub_share_top20_mass_on_top5_degree_files": hub,
        "timing_ms_per_query": timing,
    });
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
