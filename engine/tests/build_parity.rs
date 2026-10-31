//! Build-parity capture instrument (L0.6). Not a gate on its own: it writes the
//! sharded store of a fixed set of repos so two builds of the engine (before and
//! after a refactor) can be compared with `diff -r`. It uses only the engine's
//! public API, so the same file compiles against both sides of a move.
//!
//! ```text
//! GLIA_PARITY_REPOS=<dir>:<dir>:… GLIA_PARITY_OUT=<out> \
//!   cargo test -p glia-engine --test build_parity -- --ignored --nocapture 2> <out>.stderr
//! ```
//!
//! For each repo `i` (0-based, in argument order) it writes
//! `generate_one(repo)` into `<out>/<i>`, then one `generate_many` over every
//! repo into `<out>/all`. `manifest.json` records the build stamp, which moves
//! whenever engine sources change, so compare with `diff -r -x manifest.json`.
//! Engine marker lines go to stderr; the `[parity]` summary lines below go there
//! too, so the captured stderr pins node / edge / parse-error counts as well.
//!
//! Reused by LD.13, LD.14a, LG.1a and LG.1b instead of a second parity test.

use std::path::{Path, PathBuf};

use glia_engine::{GenerateResult, generate_many, generate_one};
use glia_store::write_merged_sharded;

fn report(tag: &str, r: &GenerateResult) {
    eprintln!(
        "[parity] {tag} nodes={} edges={} cross_edges={} graphs={} parse_errors={}",
        r.total_nodes,
        r.total_edges,
        r.merged.cross_edges.len(),
        r.merged.graphs.len(),
        r.parse_errors.len()
    );
    for e in &r.parse_errors {
        eprintln!("[parity] {tag} parse_error: {e}");
    }
}

fn capture(tag: &str, r: &GenerateResult, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    write_merged_sharded(&r.merged, dir)
        .unwrap_or_else(|e| panic!("write {tag} into {}: {e:?}", dir.display()));
    report(tag, r);
}

#[test]
#[ignore = "capture instrument: needs GLIA_PARITY_REPOS and GLIA_PARITY_OUT"]
fn capture_merged_shards() {
    let repos: Vec<String> = std::env::var("GLIA_PARITY_REPOS")
        .expect("GLIA_PARITY_REPOS: colon-separated repo dirs")
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    assert!(!repos.is_empty(), "GLIA_PARITY_REPOS names no repo");
    let out =
        PathBuf::from(std::env::var("GLIA_PARITY_OUT").expect("GLIA_PARITY_OUT: output directory"));

    for (i, repo) in repos.iter().enumerate() {
        let tag = format!("one[{i}]");
        let r = generate_one(repo).unwrap_or_else(|e| panic!("generate_one {repo}: {e}"));
        capture(&tag, &r, &out.join(i.to_string()));
    }
    let all = generate_many(&repos).unwrap_or_else(|e| panic!("generate_many: {e}"));
    capture("all", &all, &out.join("all"));
}

// ============================================================================
// GA.2: canonical per-graph order
// ============================================================================
//
// Every language graph leaves its group build with its intra edges and its
// unresolved lists in one order (`RepoGraph::canonicalize`: endpoints by
// `(qname, kind)`, then `glia_core::canonical_edge_cmp`), so a graph built in
// any emission order has one byte form. The golden under `tests/data/canonical/` is the
// content of each corpus build as an order-free multiset (per graph: the
// sorted node lines, edge lines and unresolved lines; then the cross edges),
// captured on the tree before the sort landed: it proves the sort moved only
// order. A packet that changes what a corpus fixture builds regenerates it:
//
// ```text
// GLIA_UPDATE_GOLDEN=1 cargo test -p glia-engine --test engine_tests build_parity::
// ```
//
// `capture_canonical_multisets` (ignored) writes the same dump for every
// substrate-gap fixture, for a before / after diff of a whole-corpus change:
//
// ```text
// GLIA_CANONICAL_FIXTURES=<bench/substrate-gap/fixtures> GLIA_CANONICAL_OUT=<dir> \
//   cargo test -p glia-engine --test engine_tests build_parity::capture_canonical_multisets \
//   -- --ignored --nocapture
// ```

use std::cmp::Ordering;
use std::collections::HashMap;

use glia_code_domain::{
    CallQualifier, CallSite, UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, canonical_edge_cmp};
use glia_graph::RepoGraph;

/// The substrate-gap fixtures of the committed corpus (each built as
/// `bench/substrate-gap/grade.py` builds it: one dir `generate_one`, more
/// `generate_many`), beside the inline multi-language fixture.
const CANONICAL_CORPUS: &[&str] = &[
    "py-calls",
    "go-calls",
    "ts-inherited-calls",
    "java-spring-calls",
    "xstack-go-http",
    "angular-base-url",
];

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/canonical")
}

/// The byte_identical fixture: python + go + typescript + markdown.
fn write_inline_fixture(dir: &Path) {
    let files = [
        (
            "a.py",
            "def foo():\n    \"\"\"Frobnicates.\"\"\"\n    return bar()\n\ndef bar():\n    return 1\n",
        ),
        (
            "b.py",
            "class Widget:\n    def spin(self):\n        return 2\n",
        ),
        ("go.mod", "module example.com/fixture\n"),
        (
            "m.go",
            "package m\n\n// F does a thing.\nfunc F() int { return G() }\n\nfunc G() int { return 1 }\n",
        ),
        (
            "c.ts",
            "export function tsEntry(): number {\n  return 3;\n}\n",
        ),
        ("README.md", "# Fixture\n\nUse `foo` and `Widget.spin`.\n"),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text)
            .unwrap_or_else(|e| panic!("write {}: {e}", dir.join(name).display()));
    }
}

/// The dirs a fixture's key.json names (`["."]` when it names none).
fn fixture_dirs(fixture: &Path) -> Vec<String> {
    let key = std::fs::read_to_string(fixture.join("key.json"))
        .unwrap_or_else(|e| panic!("read {}/key.json: {e}", fixture.display()));
    let key: serde_json::Value =
        serde_json::from_str(&key).unwrap_or_else(|e| panic!("{}: {e}", fixture.display()));
    let dirs: Vec<String> = key
        .get("dirs")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if dirs.is_empty() {
        vec![".".to_string()]
    } else {
        dirs
    }
}

/// Build `dirs` (absolute, canonical) as grade.py does.
fn build_dirs(dirs: &[String]) -> GenerateResult {
    if let [one] = dirs {
        generate_one(one).unwrap_or_else(|e| panic!("generate_one {one}: {e}"))
    } else {
        generate_many(dirs).unwrap_or_else(|e| panic!("generate_many {dirs:?}: {e}"))
    }
}

fn canonical_path(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.display()))
        .to_string_lossy()
        .into_owned()
}

fn confidence_word(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

fn render_cell(c: &Cell) -> String {
    let payload = match &c.payload {
        CellPayload::Text(s) => format!("text:{s}"),
        CellPayload::Json(s) => format!("json:{s}"),
        CellPayload::Bytes(b) => {
            let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
            format!("bytes:{hex}")
        }
    };
    format!("{}={payload}", cell_type::name(c.kind))
}

fn render_cells(cells: &[Cell]) -> String {
    cells
        .iter()
        .map(render_cell)
        .collect::<Vec<_>>()
        .join(" ; ")
}

fn render_qualifier(q: &CallQualifier) -> String {
    match q {
        CallQualifier::Bare(n) => format!("bare({n})"),
        CallQualifier::SelfMethod(n) => format!("self({n})"),
        CallQualifier::SuperMethod(n) => format!("super({n})"),
        CallQualifier::Attribute { base, name } => format!("attr({base}.{name})"),
        CallQualifier::ComplexReceiver { receiver, name } => format!("complex({receiver}.{name})"),
    }
}

/// `<qname>[<KIND>]` for every node of the build, by id (first graph that
/// names it wins: the same id can sit in several graphs with one qname).
fn node_names(r: &GenerateResult) -> HashMap<NodeId, String> {
    let mut names = HashMap::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            names.entry(n.id).or_insert_with(|| {
                let q = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
                let k = g
                    .nav
                    .kind_by_id
                    .get(&n.id)
                    .map(|k| node_kind::name(*k))
                    .unwrap_or("?");
                format!("{q}[{k}]")
            });
        }
    }
    names
}

fn name_of(names: &HashMap<NodeId, String>, id: NodeId) -> String {
    names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| "<foreign>".to_string())
}

fn edge_line(names: &HashMap<NodeId, String>, e: &Edge) -> String {
    format!(
        "E {} -> {} {} {} | {}",
        name_of(names, e.from),
        name_of(names, e.to),
        edge_category::name(e.category),
        confidence_word(e.confidence),
        render_cells(&e.cells)
    )
}

fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines
}

/// The order-free content of one build, with every `roots` spelling replaced
/// by `<root>` (a fixture's absolute path reaches CODE spans and ORIGIN cells).
fn multiset(r: &GenerateResult, roots: &[String]) -> String {
    let names = node_names(r);
    let mut out = Vec::new();
    for (i, g) in r.merged.graphs.iter().enumerate() {
        out.push(format!(
            "## graph {i} nodes={} edges={} unresolved_calls={} unresolved_refs={}",
            g.nodes.len(),
            g.edges.len(),
            g.unresolved_calls.len(),
            g.unresolved_refs.len()
        ));
        out.extend(sorted(
            g.nodes
                .iter()
                .map(|n| {
                    format!(
                        "N {} {} | {}",
                        name_of(&names, n.id),
                        confidence_word(n.confidence),
                        render_cells(&n.cells)
                    )
                })
                .collect(),
        ));
        out.extend(sorted(
            g.edges.iter().map(|e| edge_line(&names, e)).collect(),
        ));
        out.extend(sorted(
            g.unresolved_calls
                .iter()
                .map(|c| {
                    format!(
                        "C {} {} {}",
                        name_of(&names, c.from),
                        render_qualifier(&c.qualifier),
                        c.line
                    )
                })
                .collect(),
        ));
        out.extend(sorted(
            g.unresolved_refs
                .iter()
                .map(|u| {
                    format!(
                        "R {} {} {} {} {}",
                        name_of(&names, u.from),
                        name_of(&names, u.from_module),
                        edge_category::name(u.category),
                        render_qualifier(&u.qualifier),
                        u.line
                    )
                })
                .collect(),
        ));
    }
    out.push(format!("## cross edges={}", r.merged.cross_edges.len()));
    out.extend(sorted(
        r.merged
            .cross_edges
            .iter()
            .map(|e| edge_line(&names, e))
            .collect(),
    ));
    let mut text = out.join("\n");
    text.push('\n');
    // Longest spelling first, so a root never leaves a tail of a longer one.
    let mut roots: Vec<&String> = roots.iter().collect();
    roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
    for root in roots {
        text = text.replace(root.as_str(), "<root>");
    }
    text
}

/// `CallQualifier` as `(variant index, first string, second string)`: the
/// key `RepoGraph::canonicalize` sorts unresolved sites by (an independent
/// copy, so this test is an oracle for it).
fn qualifier_key(q: &CallQualifier) -> (u8, &str, &str) {
    match q {
        CallQualifier::Bare(n) => (0, n, ""),
        CallQualifier::SelfMethod(n) => (1, n, ""),
        CallQualifier::SuperMethod(n) => (2, n, ""),
        CallQualifier::Attribute { base, name } => (3, base, name),
        CallQualifier::ComplexReceiver { receiver, name } => (4, receiver, name),
    }
}

/// A node as the canonical order reads it: `(qname, kind)` from the graph's
/// nav, `None` (first) for an id it does not name.
fn node_key(g: &RepoGraph, id: NodeId) -> Option<(&str, u32)> {
    let q = g.nav.qname_by_id.get(&id)?;
    Some((
        q.as_str(),
        g.nav.kind_by_id.get(&id).map_or(u32::MAX, |k| k.0),
    ))
}

fn edge_cmp(g: &RepoGraph, a: &Edge, b: &Edge) -> Ordering {
    (node_key(g, a.from), node_key(g, a.to))
        .cmp(&(node_key(g, b.from), node_key(g, b.to)))
        .then_with(|| canonical_edge_cmp(a, b))
}

fn call_cmp(g: &RepoGraph, a: &CallSite, b: &CallSite) -> Ordering {
    (node_key(g, a.from), a.line, qualifier_key(&a.qualifier))
        .cmp(&(node_key(g, b.from), b.line, qualifier_key(&b.qualifier)))
        .then_with(|| a.from.0.cmp(&b.from.0))
}

fn ref_cmp(g: &RepoGraph, a: &UnresolvedRef, b: &UnresolvedRef) -> Ordering {
    (
        node_key(g, a.from),
        node_key(g, a.from_module),
        a.category.0,
        a.line,
        qualifier_key(&a.qualifier),
    )
        .cmp(&(
            node_key(g, b.from),
            node_key(g, b.from_module),
            b.category.0,
            b.line,
            qualifier_key(&b.qualifier),
        ))
        .then_with(|| (a.from.0, a.from_module.0).cmp(&(b.from.0, b.from_module.0)))
}

/// Every graph's edges and unresolved lists in the canonical order (qnames
/// first, then `canonical_edge_cmp` / the ids); the failures, one line each.
fn canonical_violations(tag: &str, graphs: &[RepoGraph]) -> Vec<String> {
    let mut bad = Vec::new();
    for (i, g) in graphs.iter().enumerate() {
        if let Some(j) = g
            .edges
            .windows(2)
            .position(|w| edge_cmp(g, &w[0], &w[1]) == Ordering::Greater)
        {
            bad.push(format!(
                "{tag} graph {i}: edges {j}, {} out of order",
                j + 1
            ));
        }
        if let Some(j) = g
            .unresolved_calls
            .windows(2)
            .position(|w| call_cmp(g, &w[0], &w[1]) == Ordering::Greater)
        {
            bad.push(format!(
                "{tag} graph {i}: unresolved_calls {j}, {} out of order",
                j + 1
            ));
        }
        if let Some(j) = g
            .unresolved_refs
            .windows(2)
            .position(|w| ref_cmp(g, &w[0], &w[1]) == Ordering::Greater)
        {
            bad.push(format!(
                "{tag} graph {i}: unresolved_refs {j}, {} out of order",
                j + 1
            ));
        }
    }
    bad
}

/// Build one corpus entry: `(golden name, its multiset, its order violations)`.
fn corpus_entry(name: &str) -> (String, String, Vec<String>) {
    let fixture = fixtures_root().join(name);
    let dirs: Vec<String> = fixture_dirs(&fixture)
        .iter()
        .map(|d| canonical_path(&fixture.join(d)))
        .collect();
    let r = build_dirs(&dirs);
    let mut roots = dirs.clone();
    roots.push(canonical_path(&fixture));
    let text = multiset(&r, &roots);
    let bad = canonical_violations(name, &r.merged.graphs);
    (name.to_string(), text, bad)
}

fn inline_entry() -> (String, String, Vec<String>) {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("mkdir: {e}"));
    write_inline_fixture(&repo);
    let root = canonical_path(&repo);
    let r = build_dirs(std::slice::from_ref(&root));
    let text = multiset(&r, &[root, repo.to_string_lossy().into_owned()]);
    let bad = canonical_violations("inline", &r.merged.graphs);
    ("inline".to_string(), text, bad)
}

#[test]
fn canonical_order_moves_only_order() {
    let update = std::env::var_os("GLIA_UPDATE_GOLDEN").is_some();
    let golden = golden_dir();
    let mut entries = vec![inline_entry()];
    entries.extend(CANONICAL_CORPUS.iter().map(|name| corpus_entry(name)));
    let mut mismatched = Vec::new();
    let mut violations = Vec::new();
    for (name, text, bad) in entries {
        let path = golden.join(format!("{name}.multiset.txt"));
        if update {
            std::fs::create_dir_all(&golden)
                .unwrap_or_else(|e| panic!("create {}: {e}", golden.display()));
            std::fs::write(&path, &text)
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "read {}: {e} (regenerate with GLIA_UPDATE_GOLDEN=1)",
                path.display()
            )
        });
        if want != text {
            let first = want
                .lines()
                .zip(text.lines())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| want.lines().count().min(text.lines().count()));
            mismatched.push(format!(
                "{name}: first difference at line {} (golden {:?}, built {:?})",
                first + 1,
                want.lines().nth(first),
                text.lines().nth(first)
            ));
        }
        violations.extend(bad);
    }
    assert!(
        mismatched.is_empty(),
        "build content moved (regenerate with GLIA_UPDATE_GOLDEN=1 only when the move is \
         the change's own):\n{}",
        mismatched.join("\n")
    );
    assert!(
        violations.is_empty(),
        "graphs not in canonical order:\n{}",
        violations.join("\n")
    );
}

#[test]
#[ignore = "capture instrument: needs GLIA_CANONICAL_FIXTURES and GLIA_CANONICAL_OUT"]
fn capture_canonical_multisets() {
    let root = PathBuf::from(
        std::env::var("GLIA_CANONICAL_FIXTURES").expect("GLIA_CANONICAL_FIXTURES: fixtures dir"),
    );
    let out = PathBuf::from(
        std::env::var("GLIA_CANONICAL_OUT").expect("GLIA_CANONICAL_OUT: output directory"),
    );
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("create {}: {e}", out.display()));
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read {}: {e}", root.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("key.json").is_file())
        .collect();
    fixtures.sort();
    let mut violations = 0usize;
    for fixture in &fixtures {
        let name = fixture
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dirs: Vec<String> = fixture_dirs(fixture)
            .iter()
            .map(|d| canonical_path(&fixture.join(d)))
            .collect();
        let r = build_dirs(&dirs);
        let mut roots = dirs.clone();
        roots.push(canonical_path(fixture));
        std::fs::write(
            out.join(format!("{name}.multiset.txt")),
            multiset(&r, &roots),
        )
        .unwrap_or_else(|e| panic!("write {name}: {e}"));
        let bad = canonical_violations(&name, &r.merged.graphs);
        violations += bad.len();
        for line in bad {
            eprintln!("[canonical-capture] {line}");
        }
    }
    eprintln!(
        "[canonical-capture] fixtures={} order_violations={violations}",
        fixtures.len()
    );
}
