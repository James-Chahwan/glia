//! EA.5: blast radius, diff-impact and review ranked by the selected
//! `Ranking`. The closure keeps its own directional PPR for the row scores
//! (over a cached `PprIndex`, bit-identical); the measured file ranking
//! (EA.4's `rank_files` from the seeds' files) decides which files' rows
//! come first (`order_rows`), and the answer gains the ranked `files` list.
//!
//! The tree: `svc/core.py` defines `core`; `svc/a.py`, `b.py`, `c.py` each
//! call it once, `svc/d.py` twice (`d1`, `d2`); `web/page.ts` is unrelated
//! code. The history snapshot (EA.3's shape, cut to this tree): core changed
//! with c 5 times, with page 4 times, with b twice and alone 5 times, so
//! core's count is 16 and C0 is c 5/17, page 4/17, b 2/17.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};
use glia_core::NodeId;
use glia_engine::rank::{RankBase, RankCache, Ranking, rank_files};
use glia_engine::{BlastOptions, BlastRadius, blast_radius, generate_one};
use glia_graph::{MergedGraph, Reach};

const T0: i64 = 1_767_225_600;
const CHILD_ENV: &str = "GLIA_RANK_BLAST_CHILD";

const CORE: &str = "svc/core.py";
const A: &str = "svc/a.py";
const B: &str = "svc/b.py";
const C: &str = "svc/c.py";
const D: &str = "svc/d.py";
const PAGE: &str = "web/page.ts";

/// The HEAD (0.5.2 / pre-EA.5) backward depth-1 rows from `core`, captured
/// at landing with `glia blast-radius <tree> core --direction backward
/// --depth 1 --json`: five rows of one score, by node id.
const HEAD_ROWS: [&str; 5] = ["svc::a::a", "svc::d::d2", "svc::b::b", "svc::d::d1", "svc::c::c"];

fn tree() -> Vec<(&'static str, &'static str)> {
    vec![
        (CORE, "def core(x):\n    return x\n"),
        (A, "from svc.core import core\n\n\ndef a():\n    return core(1)\n"),
        (B, "from svc.core import core\n\n\ndef b():\n    return core(1)\n"),
        (C, "from svc.core import core\n\n\ndef c():\n    return core(1)\n"),
        (
            D,
            "from svc.core import core\n\n\ndef d1():\n    return core(1)\n\n\ndef d2():\n    return core(2)\n",
        ),
        (PAGE, "export function renderPage(t: string): string {\n  return t;\n}\n"),
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
    let groups: Vec<(Vec<&str>, usize)> = vec![
        (vec![CORE, C], 5),
        (vec![CORE, PAGE], 4),
        (vec![CORE, B], 2),
        (vec![CORE], 5),
    ];
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

fn write_tree(root: &Path) {
    for (p, text) in tree() {
        let path = root.join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
}

fn write_snapshot(root: &Path) {
    let h = history();
    let head = h.first().map_or_else(|| "0".repeat(40), |c| c.c.clone());
    write_history(root, HistoryMeta::new(head, 2000, None, String::new()), &h, &[])
        .expect("write snapshot");
}

struct Built {
    merged: MergedGraph,
    roots: BTreeMap<u64, String>,
    labels: BTreeMap<u64, String>,
}

fn build(root: &Path) -> Built {
    let r = generate_one(&root.to_string_lossy()).expect("build");
    Built {
        merged: r.merged,
        roots: r.repo_roots,
        labels: r.repo_labels,
    }
}

/// The tree's root under a tempdir: a fixed directory name, so the repo id
/// (`dir:tree`, `gitdir:tree`) and the node ids the head order ties by do not
/// depend on where the tempdir sits.
fn root_in(d: &tempfile::TempDir) -> std::path::PathBuf {
    let root = d.path().join("tree");
    std::fs::create_dir_all(&root).expect("mkdir tree");
    root
}

fn tree_with_snapshot() -> (tempfile::TempDir, Built) {
    let d = tempfile::tempdir().expect("tempdir");
    let root = root_in(&d);
    write_tree(&root);
    write_snapshot(&root);
    let b = build(&root);
    (d, b)
}

fn cache(b: &Built) -> Arc<RankCache> {
    Arc::new(RankCache::new("answer", &b.roots, &b.labels))
}

fn opts(ranking: Ranking, cache: Option<Arc<RankCache>>) -> BlastOptions {
    let mut o = BlastOptions::default();
    o.direction = Reach::Backward;
    o.depth = 1;
    o.ranking = ranking;
    o.rank_cache = cache;
    o
}

fn qnames(r: &BlastRadius) -> Vec<&str> {
    r.results.iter().map(|a| a.qname.as_str()).collect()
}

fn files_of(r: &BlastRadius) -> Vec<&str> {
    r.results.iter().map(|a| a.file.as_deref().unwrap_or("")).collect()
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serialise")
}

fn seed_file(b: &Built, c: &RankCache) -> u32 {
    let files = c.files(&b.merged);
    files.index_of(files.repo(0), CORE).expect("core.py is a candidate")
}

/// (1) Ranking::head(): the HEAD rows, and `files` is P1's top 10.
#[test]
fn head_ranking_keeps_the_head_rows_and_lists_p1_files() {
    let (_d, b) = tree_with_snapshot();
    let c = cache(&b);
    let r = blast_radius(&b.merged, &["core"], &opts(Ranking::head(), Some(c.clone())));
    assert_eq!(r.seeds.len(), 1);
    assert_eq!(r.seeds[0].file.as_deref(), Some(CORE), "the seed is core in svc/core.py");
    assert_eq!(qnames(&r), HEAD_ROWS);

    let files = c.files(&b.merged);
    let fr = rank_files(&b.merged, &c, &[seed_file(&b, &c)], &[], Ranking::head());
    let mut per: BTreeMap<u32, usize> = BTreeMap::new();
    for a in &r.results {
        let f = files.file_of_node(NodeId(a.id)).expect("a row's file is a candidate");
        *per.entry(f).or_default() += 1;
    }
    assert_eq!(json(&r.files), json(&fr.top(&files, 10, &per)));
    assert!(!r.files.iter().any(|f| f.file == CORE), "a seed file is never listed");
}

/// (2) The default (H10): co-changed files' rows first by C0, the rest in
/// the head order; `files` leads with c, page (no rows), b.
#[test]
fn default_ranking_lifts_cochanged_files() {
    let (_d, b) = tree_with_snapshot();
    let r = blast_radius(&b.merged, &["core"], &opts(Ranking::default(), Some(cache(&b))));
    assert_eq!(qnames(&r), ["svc::c::c", "svc::b::b", "svc::a::a", "svc::d::d2", "svc::d::d1"]);
    assert_eq!(files_of(&r)[0], C);
    let top: Vec<(&str, f64, usize)> =
        r.files.iter().take(3).map(|f| (f.file.as_str(), f.cochange, f.rows)).collect();
    assert_eq!(top, [(C, 5.0 / 17.0, 1), (PAGE, 4.0 / 17.0, 0), (B, 2.0 / 17.0, 1)]);
    assert!(!r.files.iter().any(|f| f.file == CORE), "a seed file is never listed");
}

/// (3) PT* without co-change: each file's rows contiguous, files in PT*'s
/// order; the closure scores are the head answer's in f64 bits.
#[test]
fn ppr_wide_groups_rows_by_file_and_keeps_scores() {
    let (_d, b) = tree_with_snapshot();
    let c = cache(&b);
    let head = blast_radius(&b.merged, &["core"], &opts(Ranking::head(), Some(c.clone())));
    let wide = Ranking::new(RankBase::PprWide, false);
    let r = blast_radius(&b.merged, &["core"], &opts(wide, Some(c.clone())));

    let files = c.files(&b.merged);
    let fr = rank_files(&b.merged, &c, &[seed_file(&b, &c)], &[], wide);
    let mut seen: Vec<&str> = Vec::new();
    for f in files_of(&r) {
        if seen.last() != Some(&f) {
            assert!(!seen.contains(&f), "{f}'s rows are contiguous: {:?}", files_of(&r));
            seen.push(f);
        }
    }
    let place = |p: &str| fr.position(files.index_of(files.repo(0), p).expect("candidate"));
    let places: Vec<Option<usize>> = seen.iter().map(|p| place(p)).collect();
    let mut sorted = places.clone();
    sorted.sort();
    assert_eq!(places, sorted, "files in PT* order: {seen:?}");

    let by_id: BTreeMap<u64, u64> = head.results.iter().map(|a| (a.id, a.score.to_bits())).collect();
    assert_eq!(r.results.len(), head.results.len());
    for a in &r.results {
        assert_eq!(by_id.get(&a.id), Some(&a.score.to_bits()), "{}'s score", a.qname);
    }
}

/// (5) No snapshot: the default ranking is the head answer byte for byte
/// (co-change is a no-op without history); only `files` is new.
#[test]
fn no_snapshot_default_is_head() {
    let d = tempfile::tempdir().expect("tempdir");
    let root = root_in(&d);
    write_tree(&root);
    let b = build(&root);
    let h = blast_radius(&b.merged, &["core"], &opts(Ranking::head(), Some(cache(&b))));
    let r = blast_radius(&b.merged, &["core"], &opts(Ranking::default(), Some(cache(&b))));
    assert_eq!(json(&r.results), json(&h.results));
    assert_eq!(qnames(&r), HEAD_ROWS);
    // No cache at all (a library caller): a transient one with no roots,
    // unlabelled (`repo#<n>` in `files`).
    let none = blast_radius(&b.merged, &["core"], &opts(Ranking::default(), None));
    assert_eq!(json(&none.results), json(&r.results));
    let paths = |x: &BlastRadius| x.files.iter().map(|f| f.file.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&none), paths(&r));
}

/// An empty answer has `files: []`.
#[test]
fn an_unresolved_seed_lists_no_files() {
    let (_d, b) = tree_with_snapshot();
    let r = blast_radius(&b.merged, &["no_such_symbol"], &opts(Ranking::default(), Some(cache(&b))));
    assert!(r.results.is_empty() && r.files.is_empty());
    let v: serde_json::Value = serde_json::from_str(&json(&r)).expect("json");
    assert_eq!(v["files"], serde_json::json!([]));
}

/// (6) diff_impact_vs_rev, no cache passed: the working tree's snapshot
/// orders the impact rows as (2).
#[test]
fn diff_impact_vs_rev_reads_the_working_trees_snapshot() {
    let d = tempfile::tempdir().expect("tempdir");
    let root_buf = root_in(&d);
    let root = root_buf.as_path();
    write_tree(root);
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-q"]);
    git(&["add", "svc", "web"]);
    git(&["commit", "-q", "-m", "base"]);
    write_snapshot(root);
    std::fs::write(root.join(CORE), "def core(x):\n    return x + 0\n").expect("touch core");

    let run = |ranking: Ranking| {
        let mut o = BlastOptions::default();
        o.direction = Reach::Backward;
        o.depth = 1;
        o.ranking = ranking;
        glia_engine::diff_impact::diff_impact_vs_rev(&root.to_string_lossy(), "HEAD", &o)
            .expect("diff impact")
    };
    let head = run(Ranking::head());
    let di = run(Ranking::default());
    // (2)'s order: c, b lifted, then the rest in the head order (this repo's
    // ids are the git repo's, so the head ties differ from the dir tree's).
    let lifted = ["svc::c::c", "svc::b::b"];
    let mut want: Vec<&str> = lifted.to_vec();
    want.extend(qnames(&head.impact).into_iter().filter(|q| !lifted.contains(q)));
    assert_eq!(head.impact.results.len(), 5);
    assert_eq!(qnames(&di.impact), want);
    assert_eq!(di.impact.files.first().map(|f| f.file.as_str()), Some(C));
}

/// Child half of the marker tests: runs the answer on the tree in
/// [`CHILD_ENV`] twice over one RankCache and prints both answers.
#[test]
fn child_rank_blast() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let b = build(Path::new(&dir));
        let c = cache(&b);
        for _ in 0..2 {
            let r = blast_radius(&b.merged, &["core"], &opts(Ranking::default(), Some(c.clone())));
            println!("{}", json(&r));
        }
    }
}

fn run_child(root: &Path) -> (Vec<String>, String) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_rank_blast", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, root)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child failed: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let answers: Vec<String> = stdout
        .lines()
        .filter_map(|l| l.find("{\"seeds\"").map(|i| l[i..].to_string()))
        .collect();
    assert_eq!(answers.len(), 2, "{stdout}");
    (answers, stderr)
}

fn lines<'a>(stderr: &'a str, prefix: &str) -> Vec<&'a str> {
    stderr.lines().filter(|l| l.starts_with(prefix)).collect()
}

/// (4) The same query twice on one RankCache: the second reads both
/// indexes from the cache, and the answers are identical.
#[test]
fn a_held_cache_answers_the_second_query_from_its_indexes() {
    let (d, _b) = tree_with_snapshot();
    let (answers, stderr) = run_child(&d.path().join("tree"));
    assert_eq!(answers[0], answers[1]);

    let rank = lines(&stderr, "[rank] surface=");
    assert_eq!(rank.len(), 2, "one marker per answer: {stderr}");
    for (i, how) in [(0, "built"), (1, "hit")] {
        let l = rank[i];
        assert!(
            l.starts_with("[rank] surface=blast_radius ranking=H10 seed_files=1 cochange_files=3 files=5 lifted=2 closure_index="),
            "{l}"
        );
        assert!(l.ends_with(&format!("closure_index={how}")), "{l}");
    }
    let cache = lines(&stderr, "[rank-cache] holder=answer");
    let closure: Vec<&&str> = cache.iter().filter(|l| l.contains("direction=backward")).collect();
    let file: Vec<&&str> = cache.iter().filter(|l| l.contains("direction=undirected")).collect();
    assert_eq!(closure.len(), 2, "{stderr}");
    assert_eq!(file.len(), 2, "{stderr}");
    assert!(closure[0].contains("index=built") && closure[1].contains("index=hit"), "{stderr}");
    assert!(file[0].contains("index=built") && file[1].contains("index=hit"), "{stderr}");
    // The marker follows the unchanged `[blast]` line.
    let order: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[blast] seeds=") || l.starts_with("[rank] surface="))
        .collect();
    assert_eq!(order.len(), 4);
    assert!(order[0].starts_with("[blast]") && order[1].starts_with("[rank]"), "{order:?}");
}

/// (5)'s marker: no snapshot, co-change reads nothing.
#[test]
fn no_snapshot_marker() {
    let d = tempfile::tempdir().expect("tempdir");
    let root = root_in(&d);
    write_tree(&root);
    let (answers, stderr) = run_child(&root);
    assert_eq!(answers[0], answers[1]);
    assert_eq!(lines(&stderr, "[rank-cochange] holder=answer none reason=no_snapshot").len(), 1, "{stderr}");
    let rank = lines(&stderr, "[rank] surface=blast_radius ranking=H10 seed_files=1 cochange_files=0 ");
    assert_eq!(rank.len(), 2, "{stderr}");
    assert!(rank.iter().all(|l| l.contains(" lifted=0 ")), "{stderr}");
}
