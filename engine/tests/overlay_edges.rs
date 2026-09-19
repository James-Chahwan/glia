//! LF.2b: `.glia/overlay.toml` `[[edge]]` stanzas land as Weak cross edges
//! carrying their provenance (ORIGIN) and evidence (the stanza's line), and
//! `BuildOptions::with_overlay(false)` builds the extraction-only graph, byte
//! for byte the graph of the same tree without the overlay file.
//!
//! The web / api sources and the overlay file are the substrate-gap fixture
//! `xcut-overlay-edges`, copied into a temp dir per test so a test can edit or
//! delete the overlay file.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, Confidence, Edge, EdgeCategoryId, RepoId};
use glia_engine::{BuildOptions, GenerateResult, generate_many_opts};
use glia_graph::MergedGraph;
use glia_store::write_merged_sharded;

const REPORT_TS: &str = include_str!("../../bench/substrate-gap/fixtures/xcut-overlay-edges/web/src/report.ts");
const REPORT_PY: &str = include_str!("../../bench/substrate-gap/fixtures/xcut-overlay-edges/api/report.py");
const OVERLAY: &str = include_str!("../../bench/substrate-gap/fixtures/xcut-overlay-edges/web/.glia/overlay.toml");

/// `<tmp>/web` + `<tmp>/api` with the fixture sources; the web repo's overlay
/// file is `overlay` (none when `None`). Returns the two repo paths.
fn tree(tmp: &Path, overlay: Option<&str>) -> Vec<String> {
    let web = tmp.join("web");
    let api = tmp.join("api");
    std::fs::create_dir_all(web.join("src")).unwrap();
    std::fs::create_dir_all(web.join(".glia")).unwrap();
    std::fs::create_dir_all(&api).unwrap();
    std::fs::write(web.join("src/report.ts"), REPORT_TS).unwrap();
    std::fs::write(api.join("report.py"), REPORT_PY).unwrap();
    set_overlay(&web, overlay);
    vec![web.to_string_lossy().into_owned(), api.to_string_lossy().into_owned()]
}

fn set_overlay(repo: &Path, overlay: Option<&str>) {
    let file = repo.join(".glia/overlay.toml");
    match overlay {
        Some(text) => std::fs::write(file, text).unwrap(),
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn build(repos: &[String], overlay: bool) -> GenerateResult {
    generate_many_opts(repos, false, &BuildOptions::default().with_overlay(overlay)).expect("build")
}

/// The qname of every node, and the repo it sits in.
fn qname(m: &MergedGraph, id: glia_core::NodeId) -> Option<(RepoId, String)> {
    m.graphs.iter().find_map(|g| g.nav.qname_by_id.get(&id).map(|q| (g.repo, q.clone())))
}

/// Every edge (intra and cross) `from -[category]-> to`, by exact qname.
fn edges<'m>(m: &'m MergedGraph, from: &str, to: &str, category: EdgeCategoryId) -> Vec<&'m Edge> {
    m.all_edges()
        .filter(|e| {
            e.category == category
                && qname(m, e.from).is_some_and(|(_, q)| q == from)
                && qname(m, e.to).is_some_and(|(_, q)| q == to)
        })
        .collect()
}

fn edge_count(m: &MergedGraph) -> usize {
    m.all_edges().count()
}

/// Every file of a written layout, by name, with its bytes.
fn layout_bytes(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
    write_merged_sharded(m, dir).expect("write layout");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

fn assert_same_layout(a: &MergedGraph, b: &MergedGraph, out: &Path, context: &str) {
    let (la, lb) = (layout_bytes(a, &out.join("a")), layout_bytes(b, &out.join("b")));
    let names = |l: &[(String, Vec<u8>)]| l.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&la), names(&lb), "{context}: file sets differ");
    for ((name, x), (_, y)) in la.iter().zip(&lb) {
        assert_eq!(x, y, "{context}: {name} bytes differ");
    }
}

/// 0-based line of the `n`th (1-based) `[[edge]]` header of `text`.
fn stanza_line(text: &str, n: usize) -> u32 {
    let i = text.lines().enumerate().filter(|(_, l)| l.trim() == "[[edge]]").nth(n - 1).expect("stanza").0;
    u32::try_from(i).unwrap()
}

#[test]
fn overlay_edge_is_applied_weak() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), Some(OVERLAY));
    let r = build(&repos, true);
    let m = &r.merged;
    let hits = edges(m, "src::report::loadReport", "report::build_report", edge_category::CALLS);
    assert_eq!(hits.len(), 1, "exactly one declared CALLS edge");
    let e = hits[0];
    assert_eq!(e.confidence, Confidence::Weak, "an llm stanza is Weak");
    assert!(m.cross_edges.iter().any(|c| std::ptr::eq(c, e)), "a cross edge");
    let origin = e.cell(cell_type::ORIGIN).expect("ORIGIN edge cell");
    assert_eq!(
        origin.payload,
        CellPayload::Json(
            r#"{"provenance":"overlay:llm","rule":"edge#1","note":"URL built in buildUrl()"}"#.into()
        )
    );
    let ev = Evidence::of(e).expect("evidence");
    assert_eq!(ev.emitter, "overlay:edge");
    assert_eq!(ev.rule.as_deref(), Some("edge#1"));
    assert_eq!(ev.file.as_deref(), Some(".glia/overlay.toml"));
    assert_eq!(ev.line, Some(stanza_line(OVERLAY, 1)), "the stanza's [[edge]] line, 0-based");
    assert_eq!(ev.basis, Basis::Site);
    // The web repo holds the stanza, the api repo the handler.
    let (from_repo, _) = qname(m, e.from).unwrap();
    let (to_repo, _) = qname(m, e.to).unwrap();
    assert_ne!(from_repo, to_repo, "a cross-repo pairing");
}

#[test]
fn no_overlay_hides_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), Some(OVERLAY));
    let with = build(&repos, true);
    let without = build(&repos, false);
    assert!(edges(&without.merged, "src::report::loadReport", "report::build_report", edge_category::CALLS).is_empty());
    assert_eq!(edge_count(&without.merged) + 1, edge_count(&with.merged));

    // The fixture's file holds only [[edge]] stanzas, so the build without the
    // overlay is the build of the same tree with the file deleted, byte for
    // byte (user-config and declared sections are not switched off).
    set_overlay(&PathBuf::from(&repos[0]), None);
    let deleted = build(&repos, true);
    assert_same_layout(&without.merged, &deleted.merged, &tmp.path().join("out"), "--no-overlay vs no file");
}

#[test]
fn structural_category_never_lands() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), Some(OVERLAY));
    let m = build(&repos, true).merged;
    assert!(edges(&m, "src::report::loadReport", "report::helper", edge_category::DEFINES).is_empty());
    let any = m.all_edges().filter(|e| {
        qname(&m, e.from).is_some_and(|(_, q)| q == "src::report::loadReport")
            && qname(&m, e.to).is_some_and(|(_, q)| q == "report::helper")
    });
    assert_eq!(any.count(), 0, "the rejected stanza adds no edge of any category");
}

#[test]
fn orphan_qname_is_counted_not_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let orphan = "version = 1\n\n[[edge]]\nfrom = \"src::report::loadReport\"\nto = \"report::gone\"\ncategory = \"CALLS\"\n";
    let repos = tree(tmp.path(), Some(orphan));
    let m = build(&repos, true).merged;
    let bare = build(&repos, false).merged;
    assert_eq!(edge_count(&m), edge_count(&bare), "an orphaned stanza adds nothing");
    assert!(m.all_edges().all(|e| Evidence::of(e).is_none_or(|ev| !ev.emitter.starts_with("overlay:"))));
}

#[test]
fn redundant_stanza_adds_no_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), None);
    let api = PathBuf::from(&repos[1]);
    std::fs::create_dir_all(api.join(".glia")).unwrap();
    set_overlay(
        &api,
        Some("version = 1\n\n[[edge]]\nfrom = \"report::build_report\"\nto = \"report::helper\"\ncategory = \"CALLS\"\n"),
    );
    let m = build(&repos, true).merged;
    let bare = build(&repos, false).merged;
    assert_eq!(edge_count(&m), edge_count(&bare), "the extractor already has this edge");
    let hits = edges(&m, "report::build_report", "report::helper", edge_category::CALLS);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].cell(cell_type::ORIGIN).is_none(), "the extracted edge, not an overlay copy");
}

#[test]
fn own_repo_binds_before_other_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
    for d in [&a, &b] {
        std::fs::create_dir_all(d.join(".glia")).unwrap();
        std::fs::write(d.join("util.py"), "def f():\n    return 1\n").unwrap();
    }
    std::fs::write(a.join("main.py"), "def run():\n    return 0\n").unwrap();
    // a: `util::f` exists in both repos -> a's own. b: `main::run` only in a.
    set_overlay(&a, Some("version = 1\n\n[[edge]]\nfrom = \"main::run\"\nto = \"util::f\"\ncategory = \"USES\"\n"));
    set_overlay(&b, Some("version = 1\n\n[[edge]]\nfrom = \"util::f\"\nto = \"main::run\"\ncategory = \"USES\"\n"));
    let repos = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
    let m = build(&repos, true).merged;
    let repo_a = m.graphs.first().map(|g| g.repo).unwrap();
    let uses: Vec<_> = m
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::USES)
        .map(|e| (qname(&m, e.from).unwrap(), qname(&m, e.to).unwrap()))
        .collect();
    assert_eq!(uses.len(), 2, "{uses:?}");
    let a_edge = uses.iter().find(|(f, _)| f.1 == "main::run").expect("a's stanza");
    assert_eq!(a_edge.1.0, repo_a, "util::f binds in the stanza's own repo first");
    let b_edge = uses.iter().find(|(f, _)| f.1 == "util::f").expect("b's stanza");
    assert_ne!(b_edge.0.0, repo_a, "b's util::f is b's own");
    assert_eq!(b_edge.1.0, repo_a, "main::run falls back to the other repo");
}

#[test]
fn malformed_toml_builds_without_overlay() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), Some("version = 1\n[[edge]\nfrom = \"x\"\n"));
    let broken = build(&repos, true);
    set_overlay(&PathBuf::from(&repos[0]), None);
    let none = build(&repos, true);
    assert_same_layout(&broken.merged, &none.merged, &tmp.path().join("out"), "malformed vs no file");
}

#[test]
fn overlay_builds_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let repos = tree(tmp.path(), Some(OVERLAY));
    let one = build(&repos, true);
    let two = build(&repos, true);
    assert_eq!(edges(&one.merged, "src::report::loadReport", "report::build_report", edge_category::CALLS).len(), 1);
    assert_same_layout(&one.merged, &two.merged, &tmp.path().join("out"), "clean vs clean");
}
