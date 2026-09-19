//! LD.4a — `trace::cross_stack_trace` answers with ranked distinct paths, a
//! two-node mode, explainable rank keys, and a `cross_service` flag keyed like
//! `glia arch`.
//!
//! The fixture is the packet's three-service build: `web/checkout.ts`
//! `placeOrder` POSTs `/orders`; `api/app.py`'s `create_order` serves it and
//! calls `save -> audit`, `bill` (POSTs `/charge`) and `enqueue`
//! (`producer.send('orders')`); `billing/app.py` serves `/charge`.
//!
//! Before LD.4a the answer was the BFS tree flattened in discovery order (the
//! [`HEAD_HOPS`] before-file): the three routes out of `create_order`
//! interleaved and nothing said which hops formed one path.

use std::path::Path;
use std::time::{Duration, Instant};

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Confidence, Edge, Node, NodeId, RepoId};
use repo_graph_engine::trace::{EXPANSION_BUDGET, TraceAnswer, TraceOptions, cross_stack_trace};
use repo_graph_engine::{generate_many, generate_one};
use repo_graph_graph::{MergedGraph, RepoGraph, SymbolTable};

const CHECKOUT_TS: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";
const API_PY: &str = "import requests\nfrom kafka import KafkaProducer\nfrom flask import Flask\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef bill(order):\n    return requests.post('/charge', json=order)\n\n\ndef enqueue(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    bill(order)\n    enqueue(order)\n    return order\n";
const BILLING_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route('/charge', methods=['POST'])\ndef charge():\n    return {}\n";

/// The before-file: `glia trace web checkout::placeOrder --with api --with
/// billing --depth 8 --json` at the wave HEAD (0b2b391), the pre-LD.4a
/// function, as `(depth, mechanism, cross_service, from, to)`. Its
/// `cross_service` was the repo boundary, now `cross_repo`. (The spec's
/// measurement counted 10 hops; since the marker-anchor pass `enqueue` USES
/// the `queue_producer:orders` it sends on, the 11th.)
const HEAD_HOPS: [(usize, &str, bool, &str, &str); 11] = [
    (
        1,
        "CALLS",
        false,
        "checkout::placeOrder",
        "endpoint:POST:/orders",
    ),
    (
        2,
        "HTTP_CALLS",
        true,
        "endpoint:POST:/orders",
        "POST /orders",
    ),
    (3, "HANDLED_BY", false, "POST /orders", "app::create_order"),
    (4, "CALLS", false, "app::create_order", "app::enqueue"),
    (4, "CALLS", false, "app::create_order", "app::bill"),
    (4, "CALLS", false, "app::create_order", "app::save"),
    (5, "USES", false, "app::enqueue", "queue_producer:orders"),
    (5, "CALLS", false, "app::bill", "endpoint:POST:/charge"),
    (5, "CALLS", false, "app::save", "app::audit"),
    (
        6,
        "HTTP_CALLS",
        true,
        "endpoint:POST:/charge",
        "POST /charge",
    ),
    (7, "HANDLED_BY", false, "POST /charge", "app::charge"),
];

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(p, text).expect("write fixture file");
}

fn write_sources(root: &Path) {
    write(root, "web/checkout.ts", CHECKOUT_TS);
    write(root, "api/app.py", API_PY);
    write(root, "billing/app.py", BILLING_PY);
}

fn dir(root: &Path, sub: &str) -> String {
    root.join(sub).to_string_lossy().into_owned()
}

/// The three dirs as three repos.
fn three_repos() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().expect("tempdir");
    write_sources(td.path());
    let paths = [
        dir(td.path(), "web"),
        dir(td.path(), "api"),
        dir(td.path(), "billing"),
    ];
    let m = generate_many(&paths).expect("generate_many").merged;
    (td, m)
}

fn opts(depth: usize) -> TraceOptions {
    let mut o = TraceOptions::default();
    o.depth = depth;
    o
}

fn opts_to(depth: usize, to: &str) -> TraceOptions {
    let mut o = opts(depth);
    o.to = Some(to.to_string());
    o
}

/// `from -> to` per hop of path `i`.
fn walk(a: &TraceAnswer, i: usize) -> Vec<(&str, &str)> {
    a.paths[i]
        .hops
        .iter()
        .map(|h| (h.from_qname.as_str(), h.to_qname.as_str()))
        .collect()
}

fn last_qname(a: &TraceAnswer, i: usize) -> &str {
    a.paths[i]
        .hops
        .last()
        .map(|h| h.to_qname.as_str())
        .unwrap_or("")
}

#[test]
fn a_feature_answers_three_ranked_paths_and_keeps_the_bfs_tree() {
    let (_td, m) = three_repos();
    let a = cross_stack_trace(&m, "checkout::placeOrder", &opts(8));
    assert_eq!(a.resolved_by, "qname");
    assert_eq!(
        a.seed.as_ref().map(|s| s.qname.as_str()),
        Some("checkout::placeOrder")
    );
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert!(!a.truncated);

    // `hops` is the pre-LD.4a answer, hop for hop, in the same order.
    let hops: Vec<(usize, &str, bool, &str, &str)> = a
        .hops
        .iter()
        .map(|h| {
            (
                h.depth,
                h.mechanism,
                h.cross_repo,
                h.from_qname.as_str(),
                h.to_qname.as_str(),
            )
        })
        .collect();
    assert_eq!(hops, HEAD_HOPS.to_vec());
    // Three repos: the service boundary IS the repo boundary.
    assert!(
        a.hops.iter().all(|h| h.cross_service == h.cross_repo),
        "{:?}",
        a.hops
    );

    let ends: Vec<&str> = (0..a.paths.len()).map(|i| last_qname(&a, i)).collect();
    assert_eq!(
        ends,
        ["app::charge", "queue_producer:orders", "app::audit"],
        "{:#?}",
        a.paths
    );
    assert_eq!(
        a.paths.iter().map(|p| p.rank).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(a.paths.iter().all(|p| p.directed));

    // Rank 1: two service crossings (web -> api -> billing).
    let p = &a.paths[0];
    assert_eq!((p.length, p.cross_service_hops), (7, 2));
    assert_eq!(p.mechanisms, ["CALLS", "HTTP_CALLS", "HANDLED_BY"]);
    assert_eq!(
        walk(&a, 0),
        [
            ("checkout::placeOrder", "endpoint:POST:/orders"),
            ("endpoint:POST:/orders", "POST /orders"),
            ("POST /orders", "app::create_order"),
            ("app::create_order", "app::bill"),
            ("app::bill", "endpoint:POST:/charge"),
            ("endpoint:POST:/charge", "POST /charge"),
            ("POST /charge", "app::charge"),
        ]
    );
    assert_eq!(
        p.hops.iter().map(|h| h.depth).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(
        p.hops
            .iter()
            .filter(|h| h.cross_service)
            .map(|h| h.mechanism)
            .collect::<Vec<_>>(),
        ["HTTP_CALLS", "HTTP_CALLS"]
    );

    // Ranks 2 and 3 tie on one crossing and five hops; the enqueue path
    // shows four mechanisms (USES into the producer) to audit's three.
    let (q, s) = (&a.paths[1], &a.paths[2]);
    assert_eq!((q.length, q.cross_service_hops), (5, 1));
    assert_eq!(q.mechanisms, ["CALLS", "HTTP_CALLS", "HANDLED_BY", "USES"]);
    assert_eq!((s.length, s.cross_service_hops), (5, 1));
    assert_eq!(s.mechanisms, ["CALLS", "HTTP_CALLS", "HANDLED_BY"]);
    assert_eq!(
        walk(&a, 2)[3..],
        [
            ("app::create_order", "app::save"),
            ("app::save", "app::audit")
        ]
    );

    // `max_paths` keeps the head of the same order.
    let mut one = opts(8);
    one.max_paths = 1;
    let top = cross_stack_trace(&m, "checkout::placeOrder", &one);
    assert_eq!(top.paths.len(), 1);
    assert_eq!(top.paths[0], a.paths[0]);
    // ... and `depth` cuts every path (and the tree) at that many hops.
    let short = cross_stack_trace(&m, "checkout::placeOrder", &opts(4));
    assert!(
        short.paths.iter().all(|p| p.length <= 4),
        "{:#?}",
        short.paths
    );
    assert_eq!(
        short.paths.len(),
        3,
        "enqueue / bill / save each end at hop 4"
    );
    assert!(short.hops.iter().all(|h| h.depth <= 4));
}

#[test]
fn two_node_mode_answers_directed_paths_then_an_undirected_fallback() {
    let (_td, m) = three_repos();

    let a = cross_stack_trace(&m, "checkout::placeOrder", &opts_to(6, "app::audit"));
    assert_eq!(
        a.target.as_ref().map(|t| t.qname.as_str()),
        Some("app::audit")
    );
    assert_eq!(a.paths.len(), 1, "{:#?}", a.paths);
    let p = &a.paths[0];
    assert!(p.directed);
    assert_eq!(p.length, 5);
    assert_eq!(
        p.hops.first().map(|h| h.from_qname.as_str()),
        Some("checkout::placeOrder")
    );
    assert_eq!(
        p.hops.last().map(|h| h.to_qname.as_str()),
        Some("app::audit")
    );
    assert!(a.absence.is_none());

    // No carry edge runs from audit back to the client: the answer is the
    // shortest path over any edge, walked either way.
    let b = cross_stack_trace(&m, "app::audit", &opts_to(6, "checkout::placeOrder"));
    assert_eq!(b.paths.len(), 1, "{:#?}", b.paths);
    let p = &b.paths[0];
    assert!(!p.directed);
    assert_eq!(p.length, 5, "{:#?}", p.hops);
    assert_eq!(
        p.hops.first().map(|h| h.from_qname.as_str()),
        Some("app::audit")
    );
    assert_eq!(
        p.hops.last().map(|h| h.to_qname.as_str()),
        Some("checkout::placeOrder")
    );
    for w in p.hops.windows(2) {
        assert_eq!(w[0].to_qname, w[1].from_qname, "the hops chain");
    }

    // Out of reach within the depth: an absence, never an error.
    let c = cross_stack_trace(&m, "app::audit", &opts_to(2, "checkout::placeOrder"));
    assert!(c.paths.is_empty());
    let why = c.absence.expect("an unreachable target is an absence");
    assert_eq!(why.reason, "no_edges");

    // An unknown target names itself in the absence.
    let d = cross_stack_trace(&m, "checkout::placeOrder", &opts_to(6, "no::such::thing"));
    assert!(d.target.is_none() && d.paths.is_empty());
    let why = d.absence.expect("an unknown target is an absence");
    assert_eq!(
        (why.reason, why.query.as_str()),
        ("unknown_symbol", "no::such::thing")
    );
}

#[test]
fn a_manifest_rooted_monorepo_crosses_services_where_glia_arch_does() {
    let td = tempfile::tempdir().expect("tempdir");
    write_sources(td.path());
    write(td.path(), "web/package.json", "{\"name\":\"web\"}\n");
    write(
        td.path(),
        "api/pyproject.toml",
        "[project]\nname = \"api\"\n",
    );
    write(
        td.path(),
        "billing/pyproject.toml",
        "[project]\nname = \"billing\"\n",
    );
    let m = generate_one(&td.path().to_string_lossy())
        .expect("generate_one")
        .merged;

    let a = cross_stack_trace(&m, "placeOrder", &opts(8));
    assert_eq!(a.resolved_by, "name");
    let http: Vec<(&str, bool, bool)> = a
        .hops
        .iter()
        .filter(|h| h.mechanism == "HTTP_CALLS")
        .map(|h| (h.to_qname.as_str(), h.cross_service, h.cross_repo))
        .collect();
    assert_eq!(
        http,
        [
            ("POST /orders @api", true, false),
            ("POST /charge @billing", true, false)
        ],
        "{:#?}",
        a.hops
    );
    assert!(
        a.hops
            .iter()
            .filter(|h| h.mechanism != "HTTP_CALLS")
            .all(|h| !h.cross_service && !h.cross_repo),
        "{:#?}",
        a.hops
    );
    let p = &a.paths[0];
    assert_eq!(
        p.hops.last().map(|h| h.to_qname.as_str()),
        Some("billing::app::charge")
    );
    assert_eq!(p.cross_service_hops, 2);

    // The same links `glia arch` draws on this tree.
    let map = repo_graph_engine::service_map(&m, &Default::default());
    let links: Vec<(&str, &str, &str)> = map
        .links
        .iter()
        .map(|l| (l.from.as_str(), l.to.as_str(), l.mechanism))
        .collect();
    assert!(links.contains(&("web", "api", "HTTP_CALLS")), "{links:?}");
    assert!(
        links.contains(&("api", "billing", "HTTP_CALLS")),
        "{links:?}"
    );
}

#[test]
fn a_dead_end_seed_yields_to_its_entry_flow_else_is_an_absence() {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), "api/app.py", API_PY);
    let m = generate_one(&dir(td.path(), "api"))
        .expect("generate_one")
        .merged;

    // LD.4b: `orders` names the QUEUE_PRODUCER, which no carry edge leaves;
    // the entry flow keyed `post_/orders` contains the word, so the route
    // seeds the trace. (Through LD.4a: resolved_by `name`, the producer as the
    // seed, and a `no_edges` absence.)
    let a = cross_stack_trace(&m, "orders", &TraceOptions::default());
    assert_eq!(a.resolved_by, "entry_flow");
    let seed = a.seed.as_ref().expect("orders resolves");
    assert_eq!((seed.kind, seed.qname.as_str()), ("ROUTE", "POST /orders"));
    assert!(a.absence.is_none(), "{:?}", a.absence);
    assert_eq!(
        a.hops.first().map(|h| (h.mechanism, h.to_qname.as_str())),
        Some(("HANDLED_BY", "app::create_order"))
    );
    let ends: Vec<&str> = (0..a.paths.len()).map(|i| last_qname(&a, i)).collect();
    assert_eq!(
        ends,
        [
            "queue_producer:orders",
            "endpoint:POST:/charge",
            "app::audit"
        ],
        "{:#?}",
        a.paths
    );
    assert!(
        a.paths
            .iter()
            .all(|p| p.hops.first().map(|h| h.from_qname.as_str()) == Some("POST /orders"))
    );

    // A dead end no entry key names is still an absence, never an error.
    let b = cross_stack_trace(&m, "queue_producer:orders", &TraceOptions::default());
    assert_eq!(b.resolved_by, "qname");
    let seed = b.seed.as_ref().expect("the producer resolves");
    assert_eq!(
        (seed.kind, seed.qname.as_str()),
        ("QUEUE_PRODUCER", "queue_producer:orders")
    );
    assert!(b.hops.is_empty() && b.paths.is_empty());
    let why = b.absence.expect("a dead end is an absence");
    assert_eq!(why.reason, "no_edges");
    assert!(why.note.contains("queue_producer:orders"), "{}", why.note);
    assert_eq!(why.mechanisms, ["QUEUE_FLOWS"]);
}

#[test]
fn seeds_resolve_by_qname_name_then_a_naming_find_tier() {
    let (_td, m) = three_repos();
    let by_prefix = cross_stack_trace(&m, "placeOrd", &opts(8));
    assert_eq!(by_prefix.resolved_by, "find");
    assert_eq!(
        by_prefix.seed.as_ref().map(|s| s.qname.as_str()),
        Some("checkout::placeOrder")
    );
    assert_eq!(by_prefix.paths.len(), 3);

    let by_case = cross_stack_trace(&m, "PLACEORDER", &opts(8));
    assert_eq!(by_case.resolved_by, "find");

    // A subsequence-only match never seeds a trace: suggestions instead.
    let none = cross_stack_trace(&m, "plcordr", &opts(8));
    assert_eq!(none.resolved_by, "none");
    assert!(none.seed.is_none() && none.hops.is_empty() && none.paths.is_empty());
    let why = none.absence.expect("an unknown feature is an absence");
    assert_eq!(why.reason, "unknown_symbol");
    assert_eq!(
        why.suggestions.first().map(String::as_str),
        Some("checkout::placeOrder")
    );
}

/// `seed` calls every node of layer 0; every node of layer `i` calls every
/// node of layer `i + 1`: `WIDTH^LAYERS` maximal paths.
fn bipartite(layers: usize, width: usize) -> MergedGraph {
    let r = RepoId::from_canonical("test://trace-budget");
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    let mut mk = |qname: String| {
        let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, &qname);
        let name = qname.rsplit("::").next().unwrap_or(&qname).to_string();
        nav.record(id, &name, &qname, node_kind::FUNCTION, None);
        nodes.push(Node {
            id,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![],
        });
        id
    };
    let seed = mk("m::seed".to_string());
    let grid: Vec<Vec<NodeId>> = (0..layers)
        .map(|l| (0..width).map(|w| mk(format!("m::n{l}_{w}"))).collect())
        .collect();
    let call = |from: NodeId, to: NodeId| Edge {
        from,
        to,
        category: edge_category::CALLS,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    };
    let mut edges: Vec<Edge> = grid[0].iter().map(|&t| call(seed, t)).collect();
    for pair in grid.windows(2) {
        for &f in &pair[0] {
            for &t in &pair[1] {
                edges.push(call(f, t));
            }
        }
    }
    MergedGraph::new(vec![RepoGraph {
        repo: r,
        nodes,
        edges,
        nav,
        symbols: SymbolTable::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }])
}

#[test]
fn path_enumeration_stops_at_its_budget_and_says_so() {
    let m = bipartite(12, 4);
    let t = Instant::now();
    let a = cross_stack_trace(&m, "m::seed", &opts(20));
    let took = t.elapsed();
    assert!(
        a.truncated,
        "4^12 maximal paths exceed the {EXPANSION_BUDGET}-step budget"
    );
    assert_eq!(a.paths.len(), 10, "the default max_paths");
    assert!(
        a.paths.iter().all(|p| p.length == 12),
        "every path found is maximal"
    );
    assert!(took < Duration::from_secs(2), "took {took:?}");

    // The same graph cut at 3 hops is small enough to finish.
    let small = cross_stack_trace(&m, "m::seed", &opts(3));
    assert!(!small.truncated);
    assert!(small.paths.iter().all(|p| p.length == 3));
}
