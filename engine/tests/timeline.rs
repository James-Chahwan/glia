//! CD.5c acceptance: the time-travel graph (`glia_engine::timeline`) built
//! over the first-parent revs of a real git repo through the shared hermetic
//! harness, then read back as edge history and as-of views.

mod git_fixture;

use std::path::PathBuf;

use git_fixture::GitRepo;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk, reach};
use glia_code_domain::edge_category;
use glia_core::NodeId;
use glia_engine::timeline::{
    AsOfView, EdgeHistoryRow, TimelineArgs, TimelineBuilt, as_of, build_timeline, edge_history, load_timeline,
};
use glia_engine::{generate_one, generate_one_incremental};
use glia_store::TimelineStore;

/// Commit 1: `f` calls `g`.
const C1: &str = "def f():\n    return g()\n\n\ndef g():\n    return 1\n";
/// Commit 2: `h` added, `f` calls both.
const C2: &str = "def f():\n    g()\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n";
/// Commit 4 (in the moved file): `f` calls `h` only.
const C4: &str = "def f():\n    return h()\n\n\ndef g():\n    return 1\n\n\ndef h():\n    return 2\n";

/// The four-commit history of the acceptance: f -> g (1), + h and f -> h (2),
/// `git mv app/a.py app/b.py` (3), f -> g removed (4).
fn four_commits() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("app/a.py", C1);
    repo.commit("f calls g");
    repo.write("app/a.py", C2);
    repo.commit("f calls h");
    repo.git_mv("app/a.py", "app/b.py");
    repo.commit("move a to b");
    repo.write("app/b.py", C4);
    repo.commit("f drops g");
    repo
}

fn args(revs: usize, persist: bool) -> TimelineArgs {
    let mut a = TimelineArgs::default();
    a.revs = revs;
    a.persist = persist;
    a
}

fn build(repo: &GitRepo, revs: usize) -> TimelineBuilt {
    build_timeline(repo.path(), &args(revs, true)).unwrap_or_else(|e| panic!("timeline build: {e}"))
}

fn timeline_file(repo: &GitRepo) -> PathBuf {
    repo.root().join(".glia/graph/timeline.gmap")
}

fn store(repo: &GitRepo) -> TimelineStore {
    load_timeline(repo.path()).unwrap_or_else(|e| panic!("load timeline: {e}"))
}

fn show(rows: &[EdgeHistoryRow]) -> String {
    rows.iter()
        .map(|r| {
            format!(
                "  {} {} {} since {} until {:?}\n",
                r.direction,
                r.category,
                r.other_qname,
                r.since.index,
                r.until.as_ref().map(|u| u.index)
            )
        })
        .collect()
}

fn calls_to<'a>(rows: &'a [EdgeHistoryRow], suffix: &str) -> Vec<&'a EdgeHistoryRow> {
    rows.iter()
        .filter(|r| r.category == "CALLS" && r.direction == "out" && r.other_qname.ends_with(suffix))
        .collect()
}

fn has_edge(v: &AsOfView, from: NodeId, to: NodeId) -> bool {
    v.edges().any(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
}

fn named(v: &AsOfView, q: &str) -> NodeId {
    v.node_named(q).unwrap_or_else(|| panic!("`{q}` at rev {}", v.rev.index))
}

#[test]
fn build_over_four_revs() {
    let repo = four_commits();
    let built = build(&repo, 4);
    assert!(built.skipped.is_empty(), "{:?}", built.skipped);
    let subjects: Vec<&str> = built.revs.iter().map(|r| r.subject.as_str()).collect();
    assert_eq!(subjects, ["f calls g", "f calls h", "move a to b", "f drops g"], "oldest first");
    let indexes: Vec<u32> = built.revs.iter().map(|r| r.index).collect();
    assert_eq!(indexes, [0, 1, 2, 3]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(built.revs[3].sha, String::from_utf8_lossy(&head.stdout).trim());
    assert!(built.revs.iter().all(|r| r.sha.len() == 40 && r.time > 0));
    assert!(built.moves >= 4, "the MODULE and f, g, h moved at rev 2: {}", built.moves);
    assert!(built.closed >= 1 && built.edge_spans >= built.edges && built.nodes > 0);
    let path = timeline_file(&repo);
    assert_eq!(built.written.as_deref(), path.to_str());
    assert!(path.is_file());
    // The sidecar dir hides itself: the work tree stays clean.
    let status = repo.git(&["status", "--porcelain"]);
    assert!(status.stdout.is_empty(), "{}", String::from_utf8_lossy(&status.stdout));
    let s = store(&repo);
    assert_eq!(s.revs.len(), 4);
    assert_eq!(s.revs[0].subject, "f calls g");
    // A window wider than the history holds every commit.
    let all = build(&repo, 20);
    assert_eq!(all.revs.len(), 4);
}

#[test]
fn history_of_f() {
    let repo = four_commits();
    build(&repo, 4);
    let merged = generate_one(repo.path()).expect("working-tree build").merged;
    let s = store(&repo);
    // f is ONE node span despite the rename, under its new module.
    let f: Vec<_> = s.nodes.iter().filter(|n| s.node_qname(n).is_some_and(|q| q.ends_with("::f"))).collect();
    assert_eq!(f.len(), 1, "one span for f");
    assert_eq!(s.node_qname(f[0]), Some("app::b::f"));
    assert_eq!(s.node_file(f[0]), Some("app/b.py"));
    assert_eq!(f[0].prior.len(), 1, "one move chained");
    assert_eq!(f[0].prior[0].0, 2, "the id changed at rev 2");

    let answer = edge_history(&merged, &s, "f", None);
    assert!(answer.absence.is_none());
    let rows = &answer.results;
    let g = calls_to(rows, "::g");
    assert_eq!(g.len(), 1, "one f -> g row:\n{}", show(rows));
    assert_eq!(g[0].other_qname, "app::b::g");
    assert_eq!(g[0].other_kind, "FUNCTION");
    assert_eq!(g[0].since.index, 0);
    assert!(g[0].since_window_start, "present at the first rev: its true start is unknown");
    assert_eq!(g[0].until.as_ref().map(|u| u.index), Some(3));
    assert_eq!(g[0].until.as_ref().map(|u| u.subject.as_str()), Some("f drops g"));
    assert_eq!(g[0].tier, "derived");
    assert_eq!(g[0].file.as_deref(), Some("app/b.py"));
    assert_eq!(g[0].line, Some(5), "g's def line, 1-based, as last seen");
    let h = calls_to(rows, "::h");
    assert_eq!(h.len(), 1, "one f -> h row:\n{}", show(rows));
    assert_eq!(h[0].since.index, 1);
    assert_eq!(h[0].since.subject, "f calls h");
    assert!(!h[0].since_window_start);
    assert!(h[0].until.is_none(), "still present");
    // Sorted by (since, category, other).
    let keys: Vec<(u32, &str, &str)> =
        rows.iter().map(|r| (r.since.index, r.category, r.other_qname.as_str())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);

    // The category filter keeps exactly the CALLS rows.
    let calls = edge_history(&merged, &s, "f", Some("calls"));
    assert!(calls.results.iter().all(|r| r.category == "CALLS"));
    assert_eq!(calls.results.len(), 2, "{}", show(&calls.results));
    // g is called: an `in` row from f.
    let into_g = edge_history(&merged, &s, "app::b::g", Some("CALLS"));
    assert!(
        into_g.results.iter().any(|r| r.direction == "in" && r.other_qname == "app::b::f"),
        "{}",
        show(&into_g.results)
    );

    // A resolved node with no edge of the category, and an unknown symbol.
    let none = edge_history(&merged, &s, "f", Some("HTTP_CALLS"));
    let why = none.absence.expect("empty answer carries its absence");
    assert_eq!(why.reason, "no_edges");
    assert!(why.mechanisms.contains(&"HTTP_CALLS"));
    let unknown = edge_history(&merged, &s, "nope_not_here", None);
    assert_eq!(unknown.absence.map(|a| a.reason), Some("unknown_symbol"));
}

#[test]
fn as_of_views() {
    let repo = four_commits();
    build(&repo, 4);
    let s = store(&repo);

    let v1 = as_of(&s, "1").expect("rev 1");
    let (f, g, h) = (named(&v1, "f"), named(&v1, "g"), named(&v1, "h"));
    assert!(has_edge(&v1, f, g) && has_edge(&v1, f, h));
    let sum = v1.summary();
    assert!(sum.edges >= 2 && sum.nodes >= 4, "{sum:?}");
    assert!(sum.by_category.get("CALLS").is_some_and(|n| *n >= 2), "{sum:?}");

    // By sha prefix: the same rev.
    let v3 = as_of(&s, &s.revs[3].sha[..7]).expect("rev 3 by sha");
    assert_eq!(v3.rev.index, 3);
    let (f3, g3, h3) = (named(&v3, "f"), named(&v3, "g"), named(&v3, "h"));
    assert!(has_edge(&v3, f3, h3));
    assert!(!has_edge(&v3, f3, g3), "f -> g is gone at rev 3");
    assert!(v3.edges().all(|e| e.category != edge_category::CALLS || e.to != g3));

    // Ids are the ones each rev had: f moved at rev 2.
    let v0 = as_of(&s, "0").expect("rev 0");
    let (f0, g0) = (named(&v0, "f"), named(&v0, "g"));
    assert_ne!(f0, f3, "a moved node's id is path-dependent");
    assert_eq!(v0.node_ids().iter().filter(|id| **id == f0).count(), 1);
    // GraphSource: reachability over the view.
    let adj = Adjacency::build(&v0, &CategorySet::of(&[edge_category::CALLS]));
    let walk = reach::bfs(&adj, &[f0], Walk::Forward, 4);
    assert!(walk.reached.iter().any(|r| r.id == g0), "f reaches g at rev 0");
    assert!(!v0.node_ids().contains(&h3), "h does not exist at rev 0");

    assert!(as_of(&s, "4").is_err(), "no rev 4");
    assert!(as_of(&s, "zzzzzzz").is_err());
}

#[test]
fn empty_rev_closes_and_reopens() {
    let repo = GitRepo::init();
    repo.write("app/a.py", C1);
    repo.commit("f calls g");
    repo.remove("app/a.py");
    repo.commit("delete everything");
    repo.write("app/a.py", C1);
    repo.commit("restore");
    let built = build(&repo, 3);
    assert!(built.skipped.is_empty(), "{:?}", built.skipped);
    assert_eq!(built.revs.len(), 3);
    let s = store(&repo);
    // Presence is observed per rev: every span open at rev 0 closes at the
    // empty rev, and the restore opens new ones.
    assert!(s.nodes.iter().filter(|n| n.from_rev == 0).all(|n| n.until() == Some(1)));
    assert!(s.edges.iter().filter(|e| e.from_rev == 0).all(|e| e.until() == Some(1)));
    assert!(s.nodes.iter().all(|n| !n.covers(1)), "nothing at the empty rev");
    let merged = generate_one(repo.path()).expect("working-tree build").merged;
    let rows = edge_history(&merged, &s, "f", Some("CALLS")).results;
    let spans: Vec<(u32, Option<u32>)> =
        calls_to(&rows, "::g").iter().map(|r| (r.since.index, r.until.as_ref().map(|u| u.index))).collect();
    assert_eq!(spans, [(0, Some(1)), (2, None)], "the gap is not bridged:\n{}", show(&rows));
    let v1 = as_of(&s, "1").expect("the empty rev");
    assert_eq!(v1.summary().edges, 0);
}

#[test]
fn cache_untouched() {
    let repo = four_commits();
    generate_one_incremental(repo.path()).expect("incremental build writes the cache");
    let cache = repo.root().join(".glia/graph/parse_cache.bin");
    let before = std::fs::read(&cache).expect("parse cache written");
    build(&repo, 4);
    let after = std::fs::read(&cache).expect("parse cache still there");
    assert!(before == after, "the rev builds must not save the parse cache");
}

#[test]
fn no_persist() {
    let repo = four_commits();
    let built = build_timeline(repo.path(), &args(4, false)).expect("timeline build");
    assert!(built.written.is_none());
    assert_eq!(built.revs.len(), 4);
    assert!(!timeline_file(&repo).exists());
    let err = load_timeline(repo.path()).expect_err("no sidecar");
    assert!(err.contains("glia timeline build"), "{err}");
}

#[test]
fn deterministic() {
    let repo = four_commits();
    build(&repo, 4);
    let first = std::fs::read(timeline_file(&repo)).expect("sidecar");
    std::fs::remove_file(timeline_file(&repo)).expect("remove sidecar");
    build(&repo, 4);
    let second = std::fs::read(timeline_file(&repo)).expect("sidecar");
    assert!(first == second, "two builds of one history write the same bytes");
}

#[test]
fn errors_are_named() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.py"), C1).expect("write");
    let path = dir.path().to_str().expect("utf-8");
    let err = build_timeline(path, &args(4, false)).expect_err("not a git work tree");
    assert!(err.contains("git"), "{err}");
    let repo = four_commits();
    assert!(build_timeline(repo.path(), &args(0, false)).is_err(), "an empty window");
    assert!(build_timeline(repo.path(), &args(201, false)).is_err(), "over the cap");
    let mut bad = args(4, false);
    bad.head = "--output=x".to_string();
    assert!(build_timeline(repo.path(), &bad).is_err(), "an option is never a rev");
}
