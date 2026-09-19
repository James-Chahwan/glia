//! LD.6 — one entrypoint set, and `live` on every record that locates nodes.
//!
//! The code domain's entry table (`CODE_TABLES.entry`) is the one entrypoint
//! set: liveness seeds from it, `profile::entry_kinds()` lists it, and the
//! dense-text `*` sigil reads it. It holds every externally-triggered inbound
//! handler kind — before LD.6 it missed QUEUE_CONSUMER, GRAPHQL_RESOLVER,
//! CRON_JOB, GRPC_SERVER and RPC_PROCEDURE, so a queue worker's or a GraphQL
//! resolver's code read as dead.

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use glia_core::{Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId};
use glia_engine::find::{FindOptions, find_nodes};
use glia_engine::profile::{CODE_PROFILE, entry_kinds};
use glia_engine::trace::{TraceOptions, cross_stack_trace};
use glia_engine::{
    entrypoint_reachable, generate_many, governing_docs, resolve_signal_located,
    resolve_signal_located_with_live,
};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

fn repo() -> RepoId {
    RepoId::from_canonical("test://live-records")
}

/// A hand-built graph, as the engine's `answers` unit tests build one: nodes
/// recorded with kind and qname (the simple name is the last `::` segment).
#[derive(Default)]
struct G {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    nav: CodeNav,
}

impl G {
    fn node(&mut self, kind: NodeKindId, qname: &str) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
        let name = qname.rsplit("::").next().unwrap_or(qname);
        self.nav.record(id, name, qname, kind, None);
        self.nodes.push(Node { id, repo: repo(), confidence: Confidence::Strong, cells: vec![] });
        id
    }

    fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
        self.edges.push(Edge { from, to, category, confidence: Confidence::Strong, cells: Vec::new() });
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

/// Each inbound handler kind the entry table lacked seeds liveness: what its
/// HANDLED_BY edge reaches is live, and so is the node itself (a CRON_JOB with
/// no outgoing edge included). A plain FUNCTION nothing reaches stays dead.
#[test]
fn inbound_handler_kinds_are_entrypoints() {
    let mut g = G::default();
    let q = g.node(node_kind::QUEUE_CONSUMER, "queue_consumer:orders");
    let handle = g.node(node_kind::FUNCTION, "worker::handle");
    let ship = g.node(node_kind::FUNCTION, "worker::ship");
    let r = g.node(node_kind::GRAPHQL_RESOLVER, "graphql:Query.user");
    let resolve_user = g.node(node_kind::FUNCTION, "schema::resolve_user");
    let s = g.node(node_kind::GRPC_SERVER, "grpc_server:Greeter");
    let serve = g.node(node_kind::METHOD, "server::Greeter::serve");
    let p = g.node(node_kind::RPC_PROCEDURE, "rpc:eliza.Say");
    let proc_fn = g.node(node_kind::FUNCTION, "server::proc_fn");
    let c = g.node(node_kind::CRON_JOB, "cron:nightly");
    let orphan = g.node(node_kind::FUNCTION, "worker::orphan");
    g.edge(q, handle, edge_category::HANDLED_BY);
    g.edge(handle, ship, edge_category::CALLS);
    g.edge(r, resolve_user, edge_category::HANDLED_BY);
    g.edge(s, serve, edge_category::HANDLED_BY);
    g.edge(p, proc_fn, edge_category::HANDLED_BY);
    let m = g.merged();

    let live = entrypoint_reachable(&m);
    for (what, id) in [
        ("QUEUE_CONSUMER", q),
        ("its handler", handle),
        ("what the handler calls", ship),
        ("GRAPHQL_RESOLVER", r),
        ("its resolver fn", resolve_user),
        ("GRPC_SERVER", s),
        ("its handler method", serve),
        ("RPC_PROCEDURE", p),
        ("its impl fn", proc_fn),
        ("CRON_JOB", c),
    ] {
        assert!(live.contains(&id), "{what} must be live");
    }
    assert!(!live.contains(&orphan), "a function nothing reaches stays dead");
}

/// `entry_kinds()` is the entry table's kinds with their names, in table
/// order: the list the repo-graph wrapper derives its entry set from.
#[test]
fn entry_kinds_matches_profile() {
    let kinds = entry_kinds();
    let ids: Vec<u32> = kinds.iter().map(|(k, _)| k.0).collect();
    assert_eq!(ids, [5, 11, 47, 48, 13, 15, 17, 19, 21, 37, 28]);
    let table: Vec<NodeKindId> = CODE_PROFILE.tables.entry.kinds.to_vec();
    assert_eq!(kinds.iter().map(|(k, _)| *k).collect::<Vec<_>>(), table);
    for (k, name) in &kinds {
        assert_eq!(*name, node_kind::name(*k));
    }
    assert!(kinds.contains(&(node_kind::QUEUE_CONSUMER, "QUEUE_CONSUMER")));
}

const APP_PY: &str = "from flask import Flask\nfrom kafka import KafkaProducer\n\napp = Flask(__name__)\nproducer = KafkaProducer()\n\n\ndef audit(order):\n    return order\n\n\ndef publish(order):\n    producer.send('orders', order)\n\n\ndef save(order):\n    return audit(order)\n\n\n@app.route('/orders', methods=['POST'])\ndef create_order():\n    order = {}\n    save(order)\n    publish(order)\n    return order\n";
const CHECKOUT_TS: &str = "export async function placeOrder(body: unknown) {\n  return fetch('/orders', { method: 'POST', body: JSON.stringify(body) });\n}\n";
/// A doc section naming `create_order`, so `governing_docs` has a row.
const README_MD: &str = "# Orders\n\n## Creating an order\n\n`create_order` saves the order, then publishes it.\n";

/// The packet's two-repo build: a Flask api whose POST /orders route is
/// HANDLED_BY `create_order` (which calls `save` -> `audit` and `publish`), and
/// a TypeScript client that fetches it.
fn orders_stack() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (api, web) = (tmp.path().join("api"), tmp.path().join("web"));
    std::fs::create_dir_all(&api).expect("api dir");
    std::fs::create_dir_all(&web).expect("web dir");
    std::fs::write(api.join("app.py"), APP_PY).expect("write app.py");
    std::fs::write(api.join("README.md"), README_MD).expect("write README.md");
    std::fs::write(web.join("checkout.ts"), CHECKOUT_TS).expect("write checkout.ts");
    let paths = [api.to_string_lossy().into_owned(), web.to_string_lossy().into_owned()];
    let merged = generate_many(&paths).expect("generate_many").merged;
    (tmp, merged)
}

/// `resolve`, `governing_docs`, `find` and `trace` rows carry `live` (a
/// trace hop `to_live`), read off the same walk as blast radius: every
/// function the route's handler reaches is live, the MODULE row is not (no
/// carry edge reaches a module).
#[test]
fn resolve_rows_carry_live() {
    let (_tmp, m) = orders_stack();
    let live = entrypoint_reachable(&m);

    let res = resolve_signal_located(&m, "app.py", "diff", None, None);
    assert!(res.absence.is_none(), "app.py resolves");
    let by_qname = |q: &str| {
        res.results
            .iter()
            .find(|r| r.qname == q)
            .unwrap_or_else(|| panic!("{q} is a resolve row: {:?}", res.results))
    };
    for q in ["app::create_order", "app::save", "app::audit", "app::publish"] {
        let row = by_qname(q);
        assert!(row.live, "{q} is reached from ROUTE POST /orders, so it is live");
        assert_eq!(row.live, live.contains(&NodeId(row.id)));
    }
    let module = by_qname("app");
    assert_eq!(module.kind, "MODULE");
    assert!(!module.live, "no carry edge reaches a module");
    for r in &res.results {
        assert_eq!(r.live, live.contains(&NodeId(r.id)), "{}", r.qname);
        let v = serde_json::to_value(r).expect("serialises");
        assert_eq!(v["live"], serde_json::Value::Bool(r.live), "{v}");
    }
    // The `_with_live` form over the same set is the same answer.
    let again = resolve_signal_located_with_live(&m, &live, "app.py", "diff", None, None);
    let flags = |rows: &[glia_engine::LocatedNode]| -> Vec<(String, bool)> {
        rows.iter().map(|r| (r.qname.clone(), r.live)).collect()
    };
    assert_eq!(flags(&again.results), flags(&res.results));

    let docs = governing_docs(&m, "app::create_order", None);
    assert!(!docs.results.is_empty(), "README section documents create_order: {:?}", docs.absence);
    for d in &docs.results {
        assert_eq!(d.kind, "DOC_SECTION");
        assert_eq!(d.live, live.contains(&NodeId(d.id)), "{}", d.qname);
        let v = serde_json::to_value(d).expect("serialises");
        assert!(v["live"].is_boolean(), "{v}");
    }

    let found = find_nodes(&m, "create_order", &FindOptions::default()).results;
    let top = found.first().expect("find names create_order");
    assert_eq!(top.qname, "app::create_order");
    assert!(top.live, "the route handler is live");
    for r in &found {
        assert_eq!(r.live, live.contains(&NodeId(r.id)), "{}", r.qname);
    }
    let module = find_nodes(&m, "app", &FindOptions::default()).results;
    let module = module.iter().find(|r| r.kind == "MODULE" && r.qname == "app").expect("module app");
    assert!(!module.live);

    let mut opts = TraceOptions::default();
    opts.depth = 4;
    let answer = cross_stack_trace(&m, "app::create_order", &opts);
    assert!(answer.seed.is_some(), "seed resolves");
    let hops = answer.hops;
    assert!(!hops.is_empty());
    for h in &hops {
        let to = find_nodes(&m, &h.to_qname, &FindOptions::default()).results;
        let exact = to.iter().find(|r| r.qname == h.to_qname).expect("hop target is findable");
        assert_eq!(h.to_live, exact.live, "{}", h.to_qname);
    }
    let save = hops.iter().find(|h| h.to_qname == "app::save").expect("create_order -> save hop");
    assert!(save.to_live);
    let v = serde_json::to_value(save).expect("serialises");
    assert_eq!(v["to_live"], serde_json::Value::Bool(true), "{v}");
}
