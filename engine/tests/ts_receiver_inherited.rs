//! DF.22b: a TypeScript call on a typed receiver whose declared class defines
//! no such method binds the nearest superclass method along INHERITS_FROM,
//! end to end through `generate_one`: CALLS with EVIDENCE `graph:calls` /
//! `inherited_receiver` at the call's own row. The receiver is typed as the
//! receiver pass types it (an injected field, a parameter, a `new` local, a
//! constructed receiver) and behind A6.2a's bare-field gate, so a bare name
//! outside the constructor never borrows a field's type. The fixture sources
//! are the `ts-receiver-inherited-calls` corpus fixture's.

use std::collections::HashMap;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const PACKAGE_JSON: &str =
    include_str!("../../bench/substrate-gap/fixtures/ts-receiver-inherited-calls/package.json");

const FIXTURE: [(&str, &str); 4] = [
    ("package.json", PACKAGE_JSON),
    (
        "src/base-store.ts",
        include_str!(
            "../../bench/substrate-gap/fixtures/ts-receiver-inherited-calls/src/base-store.ts"
        ),
    ),
    (
        "src/user-store.ts",
        include_str!(
            "../../bench/substrate-gap/fixtures/ts-receiver-inherited-calls/src/user-store.ts"
        ),
    ),
    (
        "src/users.service.ts",
        include_str!(
            "../../bench/substrate-gap/fixtures/ts-receiver-inherited-calls/src/users.service.ts"
        ),
    ),
];

const BASE: &str = "src::base-store::BaseStore";
const USERS: &str = "src::users.service::UsersService";

fn build(files: &[(&str, &str)]) -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    for &(rel, src) in files {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, src).unwrap();
    }
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    (tmp, merged)
}

/// qname -> (id, kind) over every graph.
fn by_qname(m: &MergedGraph) -> HashMap<String, (NodeId, NodeKindId)> {
    let mut out = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let (Some(q), Some(k)) = (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id))
            else {
                continue;
            };
            out.entry(q.clone()).or_insert((n.id, *k));
        }
    }
    out
}

fn node(nodes: &HashMap<String, (NodeId, NodeKindId)>, qname: &str, kind: NodeKindId) -> NodeId {
    match nodes.get(qname) {
        Some((id, k)) if *k == kind => *id,
        other => panic!(
            "no {kind:?} {qname}: {other:?}; qnames: {:?}",
            nodes.keys().collect::<Vec<_>>()
        ),
    }
}

/// The EVIDENCE of every CALLS edge `from` -> `to`, in edge order.
fn calls_evidence(m: &MergedGraph, from: NodeId, to: NodeId) -> Vec<Evidence> {
    m.all_edges()
        .filter(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)
        .map(|e| Evidence::of(e).expect("the CALLS edge carries EVIDENCE"))
        .collect()
}

/// Asserts one CALLS `from` -> `to`, EVIDENCE `graph:calls` / `rule`, a site
/// in `file` at 0-based `row`.
fn assert_call(m: &MergedGraph, from: NodeId, to: NodeId, rule: &str, file: &str, row: u32) {
    let evs = calls_evidence(m, from, to);
    let [ev] = evs.as_slice() else {
        panic!("want one CALLS {from:?} -> {to:?} (row {row}), got {evs:?}");
    };
    assert_eq!(ev.emitter, "graph:calls", "{ev:?}");
    assert_eq!(ev.rule.as_deref(), Some(rule), "{ev:?}");
    assert_eq!(ev.basis, Basis::Site, "{ev:?}");
    assert_eq!(ev.file.as_deref(), Some(file), "{ev:?}");
    assert_eq!(ev.line, Some(row), "{ev:?}");
}

#[test]
fn fixture_receiver_calls_bind_the_nearest_superclass() {
    let (_tmp, m) = build(&FIXTURE);
    let nodes = by_qname(&m);
    let method = |q: &str| node(&nodes, q, node_kind::METHOD);
    let (save, load) = (method(&format!("{BASE}::save")), method(&format!("{BASE}::load")));
    let on_requested = method(&format!("{USERS}::onRequested"));
    let sync = method(&format!("{USERS}::sync"));
    let rebuild = method(&format!("{USERS}::rebuild"));
    let svc = "src/users.service.ts";
    // An injected field: `this.store.save(id)` / `this.store.load(id)`.
    assert_call(&m, on_requested, save, "inherited_receiver", svc, 14);
    assert_call(&m, on_requested, load, "inherited_receiver", svc, 15);
    // A parameter: the own method binds in the receiver pass, the inherited
    // one here.
    let find = method("src::user-store::UserStore::find");
    assert_call(&m, sync, find, "receiver_type", svc, 19);
    assert_call(&m, sync, load, "inherited_receiver", svc, 19);
    // A `new` local (`fresh.save(id)`) and a constructed receiver
    // (`new UserStore().load(id)`).
    assert_call(&m, rebuild, save, "inherited_receiver", svc, 24);
    assert_call(&m, rebuild, load, "inherited_receiver", svc, 25);
    // CH.1b's `this.load()` keeps its rule.
    assert_call(&m, find, load, "inherited_method", "src/user-store.ts", 4);
    // `repo.load(id)` in `stale` reads the module-level `repo`: a bare name
    // outside the constructor is not the field `repo: UserStore`.
    let stale = method(&format!("{USERS}::stale"));
    assert!(calls_evidence(&m, stale, load).is_empty(), "A6.2a's gate keeps `repo` unbound");
}

#[test]
fn two_level_parameter_receiver() {
    let src = "export class W { a(): void {} }\n\
               export class S extends W {}\n\
               export class E extends S {}\n\
               export function t(e: E) { e.a(); }\n";
    let (_tmp, m) = build(&[("package.json", PACKAGE_JSON), ("src/chain.ts", src)]);
    let nodes = by_qname(&m);
    let t = node(&nodes, "src::chain::t", node_kind::FUNCTION);
    let a = node(&nodes, "src::chain::W::a", node_kind::METHOD);
    assert_call(&m, t, a, "inherited_receiver", "src/chain.ts", 3);
}
