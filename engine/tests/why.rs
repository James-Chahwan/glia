//! LE.5 — `why(edge)`: for two nodes, every edge between them with the
//! extractor / resolver that emitted it, its call site and confidence, tiered
//! fact / derived / heuristic from the emitter's stage; a located witness path
//! over the carry edges when there is no direct edge. Before this the graph
//! held the evidence (LC.3a-d) but nothing read it back: `glia why` was an
//! unrecognized subcommand and `glia analyze --format json` edges carried
//! only `{category, from, intra, to}`.

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, RepoId};
use glia_engine::why::{EdgeWhy, WhyAnswer, why_edge};
use glia_engine::{generate_many, generate_one};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

const SHOP: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n";

const CHECKOUT: &str = "def price(o):\n    return o\n\n\ndef place(o):\n    return price(o)\n\n\ndef checkout(o):\n    return place(o)\n";

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (name, src) in files {
        let path = tmp.path().join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        std::fs::write(path, src).expect("write source");
    }
    let merged = generate_one(tmp.path().to_str().expect("utf-8 temp path"))
        .expect("generate_one")
        .merged;
    (tmp, merged)
}

fn why(m: &MergedGraph, from: &str, to: &str, category: Option<&str>) -> WhyAnswer {
    why_edge(m, from, to, category).expect("why answers")
}

/// `(category, tier, emitter, site file, site line)` of each row.
fn rows(rs: &[EdgeWhy]) -> Vec<(&str, &str, Option<&str>, Option<&str>, Option<i64>)> {
    rs.iter()
        .map(|r| {
            (
                r.category,
                r.tier,
                r.emitter.as_deref(),
                r.site.as_ref().map(|s| s.file.as_str()),
                r.site.as_ref().and_then(|s| s.line),
            )
        })
        .collect()
}

/// (1) A resolved call is a FACT read at its site: the 1-based line of the
/// call, not of the caller's `def`.
#[test]
fn call_edge_is_fact_with_site() {
    let (_tmp, m) = build(&[("shop/a.py", SHOP)]);
    let a = why(&m, "shop::a::place", "shop::a::price", None);
    assert!(a.found, "{a:?}");
    assert!(a.absence.is_none() && a.path.is_empty(), "{a:?}");
    assert_eq!(a.edges.len(), 1, "{a:?}");
    let r = &a.edges[0];
    assert_eq!(
        (r.category, r.tier, r.confidence),
        ("CALLS", "fact", "strong"),
        "{r:?}"
    );
    let emitter = r.emitter.as_deref().unwrap_or("");
    assert!(
        emitter.starts_with("graph:") || emitter.starts_with("parser:"),
        "{r:?}"
    );
    assert_eq!(r.basis, Some("site"), "{r:?}");
    let site = r.site.as_ref().expect("a call site");
    assert_eq!(
        (site.file.as_str(), site.line),
        ("shop/a.py", Some(6)),
        "{r:?}"
    );
    assert_eq!(
        (r.from_qname.as_str(), r.to_qname.as_str()),
        ("shop::a::place", "shop::a::price")
    );
    assert!(!r.cross_repo, "{r:?}");
    assert!(r.note.is_none(), "{r:?}");

    // A dotted Python path names the same nodes; a category filter keeps it.
    let dotted = why(&m, "shop.a.place", "shop.a.price", Some("CALLS"));
    assert_eq!(rows(&dotted.edges), rows(&a.edges));
    // Filtering on a category the pair does not share finds no direct edge.
    let other = why(&m, "shop::a::place", "shop::a::price", Some("IMPORTS"));
    assert!(!other.found && other.edges.is_empty(), "{other:?}");
    let ab = other.absence.as_ref().expect("absence when not found");
    assert_eq!((ab.tier, ab.reason), ("FACT", "no_edges"), "{ab:?}");
    assert_eq!(ab.mechanisms, ["IMPORTS"], "{ab:?}");
}

/// (2) An HTTP edge a resolver paired by path is DERIVED, never FACT, and
/// names the resolver and its match tier.
#[test]
fn http_edge_is_derived() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let api = tmp.path().join("api");
    let web = tmp.path().join("web");
    std::fs::create_dir_all(&api).expect("mkdir api");
    std::fs::create_dir_all(&web).expect("mkdir web");
    std::fs::write(
        api.join("app.py"),
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get('/users')\ndef list_users():\n    return []\n",
    )
    .expect("write api");
    std::fs::write(
        web.join("client.py"),
        "import requests\n\n\ndef load_users():\n    return requests.get(\"http://localhost:5000/users\")\n",
    )
    .expect("write web");
    let paths = [
        api.to_str().expect("utf-8").to_string(),
        web.to_str().expect("utf-8").to_string(),
    ];
    let m = generate_many(&paths).expect("generate_many").merged;

    let a = why(&m, "endpoint:GET:/users", "GET /users", None);
    assert!(a.found, "{a:?}");
    let http: Vec<&EdgeWhy> = a
        .edges
        .iter()
        .filter(|r| r.category == "HTTP_CALLS")
        .collect();
    assert_eq!(http.len(), 1, "{a:?}");
    let r = http[0];
    assert_eq!(r.tier, "derived", "{r:?}");
    assert_eq!(r.emitter.as_deref(), Some("resolver:http"), "{r:?}");
    assert_eq!(r.rule.as_deref(), Some("exact"), "{r:?}");
    assert!(
        r.cross_repo,
        "the web client and the api are two repos: {r:?}"
    );
}

/// (3) A name two functions share resolves to both: one row per file.
#[test]
fn same_name_two_nodes_reports_both() {
    let src = "def helper(o):\n    return o\n\n\ndef handle(o):\n    return helper(o)\n";
    let (_tmp, m) = build(&[("a.py", src), ("b.py", src)]);
    let a = why(&m, "handle", "helper", None);
    assert!(a.found, "{a:?}");
    let got = rows(&a.edges);
    assert_eq!(got.len(), 2, "{a:?}");
    let mut files: Vec<(Option<&str>, Option<i64>)> = got.iter().map(|r| (r.3, r.4)).collect();
    files.sort();
    assert_eq!(
        files,
        [(Some("a.py"), Some(6)), (Some("b.py"), Some(6))],
        "{a:?}"
    );
    assert!(
        a.edges
            .iter()
            .all(|r| r.category == "CALLS" && r.tier == "fact"),
        "{a:?}"
    );
    // Each row pairs a handle with its own file's helper.
    for r in &a.edges {
        let file = r.from_qname.split("::").next().unwrap_or("");
        assert!(r.to_qname.starts_with(&format!("{file}::")), "{r:?}");
    }
}

/// (4) No direct edge: found is false, the absence says so, and the witness
/// path explains each hop like a row.
#[test]
fn no_direct_edge_gives_path() {
    let (_tmp, m) = build(&[("shop/a.py", CHECKOUT)]);
    let a = why(&m, "shop::a::checkout", "shop::a::price", None);
    assert!(!a.found, "{a:?}");
    assert!(a.edges.is_empty(), "{a:?}");
    let hops: Vec<(&str, &str, &str, &str)> = a
        .path
        .iter()
        .map(|r| {
            (
                r.from_qname.as_str(),
                r.to_qname.as_str(),
                r.category,
                r.tier,
            )
        })
        .collect();
    assert_eq!(
        hops,
        [
            ("shop::a::checkout", "shop::a::place", "CALLS", "fact"),
            ("shop::a::place", "shop::a::price", "CALLS", "fact"),
        ],
        "{a:?}"
    );
    let lines: Vec<Option<i64>> = a
        .path
        .iter()
        .map(|r| r.site.as_ref().and_then(|s| s.line))
        .collect();
    assert_eq!(lines, [Some(10), Some(6)], "{a:?}");
    let ab = a.absence.as_ref().expect("absence when not found");
    assert_eq!((ab.tier, ab.reason), ("FACT", "no_edges"), "{ab:?}");
    assert!(
        ab.mechanisms.contains(&"CALLS"),
        "every carry category: {ab:?}"
    );

    // The reverse question has no forward path; the note says an edge runs
    // the other way.
    let back = why(&m, "shop::a::price", "shop::a::place", None);
    assert!(!back.found && back.path.is_empty(), "{back:?}");
    let note = back.note.as_deref().unwrap_or("");
    assert!(note.contains("not connected within 6 carry hops"), "{note}");
    assert!(note.contains("1 edge runs the other way"), "{note}");
}

/// (5) An endpoint that names no node, or an unknown category, is an error.
#[test]
fn unknown_qname_is_err() {
    let (_tmp, m) = build(&[("shop/a.py", SHOP)]);
    let err = why_edge(&m, "shop::a::nope", "shop::a::price", None).expect_err("unknown from");
    assert!(
        err.contains("no node with qname/name `shop::a::nope`"),
        "{err}"
    );
    let err = why_edge(&m, "shop::a::place", "pric", None).expect_err("unknown to");
    assert!(err.contains("no node with qname/name `pric`"), "{err}");
    assert!(
        err.contains("shop::a::price"),
        "suggests the nearest qname: {err}"
    );
}

#[test]
fn unknown_category_is_err() {
    let (_tmp, m) = build(&[("shop/a.py", SHOP)]);
    let err = why_edge(&m, "shop::a::place", "shop::a::price", Some("CALS"))
        .expect_err("unknown category");
    assert!(err.starts_with("unknown edge category `CALS`"), "{err}");
    assert!(
        err.contains("CALLS") && err.contains("HTTP_CALLS"),
        "lists the valid names: {err}"
    );
    // Case-insensitive on the way in, the registry spelling on the way out.
    let a = why(&m, "shop::a::place", "shop::a::price", Some("calls"));
    assert_eq!(a.edges.len(), 1, "{a:?}");
}

fn repo() -> RepoId {
    RepoId::from_canonical("test://why")
}

/// A hand-built graph of FUNCTION nodes, as the engine's other tests build one.
#[derive(Default)]
struct G {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    nav: CodeNav,
}

impl G {
    fn node(&mut self, qname: &str) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname);
        let name = qname.rsplit("::").next().unwrap_or(qname);
        self.nav.record(id, name, qname, node_kind::FUNCTION, None);
        self.nodes.push(Node {
            id,
            repo: repo(),
            confidence: Confidence::Strong,
            cells: vec![],
        });
        id
    }

    fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId, cells: Vec<Cell>) {
        self.edges.push(Edge {
            from,
            to,
            category,
            confidence: Confidence::Weak,
            cells,
        });
    }

    fn merged(self) -> MergedGraph {
        MergedGraph::new(vec![RepoGraph {
            repo: repo(),
            nodes: self.nodes,
            edges: self.edges,
            nav: self.nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }])
    }
}

/// (6) An edge with no EVIDENCE cell (a .gmap from before LC.3a) is never
/// shown as a fact.
#[test]
fn edge_without_evidence_is_derived_with_note() {
    let mut g = G::default();
    let a = g.node("m::a");
    let b = g.node("m::b");
    g.edge(a, b, edge_category::CALLS, vec![]);
    let m = g.merged();
    let w = why(&m, "m::a", "m::b", None);
    assert!(w.found, "{w:?}");
    let r = &w.edges[0];
    assert_eq!(
        (r.tier, r.emitter.as_deref(), r.basis),
        ("derived", None, None),
        "{r:?}"
    );
    assert!(r.site.is_none(), "{r:?}");
    assert_eq!(r.note.as_deref(), Some("no evidence recorded"), "{r:?}");
    assert_eq!(r.confidence, "weak", "{r:?}");
}

/// The stage map: name-only graph guesses (LC.3d) and the overlay / history
/// stages are heuristic; a stage outside the documented vocabulary is derived
/// with a note, never fact; a located-less graph edge drops to derived. The
/// overlay row says where it was declared and by whom (LF.2b).
#[test]
fn stage_map_tiers_every_emitter() {
    let mut g = G::default();
    let a = g.node("m::a");
    let b = g.node("m::b");
    let ev = |e: Evidence| vec![e.to_cell()];
    g.edge(
        a,
        b,
        edge_category::CALLS,
        ev(Evidence::emitter("graph:refs")
            .rule("global_unique")
            .at("m.py", 3)),
    );
    g.edge(
        a,
        b,
        edge_category::CALLS,
        ev(Evidence::emitter("graph:calls").rule("import_binding")),
    );
    g.edge(
        a,
        b,
        edge_category::HTTP_CALLS,
        ev(Evidence::emitter("pass:tests").at("m.py", 1)),
    );
    let mut overlay = ev(Evidence::emitter("overlay:edge")
        .rule("edge#1")
        .at(".glia/overlay.toml", 4));
    overlay.push(Cell {
        kind: cell_type::ORIGIN,
        payload: CellPayload::Json(r#"{"provenance":"overlay:human","rule":"edge#1"}"#.to_string()),
    });
    g.edge(a, b, edge_category::CALLS, overlay);
    g.edge(
        a,
        b,
        edge_category::CO_CHANGES,
        ev(Evidence::emitter("history:cochange")),
    );
    g.edge(
        a,
        b,
        edge_category::CALLS,
        ev(Evidence::emitter("oracle:guess").at("m.py", 2)),
    );
    let m = g.merged();
    let w = why(&m, "m::a", "m::b", None);
    let got: Vec<(&str, Option<&str>, Option<&str>)> = w
        .edges
        .iter()
        .map(|r| (r.tier, r.emitter.as_deref(), r.note.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            // Unlocated rows first, then by (file, line).
            ("derived", Some("graph:calls"), Some("no location recorded")),
            (
                "heuristic",
                Some("history:cochange"),
                Some("files change together in git history; not a code reference")
            ),
            (
                "heuristic",
                Some("overlay:edge"),
                Some("declared in .glia/overlay.toml:5 by overlay:human")
            ),
            ("derived", Some("pass:tests"), None),
            (
                "derived",
                Some("oracle:guess"),
                Some("unknown emitter stage `oracle`")
            ),
            (
                "heuristic",
                Some("graph:refs"),
                Some("name-only binding (global_unique)")
            ),
        ],
        "{w:?}"
    );
    // One category narrows the rows to it.
    let only = why(&m, "m::a", "m::b", Some("CO_CHANGES"));
    assert_eq!(only.edges.len(), 1, "{only:?}");
}

/// Two call sites of one callee are two rows (LC.2 keeps the multiplicity),
/// each at its own line, in line order.
#[test]
fn two_call_sites_are_two_rows() {
    let src =
        "def price(o):\n    return o\n\n\ndef place(o):\n    a = price(o)\n    return price(a)\n";
    let (_tmp, m) = build(&[("shop/a.py", src)]);
    let a = why(&m, "shop::a::place", "shop::a::price", None);
    let lines: Vec<Option<i64>> = a
        .edges
        .iter()
        .map(|r| r.site.as_ref().and_then(|s| s.line))
        .collect();
    assert_eq!(lines, [Some(6), Some(7)], "{a:?}");
}

/// CC.3: a graph-stage edge the graph crate INFERRED below Strong confidence
/// is derived, not fact. Go implicit interface satisfaction is inferred from
/// the method-name set (`graph:iface` rule `method_set`, Medium); the
/// method-level pair it rests on is bound by name and signature at Strong
/// (`same_name`) and stays a fact. The rule reads confidence, never a list
/// of rule names.
#[test]
fn medium_graph_edge_is_derived() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/go-implicit-iface");
    let files: Vec<(String, String)> = ["go.mod", "store.go", "mem.go"]
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(fixture.join(f)).expect("read fixture file");
            ((*f).to_string(), src)
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(f, s)| (f.as_str(), s.as_str()))
        .collect();
    let (_tmp, m) = build(&refs);

    let a = why(&m, "mem::MemStore", "store::Store", None);
    assert_eq!(a.edges.len(), 1, "{a:?}");
    let r = &a.edges[0];
    assert_eq!(
        (
            r.category,
            r.tier,
            r.confidence,
            r.emitter.as_deref(),
            r.rule.as_deref()
        ),
        (
            "IMPLEMENTS",
            "derived",
            "medium",
            Some("graph:iface"),
            Some("method_set")
        ),
        "{r:?}"
    );
    let note = r.note.as_deref().unwrap_or("");
    assert!(
        note.contains("inferred binding (method_set, medium confidence)"),
        "{r:?}"
    );

    let m_get = why(&m, "mem::MemStore::Get", "store::Store::Get", None);
    assert_eq!(m_get.edges.len(), 1, "{m_get:?}");
    let r = &m_get.edges[0];
    assert_eq!(
        (r.category, r.tier, r.confidence, r.rule.as_deref()),
        ("IMPLEMENTS", "fact", "strong", Some("same_name")),
        "{r:?}"
    );
    assert!(r.note.is_none(), "{r:?}");
}
