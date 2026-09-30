//! CD.1d — `communities`: seeded Leiden over the code graph, with structured,
//! located per-community summaries.
//!
//! One Python repo, two packages. `pkg_a/core.py` defines `f0..f5`, each `fi`
//! calling `f(i+1 mod 6)` and `f(i+2 mod 6)`, `f3` also calling `pkg_b`'s
//! `g1` (imported with `from pkg_b.core import g1`), and `main()` calling
//! `f0`; `pkg_b/core.py` defines `g0..g5` wired the same way. Both
//! `__init__.py` files are empty. As built: CALLS 26 (one crossing,
//! `pkg_a::core::f3 -> pkg_b::core::g1`), DEFINES 13, IMPORTS 1 (the module
//! `pkg_a::core -> pkg_b::core`, which crosses too), and the two `__init__`
//! modules have no edge at all.
//!
//! The fired_on marker is read from a child process: `child_communities`
//! re-runs this test binary on one tree with `--nocapture` and the parent
//! reads its stderr (the `channel_owner_rpc_event.rs` way).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use glia_code_domain::edge_category;
use glia_engine::communities::{CommunitiesAnswer, CommunityArgs, CommunitySummary, communities};
use glia_engine::generate_one;
use glia_engine::profile::CODE_PROFILE;
use glia_graph::MergedGraph;

/// Set on the child run: the tree `child_communities` builds.
const CHILD_ENV: &str = "GLIA_CD1D_CHILD_DIR";

/// `core.py` of one package: `<p>0..<p>5` in a ring with chords, `f3` also
/// calling the imported `g1`, and `main` in `pkg_a`.
fn core_py(p: char, import: Option<&str>) -> String {
    let mut s = import
        .map(|m| format!("from {m}.core import g1\n\n"))
        .unwrap_or_default();
    for i in 0..6 {
        let extra = if p == 'f' && i == 3 { "\n    g1()" } else { "" };
        s.push_str(&format!(
            "\ndef {p}{i}():\n    {p}{}()\n    {p}{}(){extra}\n\n",
            (i + 1) % 6,
            (i + 2) % 6
        ));
    }
    if p == 'f' {
        s.push_str("\ndef main():\n    f0()\n");
    }
    s
}

fn write_fixture(root: &Path) {
    let files = [
        ("pkg_a/__init__.py", String::new()),
        ("pkg_a/core.py", core_py('f', Some("pkg_b"))),
        ("pkg_b/__init__.py", String::new()),
        ("pkg_b/core.py", core_py('g', None)),
    ];
    for (rel, src) in &files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
}

fn build(root: &Path) -> (MergedGraph, BTreeMap<u64, String>) {
    let r = generate_one(root.to_str().expect("utf-8 temp path")).expect("generate_one");
    (r.merged, r.repo_labels)
}

/// The fixture under `<tempdir>/<leaf>`. A repo with no git is keyed by its
/// directory name (LB.1), so another leaf name gives other node ids.
fn fixture_at(leaf: &str) -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join(leaf);
    write_fixture(&root);
    let (m, labels) = build(&root);
    (tmp, m, labels)
}

fn fixture() -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    fixture_at("fixture")
}

fn run(m: &MergedGraph, labels: &BTreeMap<u64, String>, scope: Option<&str>) -> CommunitiesAnswer {
    let mut args = CommunityArgs::default();
    args.scope = scope.map(String::from);
    communities(m, labels, &args)
}

fn by_label<'a>(a: &'a CommunitiesAnswer, label: &str) -> &'a CommunitySummary {
    a.communities
        .iter()
        .find(|c| c.label == label)
        .unwrap_or_else(|| panic!("no community labelled {label}: {a:#?}"))
}

fn qnames(c: &CommunitySummary) -> Vec<&str> {
    let mut q: Vec<&str> = c.top_members.iter().map(|m| m.qname.as_str()).collect();
    q.sort_unstable();
    q
}

fn w(c: glia_core::EdgeCategoryId) -> u64 {
    u64::from(CODE_PROFILE.tables.community_weight(c))
}

/// The 1-based line of `def main():` in the fixture's `pkg_a/core.py`.
fn main_line() -> i64 {
    let src = core_py('f', Some("pkg_b"));
    let at = src
        .lines()
        .position(|l| l == "def main():")
        .expect("main is defined");
    at as i64 + 1
}

#[test]
fn two_packages_two_communities() {
    let (_tmp, m, labels) = fixture();
    let a = run(&m, &labels, None);
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(a.method, "leiden");
    assert_eq!((a.seed, a.resolution), (42, 1.0));
    assert_eq!(a.communities.len(), 2, "{a:#?}");
    assert_eq!(a.total, 2);
    assert_eq!(a.nodes, 17, "2 packages x (__init__ + core + functions)");
    assert_eq!(
        a.isolated, 2,
        "the empty __init__ modules have no weighted edge"
    );
    assert!(
        a.modularity > 0.3,
        "two dense rings joined by one call: {}",
        a.modularity
    );

    let labels_listed: Vec<&str> = a.communities.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels_listed,
        ["pkg_a::core", "pkg_b::core"],
        "size order: 8 before 7"
    );
    assert_eq!(
        a.communities.iter().map(|c| c.id).collect::<Vec<_>>(),
        [0, 1]
    );

    let pa = by_label(&a, "pkg_a::core");
    let pb = by_label(&a, "pkg_b::core");
    let fns = |p: char| -> Vec<String> {
        (0..6)
            .map(|i| format!("pkg_{}::core::{p}{i}", if p == 'f' { 'a' } else { 'b' }))
            .collect()
    };
    let mut want_a: Vec<String> = fns('f');
    want_a.extend(["pkg_a::core".to_string(), "pkg_a::core::main".to_string()]);
    want_a.sort();
    let mut want_b: Vec<String> = fns('g');
    want_b.push("pkg_b::core".to_string());
    want_b.sort();
    assert_eq!((pa.size, pb.size), (8, 7));
    assert_eq!(
        qnames(pa),
        want_a.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        qnames(pb),
        want_b.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for c in &a.communities {
        assert_eq!(c.tier, "heuristic");
        assert_eq!(c.files, 1);
        assert!(
            c.sinks.is_empty(),
            "no effect sink in the fixture: {:?}",
            c.sinks
        );
    }
    assert_eq!(pa.kinds, [("FUNCTION", 7), ("MODULE", 1)]);
    assert_eq!(pb.kinds, [("FUNCTION", 6), ("MODULE", 1)]);
    assert_eq!(
        pa.services,
        [("pkg_a".to_string(), 8)],
        "no manifest: top-level dirs"
    );
    assert_eq!(pb.services, [("pkg_b".to_string(), 7)]);

    // f0 is called by f4, f5 and main, calls f1 and f2, and is DEFINED by
    // its module: the heaviest member.
    let top = &pa.top_members[0];
    assert_eq!(top.qname, "pkg_a::core::f0");
    assert_eq!(
        top.weight,
        5 * w(edge_category::CALLS) + w(edge_category::DEFINES)
    );
    assert_eq!(top.file.as_deref(), Some("pkg_a/core.py"));
    assert!(top.line.is_some_and(|l| l >= 1));
    assert_eq!(top.kind, "FUNCTION");

    assert_eq!(pa.entries.len(), 1, "{:?}", pa.entries);
    assert_eq!(pa.entries[0].qname, "pkg_a::core::main");
    assert_eq!(pa.entries[0].line, Some(main_line()), "1-based");
    assert!(pb.entries.is_empty(), "{:?}", pb.entries);

    // The one call and the module import cross, and nothing else.
    let crossing = w(edge_category::CALLS) + w(edge_category::IMPORTS);
    assert_eq!(pa.links.len(), 1);
    let link = &pa.links[0];
    assert_eq!(link.to, pb.id);
    assert_eq!(link.edges, 2);
    assert_eq!(link.categories, [("CALLS", 1), ("IMPORTS", 1)]);
    assert_eq!(link.weight, crossing);
    assert_eq!(pb.links.len(), 1, "a link is listed from both ends");
    assert_eq!(
        (pb.links[0].to, pb.links[0].weight, pb.links[0].edges),
        (pa.id, crossing, 2)
    );

    // pkg_a inside: 12 ring calls + main -> f0, 7 DEFINES.
    let inside_a = 13 * w(edge_category::CALLS) + 7 * w(edge_category::DEFINES);
    let want = inside_a as f64 / (inside_a + crossing) as f64;
    assert!(
        (pa.cohesion - want).abs() < 1e-12,
        "{} vs {want}",
        pa.cohesion
    );
}

/// The answer as JSON with every located entry's node `id` removed: ids hash
/// the repo's identity key, nothing else in the answer may depend on it.
fn json_without_node_ids(a: &CommunitiesAnswer) -> serde_json::Value {
    let mut v = serde_json::to_value(a).expect("serialise");
    if let Some(list) = v.get_mut("communities").and_then(|c| c.as_array_mut()) {
        for c in list {
            if let Some(entries) = c.get_mut("entries").and_then(|e| e.as_array_mut()) {
                for e in entries {
                    if let Some(o) = e.as_object_mut() {
                        o.remove("id");
                    }
                }
            }
        }
    }
    v
}

#[test]
fn deterministic() {
    let (tmp, m, labels) = fixture();
    let one = serde_json::to_string(&run(&m, &labels, None)).expect("serialise");
    let two = serde_json::to_string(&run(&m, &labels, None)).expect("serialise");
    assert_eq!(one, two, "two calls on one build");

    let (m2, labels2) = build(&tmp.path().join("fixture"));
    let again = serde_json::to_string(&run(&m2, &labels2, None)).expect("serialise");
    assert_eq!(one, again, "two builds of one tree");

    // The same code under another directory name: other node ids (so another
    // id order in the view's source), the same answer.
    let (_other, m3, labels3) = fixture_at("renamed_checkout");
    let a1 = run(&m, &labels, None);
    let a3 = run(&m3, &labels3, None);
    assert_ne!(
        a1.communities[0].entries[0].id, a3.communities[0].entries[0].id,
        "a git-less repo is keyed by its directory name"
    );
    assert_eq!(json_without_node_ids(&a1), json_without_node_ids(&a3));
}

#[test]
fn scope_restricts() {
    let (_tmp, m, labels) = fixture();
    let a = run(&m, &labels, Some("pkg_b"));
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(a.communities.len(), 1, "{a:#?}");
    assert_eq!(a.total, 1);
    assert_eq!(
        (a.nodes, a.isolated),
        (8, 1),
        "pkg_b's nodes, its __init__ isolated"
    );
    let c = &a.communities[0];
    assert_eq!(c.label, "pkg_b::core");
    assert_eq!(c.size, 7);
    assert!(
        c.top_members.iter().all(|m| m.qname.starts_with("pkg_b::")),
        "{:?}",
        c.top_members
    );
    assert!(
        c.links.is_empty(),
        "an edge leaving the scope is not in the view"
    );
}

#[test]
fn empty_scope_is_an_absence() {
    let (_tmp, m, labels) = fixture();
    for (scope, nodes) in [("pkg_a/__init__.py", 1), ("no_such_dir", 0)] {
        let a = run(&m, &labels, Some(scope));
        assert!(a.communities.is_empty(), "{scope}: {a:#?}");
        assert_eq!((a.total, a.nodes, a.isolated), (0, nodes, nodes), "{scope}");
        let absence = a.absence.as_ref().expect("an empty answer says why");
        assert_eq!(absence.reason, "no_edges", "{scope}");
        assert!(absence.note.contains(scope), "{}", absence.note);
        assert!(absence.mechanisms.contains(&"CALLS"));
    }
}

#[test]
fn options_shape_the_answer() {
    let (_tmp, m, labels) = fixture();

    let mut args = CommunityArgs::default();
    args.top = 1;
    args.members = 3;
    let a = communities(&m, &labels, &args);
    assert_eq!(
        (a.communities.len(), a.total),
        (1, 2),
        "top cuts the list, total counts all"
    );
    assert_eq!(a.communities[0].top_members.len(), 3);
    assert_eq!(
        a.communities[0].label, "pkg_a::core",
        "the label is the top members' prefix"
    );

    let mut args = CommunityArgs::default();
    args.min_size = 8;
    let a = communities(&m, &labels, &args);
    assert_eq!((a.communities.len(), a.total), (1, 2));
    args.min_size = 9;
    let a = communities(&m, &labels, &args);
    assert!(a.communities.is_empty());
    assert_eq!(a.absence.as_ref().map(|x| x.reason), Some("no_match"));

    let mut args = CommunityArgs::default();
    args.method = Some("lpa".to_string());
    let a = communities(&m, &labels, &args);
    assert_eq!(a.method, "label_propagation");
    assert_eq!(a.total, 2, "label propagation finds the two rings too");

    let mut args = CommunityArgs::default();
    args.method = Some("louvain".to_string());
    let a = communities(&m, &labels, &args);
    assert!(a.communities.is_empty());
    let absence = a.absence.as_ref().expect("an unknown method is refused");
    assert_eq!(absence.reason, "no_match");
    assert!(absence.note.contains("louvain"), "{}", absence.note);

    let mut args = CommunityArgs::default();
    args.resolution = 0.5;
    args.seed = 7;
    let a = communities(&m, &labels, &args);
    assert_eq!(
        (a.seed, a.resolution),
        (7, 0.5),
        "the answer reports what it ran with"
    );
}

/// Child half of `marker_line`: runs the answer on the tree in [`CHILD_ENV`]
/// and prints it; a no-op in a normal test run.
#[test]
fn child_communities() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let (m, labels) = build(Path::new(&dir));
        let a = communities(&m, &labels, &CommunityArgs::default());
        println!("{}", serde_json::to_string(&a).expect("serialise"));
    }
}

#[test]
fn marker_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_fixture(tmp.path());
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_communities",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, tmp.path())
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed: {stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[communities] "))
        .collect();
    assert_eq!(lines.len(), 1, "one marker per call: {stderr}");
    let line = lines[0];
    assert!(
        line.starts_with("[communities] method=leiden nodes=17 weighted_edges="),
        "{line}"
    );
    for part in [
        " communities=2 ",
        " listed=2 ",
        " isolated=2 ",
        " seed=42 ",
        " surface=engine",
    ] {
        assert!(line.contains(part), "{part} in {line}");
    }
    assert!(
        line.contains(" modularity=0.") && line.contains(" levels="),
        "{line}"
    );
}
