//! DF.22a: TypeScript parameters and locals type their receivers, end to end
//! through `generate_one`. The parser records a parameter's declared class, a
//! local's annotation / `new T(..)` / `x as T` / `this.<f>` alias on
//! `CodeNav::local_types` (LA.35a), and the graph crate's receiver pass binds
//! `x.m()` on that type: CALLS with EVIDENCE `graph:calls` / `receiver_type`
//! at the call's own row. A name two bindings of one body type differently
//! binds nothing; a parameter shadows a same-named field; a constructor's
//! parameter property keeps A6.2b's bind. The sources are the
//! `ts-param-local-receivers` corpus fixture's.

use std::collections::HashMap;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const API_SERVICE: &str =
    include_str!("../../bench/substrate-gap/fixtures/ts-param-local-receivers/src/api.service.ts");
const ADMIN_SERVICE: &str = include_str!(
    "../../bench/substrate-gap/fixtures/ts-param-local-receivers/src/admin.service.ts"
);

const FIXTURE: [(&str, &str); 4] = [
    (
        "package.json",
        include_str!("../../bench/substrate-gap/fixtures/ts-param-local-receivers/package.json"),
    ),
    ("src/api.service.ts", API_SERVICE),
    ("src/admin.service.ts", ADMIN_SERVICE),
    (
        "src/report.ts",
        include_str!("../../bench/substrate-gap/fixtures/ts-param-local-receivers/src/report.ts"),
    ),
];

const API: &str = "src::api.service::ApiService";
const ADMIN: &str = "src::admin.service::AdminService";

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

/// The EVIDENCE of the CALLS edge `from` -> `to`, if the edge exists.
fn calls_evidence(m: &MergedGraph, from: NodeId, to: NodeId) -> Option<Evidence> {
    let edge = m
        .all_edges()
        .find(|e| e.from == from && e.to == to && e.category == edge_category::CALLS)?;
    Some(Evidence::of(edge).expect("the CALLS edge carries EVIDENCE"))
}

fn has_calls(m: &MergedGraph, from: NodeId, to: NodeId) -> bool {
    calls_evidence(m, from, to).is_some()
}

#[test]
fn fixture_binds_parameter_and_local_receivers() {
    let (_tmp, m) = build(&FIXTURE);
    let nodes = by_qname(&m);
    let method = |q: &str| node(&nodes, q, node_kind::METHOD);
    let load = node(&nodes, "src::report::loadReport", node_kind::FUNCTION);
    let build_m = method("src::report::ReportBuilder::build");
    // (caller, callee, 0-based row of the call in src/report.ts)
    let expected = [
        (load, method(&format!("{API}::fetchUser")), 4),
        (build_m, method(&format!("{API}::purge")), 11),
        (build_m, method(&format!("{ADMIN}::purge")), 12),
        (build_m, method(&format!("{API}::fetchUser")), 13),
        (build_m, method(&format!("{ADMIN}::fetchUser")), 13),
    ];
    for (from, to, row) in expected {
        let ev = calls_evidence(&m, from, to)
            .unwrap_or_else(|| panic!("no CALLS {from:?} -> {to:?} (row {row})"));
        assert_eq!(ev.emitter, "graph:calls", "{ev:?}");
        assert_eq!(ev.rule.as_deref(), Some("receiver_type"), "{ev:?}");
        assert_eq!(ev.basis, Basis::Site, "{ev:?}");
        assert_eq!(ev.file.as_deref(), Some("src/report.ts"), "{ev:?}");
        assert_eq!(ev.line, Some(row), "{ev:?}");
    }
    // `api` is a parameter of `shadow` and of the arrow inside it, typed
    // differently: unknown in the body, so `api.purge()` binds neither.
    let shadow = method("src::report::ReportBuilder::shadow");
    assert!(!has_calls(&m, shadow, method(&format!("{API}::purge"))));
    assert!(!has_calls(&m, shadow, method(&format!("{ADMIN}::purge"))));
    // The parameter's declared type decides over the decoy method.
    assert!(!has_calls(&m, load, method(&format!("{ADMIN}::fetchUser"))));
}

#[test]
fn a_parameter_shadows_a_same_named_field() {
    let p = "import { ApiService } from './api.service';\n\
             import { AdminService } from './admin.service';\n\n\
             export class P {\n  constructor(private api: AdminService) {}\n  run(api: ApiService) {\n    return api.fetchUser(1);\n  }\n}\n";
    let (_tmp, m) = build(&[
        ("package.json", FIXTURE[0].1),
        ("src/api.service.ts", API_SERVICE),
        ("src/admin.service.ts", ADMIN_SERVICE),
        ("src/p.ts", p),
    ]);
    let nodes = by_qname(&m);
    let run = node(&nodes, "src::p::P::run", node_kind::METHOD);
    let api_fetch = node(&nodes, &format!("{API}::fetchUser"), node_kind::METHOD);
    let admin_fetch = node(&nodes, &format!("{ADMIN}::fetchUser"), node_kind::METHOD);
    let ev = calls_evidence(&m, run, api_fetch).expect("P::run CALLS ApiService::fetchUser");
    assert_eq!(ev.rule.as_deref(), Some("receiver_type"), "{ev:?}");
    assert_eq!(ev.line, Some(6), "{ev:?}");
    assert!(
        !has_calls(&m, run, admin_fetch),
        "the parameter's type, not the field's"
    );
}

/// Control (binds on HEAD through A6.2b's field type and the constructor
/// gate): a parameter property's local records the field's own type.
#[test]
fn constructor_parameter_still_binds() {
    let p = "import { ApiService } from './api.service';\n\n\
             export class P {\n  constructor(private api: ApiService) {\n    api.fetchUser(1);\n  }\n}\n";
    let (_tmp, m) = build(&[
        ("package.json", FIXTURE[0].1),
        ("src/api.service.ts", API_SERVICE),
        ("src/admin.service.ts", ADMIN_SERVICE),
        ("src/p.ts", p),
    ]);
    let nodes = by_qname(&m);
    let ctor = node(&nodes, "src::p::P::constructor", node_kind::METHOD);
    let api_fetch = node(&nodes, &format!("{API}::fetchUser"), node_kind::METHOD);
    let ev = calls_evidence(&m, ctor, api_fetch).expect("P::constructor CALLS ApiService::fetchUser");
    assert_eq!(ev.emitter, "graph:calls", "{ev:?}");
    assert_eq!(ev.rule.as_deref(), Some("receiver_type"), "{ev:?}");
    assert_eq!(ev.line, Some(4), "{ev:?}");
}
