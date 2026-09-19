//! LE.1b acceptance: the graph delta of a git rev against the working tree
//! (`glia_engine::delta::graph_delta_vs_rev`), every case built on a
//! real git repo through the shared two-commit harness.

mod git_fixture;

use git_fixture::GitRepo;
use glia_code_domain::{DocProvenance, DocRecord, DocSourceKind, node_kind};
use glia_engine::delta::{DeltaEdge, DeltaNode, RevDelta, graph_delta_vs_rev};
use glia_engine::{ParseCache, generate_one, generate_one_with_cache};
use glia_store::write_merged_sharded;

/// `place` calls `price`; `price` sits below it so its HEAD line (5) is not
/// one the working tree could also report.
const A_PY: &str = "def place(o):\n    return price(o)\n\n\ndef price(o):\n    return o\n";
const B_PY: &str = "from shop.a import place\n\n\ndef checkout(o):\n    return place(o)\n";

/// A committed two-file python repo.
fn shop() -> GitRepo {
    let repo = GitRepo::init();
    repo.write("shop/a.py", A_PY);
    repo.write("shop/b.py", B_PY);
    repo.commit("shop");
    repo
}

fn delta(repo: &GitRepo, base: &str) -> RevDelta {
    graph_delta_vs_rev(repo.path(), base).unwrap_or_else(|e| panic!("delta vs {base}: {e}"))
}

fn node<'a>(d: &'a RevDelta, change: &str, suffix: &str) -> Option<&'a DeltaNode> {
    d.answer.nodes.iter().find(|n| n.change == change && n.qname.ends_with(suffix))
}

fn edge<'a>(d: &'a RevDelta, change: &str, category: &str, from: &str, to: &str) -> Option<&'a DeltaEdge> {
    d.answer.edges.iter().find(|e| {
        e.change == change && e.category == category && e.from_qname.ends_with(from) && e.to_qname.ends_with(to)
    })
}

fn rows(d: &RevDelta) -> String {
    let mut s = String::new();
    for n in &d.answer.nodes {
        s.push_str(&format!("  node {} {} {} (was {:?})\n", n.change, n.kind, n.qname, n.before_qname));
    }
    for e in &d.answer.edges {
        s.push_str(&format!("  edge {} {} {} -> {}\n", e.change, e.category, e.from_qname, e.to_qname));
    }
    s
}

fn has_kind(merged: &glia_graph::MergedGraph, kind: glia_core::NodeKindId) -> bool {
    merged.graphs.iter().flat_map(|g| g.nav.kind_by_id.values()).any(|k| *k == kind)
}

#[test]
fn clean_tree_has_empty_delta() {
    let repo = shop();
    let d = delta(&repo, "HEAD");
    assert!(d.answer.nodes.is_empty() && d.answer.edges.is_empty(), "clean tree:\n{}", rows(&d));
    assert_eq!(d.answer.files.reparsed, 0, "every rev file is a cache hit");
    assert_eq!(d.answer.files.reused, 2);
    assert_eq!(d.answer.base, "HEAD");
    // Both sides are real builds of the same repo, under one identity.
    assert!(d.before.total_nodes > 0 && d.before.total_nodes == d.after.total_nodes);
    assert_eq!(d.before.repo_labels, d.after.repo_labels);
}

#[test]
fn added_function_and_call() {
    let repo = shop();
    repo.write(
        "shop/a.py",
        "def place(o):\n    audit(o)\n    return price(o)\n\n\ndef price(o):\n    return o\n\n\ndef audit(o):\n    return o\n",
    );
    let d = delta(&repo, "HEAD");
    let audit = node(&d, "added", "::audit").unwrap_or_else(|| panic!("audit added:\n{}", rows(&d)));
    assert_eq!(audit.kind, "FUNCTION");
    assert_eq!(audit.side, "after");
    assert_eq!(audit.file.as_deref(), Some("shop/a.py"));
    assert_eq!(audit.line, Some(10));
    assert!(node(&d, "modified", "::place").is_some(), "place modified:\n{}", rows(&d));
    assert!(node(&d, "modified", "::price").is_none(), "price did not change:\n{}", rows(&d));
    let call = edge(&d, "added", "CALLS", "::place", "::audit")
        .unwrap_or_else(|| panic!("place -> audit CALLS added:\n{}", rows(&d)));
    assert_eq!(call.site_line, Some(2), "1-based line of the audit(o) call");
    assert_eq!(call.site_file.as_deref(), Some("shop/a.py"));
    assert!(call.emitter.is_some());
    assert_eq!(d.answer.counts.nodes_added, d.answer.nodes.iter().filter(|n| n.change == "added").count());
    assert_eq!(d.answer.files.reparsed, 1, "only shop/a.py differs at HEAD");
    assert_eq!(d.answer.files.reused, 1);
}

#[test]
fn removed_function_is_located_in_head() {
    let repo = shop();
    repo.write("shop/a.py", "def place(o):\n    return o\n");
    let d = delta(&repo, "HEAD");
    let price = node(&d, "removed", "::price").unwrap_or_else(|| panic!("price removed:\n{}", rows(&d)));
    assert_eq!(price.side, "before");
    assert_eq!(price.kind, "FUNCTION");
    assert_eq!(price.file.as_deref(), Some("shop/a.py"));
    assert_eq!(price.line, Some(5), "its def line at HEAD, 1-based");
    let gone = edge(&d, "removed", "CALLS", "::place", "::price")
        .unwrap_or_else(|| panic!("place -> price CALLS removed:\n{}", rows(&d)));
    assert_eq!(gone.site_line, Some(2), "located in the HEAD file");
}

#[test]
fn line_shift_only_is_not_modified() {
    let repo = shop();
    repo.write("shop/a.py", &format!("\n\n\n{A_PY}"));
    let d = delta(&repo, "HEAD");
    assert!(node(&d, "modified", "::place").is_none(), "{}", rows(&d));
    assert!(
        d.answer.nodes.iter().all(|n| n.kind == "MODULE"),
        "only the MODULE (its CODE is the file text) may change:\n{}",
        rows(&d)
    );
    assert!(d.answer.edges.is_empty(), "{}", rows(&d));
}

#[test]
fn untracked_file_is_added() {
    let repo = shop();
    repo.write("shop/c.py", "def refund(o):\n    return o\n");
    let d = delta(&repo, "HEAD");
    let refund = node(&d, "added", "::refund").unwrap_or_else(|| panic!("refund added:\n{}", rows(&d)));
    assert_eq!(refund.file.as_deref(), Some("shop/c.py"));
    assert!(node(&d, "added", "shop::c").is_some_and(|m| m.kind == "MODULE"), "{}", rows(&d));
    assert!(d.answer.nodes.iter().all(|n| n.change == "added"), "{}", rows(&d));
}

#[test]
fn renamed_file_is_moved_not_add_remove() {
    let repo = shop();
    repo.git_mv("shop/a.py", "shop/c.py");
    let d = delta(&repo, "HEAD");
    for local in ["", "::place", "::price"] {
        let moved = d
            .answer
            .nodes
            .iter()
            .find(|n| n.change == "moved" && n.before_qname.as_deref() == Some(&format!("shop::a{local}")[..]))
            .unwrap_or_else(|| panic!("shop::a{local} moved:\n{}", rows(&d)));
        assert_eq!(moved.qname, format!("shop::c{local}"));
        assert_eq!(moved.file.as_deref(), Some("shop/c.py"));
        assert!(moved.before_id.is_some_and(|b| b != moved.id));
    }
    let fn_churn = d
        .answer
        .nodes
        .iter()
        .filter(|n| n.kind == "FUNCTION" && (n.change == "added" || n.change == "removed"))
        .count();
    assert_eq!(fn_churn, 0, "{}", rows(&d));
    assert_eq!(d.answer.counts.nodes_moved, 3);
}

#[test]
fn base_other_than_head() {
    let repo = shop();
    repo.write("shop/a.py", &format!("{A_PY}\n\ndef audit(o):\n    return o\n"));
    repo.commit("audit");
    let d = delta(&repo, "HEAD~1");
    assert_eq!(d.answer.base, "HEAD~1");
    assert!(node(&d, "added", "::audit").is_some(), "{}", rows(&d));
    let head = delta(&repo, "HEAD");
    assert!(head.answer.nodes.is_empty(), "nothing uncommitted:\n{}", rows(&head));
}

#[test]
fn untracked_docs_snapshot_is_not_a_delta() {
    let repo = shop();
    let record = DocRecord {
        rel_path: "confluence/SHOP/Pricing.md".to_string(),
        text: "# Pricing\n\nHow `place` prices an order.\n".to_string(),
        provenance: DocProvenance {
            kind: DocSourceKind::Confluence,
            url: None,
            container: Some("SHOP".to_string()),
            version: Some("1".to_string()),
        },
    };
    let line = serde_json::to_string(&record).expect("DocRecord serialises");
    repo.write(".glia/docs-snapshot/manifest.jsonl", &format!("{line}\n"));
    let d = delta(&repo, "HEAD");
    // The control: the snapshot is read at all, so its absence from the delta
    // is the materialiser's doing.
    assert!(has_kind(&d.after.merged, node_kind::DOC_SECTION), "the working tree reads the snapshot");
    assert!(has_kind(&d.before.merged, node_kind::DOC_SECTION), "the rev side reads it too");
    assert!(!d.answer.nodes.iter().any(|n| n.kind == "DOC_SECTION"), "{}", rows(&d));
    assert!(d.answer.nodes.is_empty() && d.answer.edges.is_empty(), "{}", rows(&d));
}

#[test]
fn ignored_untracked_dir_is_not_a_delta() {
    let repo = GitRepo::init();
    repo.write(".gitignore", "node_modules/\n");
    repo.write("shop/a.py", A_PY);
    repo.write("shop/b.py", B_PY);
    repo.commit("shop");
    repo.write("node_modules/x/index.js", "export function left() { return 1; }\n");
    let d = delta(&repo, "HEAD");
    assert!(has_kind(&d.after.merged, node_kind::REGION), "the working tree collapses node_modules");
    assert!(d.answer.nodes.is_empty() && d.answer.edges.is_empty(), "{}", rows(&d));
    assert!(d.answer.counts.regions_excluded >= 1);
}

#[test]
fn non_git_dir_is_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.py"), "def f():\n    return 1\n").expect("write");
    let path = dir.path().to_str().expect("utf-8 path");
    let err = graph_delta_vs_rev(path, "HEAD").err().expect("a non-git dir has no rev");
    assert!(err.contains("not a git work tree"), "{err}");
    // Checked before anything is built: no parse cache is left behind.
    assert!(!dir.path().join(".glia").exists());
    let repo = shop();
    let err = graph_delta_vs_rev(repo.path(), "no-such-rev").err().expect("unknown rev");
    assert!(err.contains("unknown rev no-such-rev"), "{err}");
}

#[test]
fn normal_builds_unchanged() {
    let repo = shop();
    repo.write("shop/c.py", "def refund(o):\n    return o\n");
    let d = delta(&repo, "HEAD");
    let out = tempfile::tempdir().expect("out dir");
    let (plain, via_delta) = (out.path().join("plain"), out.path().join("delta"));
    write_merged_sharded(&generate_one(repo.path()).expect("build").merged, &plain).expect("write plain");
    write_merged_sharded(&d.after.merged, &via_delta).expect("write delta after");
    let bytes = |dir: &std::path::Path| {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .expect("read out dir")
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).expect("read shard")))
            .collect();
        v.sort();
        v
    };
    assert_eq!(bytes(&plain), bytes(&via_delta), "the delta's after graph is the normal build");
}

#[test]
fn saved_cache_is_the_working_tree_state() {
    let repo = shop();
    repo.write("shop/a.py", "def place(o):\n    return o\n");
    repo.remove("shop/b.py");
    let d = delta(&repo, "HEAD");
    assert!(node(&d, "removed", "::checkout").is_some(), "{}", rows(&d));
    // The before build must not overwrite the sidecar: the next incremental
    // build of the working tree reparses nothing.
    let mut cache = ParseCache::load(repo.path());
    generate_one_with_cache(repo.path(), &mut cache).expect("incremental build");
    let diff = cache.last_diff().expect("a build ran");
    assert!(diff.reparsed.is_empty() && diff.evicted.is_empty(), "{diff:?}");
    assert_eq!(diff.reused, vec!["shop/a.py".to_string()]);
}

#[test]
fn subdirectory_materialises_its_subtree() {
    let repo = GitRepo::init();
    repo.write("svc/shop/a.py", A_PY);
    repo.write("svc/shop/b.py", B_PY);
    repo.write("other/x.py", "def elsewhere():\n    return 1\n");
    repo.commit("monorepo");
    repo.write("svc/shop/c.py", "def refund(o):\n    return o\n");
    let svc = repo.root().join("svc");
    let d = graph_delta_vs_rev(svc.to_str().expect("utf-8"), "HEAD").expect("delta of a subdirectory");
    assert_eq!(d.answer.files.reused, 2, "the rev side holds svc/ only");
    assert!(node(&d, "added", "::refund").is_some_and(|n| n.file.as_deref() == Some("shop/c.py")), "{}", rows(&d));
    assert!(!d.answer.nodes.iter().any(|n| n.qname.contains("elsewhere")), "{}", rows(&d));
    assert!(d.answer.nodes.iter().all(|n| n.change == "added"), "{}", rows(&d));
}

#[test]
fn unstaged_move_pairs_by_body() {
    // A plain `mv`: git sees a deletion and an untracked file, no rename, so
    // LB.6 pairs the file by its identical body instead.
    let repo = shop();
    repo.remove("shop/a.py");
    repo.write("shop/c.py", A_PY);
    let d = delta(&repo, "HEAD");
    let moved: Vec<(&str, &str)> = d
        .answer
        .nodes
        .iter()
        .filter(|n| n.change == "moved")
        .map(|n| (n.before_qname.as_deref().unwrap_or(""), n.qname.as_str()))
        .collect();
    assert_eq!(
        moved,
        [("shop::a", "shop::c"), ("shop::a::place", "shop::c::place"), ("shop::a::price", "shop::c::price")],
        "{}",
        rows(&d)
    );
    assert!(!d.answer.nodes.iter().any(|n| n.kind == "FUNCTION" && n.change != "moved"), "{}", rows(&d));
}

#[test]
fn symlinks_and_gitlinks_do_not_show_as_delta() {
    let repo = shop();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.py", repo.root().join("shop/alias.py")).expect("symlink");
    // A submodule entry with nothing checked out: a gitlink in the tree.
    let head = repo.git(&["rev-parse", "HEAD"]);
    let sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    std::fs::create_dir_all(repo.root().join("vendor/sub")).expect("submodule dir");
    repo.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{sha},vendor/sub")]);
    repo.commit("links");
    let tree = repo.git(&["ls-tree", "-r", "HEAD"]);
    let listing = String::from_utf8_lossy(&tree.stdout).into_owned();
    assert!(listing.contains("160000 commit"), "{listing}");
    #[cfg(unix)]
    assert!(listing.contains("120000 blob"), "{listing}");
    let d = delta(&repo, "HEAD");
    assert!(d.answer.nodes.is_empty() && d.answer.edges.is_empty(), "{}", rows(&d));
}
