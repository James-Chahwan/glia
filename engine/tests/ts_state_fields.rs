//! CH.2: a TypeScript class field initialised by a call (`readonly countries
//! = toSignal(this.countriesApi.list())`, `total = computed(() => …)`) is a
//! STATE_VAR of its class, end to end through `generate_one`: a call in its
//! initializer binds across files through the `inject()` field's type (A6.2a /
//! A6.2b, the graph crate walking STATE_VAR -> CLASS), every CALLS edge out of
//! a STATE_VAR carries EVIDENCE, and the class takes none of the
//! initializers' calls. The sources are the `ts-signal-state-fields` corpus
//! fixture's.

use std::collections::HashMap;
use std::path::Path;

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: [(&str, &str); 3] = [
    (
        "package.json",
        include_str!("../../bench/substrate-gap/fixtures/ts-signal-state-fields/package.json"),
    ),
    (
        "src/countries.service.ts",
        include_str!(
            "../../bench/substrate-gap/fixtures/ts-signal-state-fields/src/countries.service.ts"
        ),
    ),
    (
        "src/trade.component.ts",
        include_str!(
            "../../bench/substrate-gap/fixtures/ts-signal-state-fields/src/trade.component.ts"
        ),
    ),
];

const TRADE: &str = "src::trade.component::TradeComponent";

fn build() -> (tempfile::TempDir, std::path::PathBuf, MergedGraph) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    for (rel, src) in FIXTURE {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, src).unwrap();
    }
    let merged = generate_one(repo.to_str().unwrap()).unwrap().merged;
    (tmp, repo, merged)
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

fn source_line(repo: &Path, file: &str, line: u32) -> String {
    let text = std::fs::read_to_string(repo.join(file)).unwrap();
    text.lines()
        .nth(line as usize)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn state_var_call_binds_across_files_on_the_inject_field_type() {
    let (_tmp, repo, m) = build();
    let nodes = by_qname(&m);
    let countries = node(&nodes, &format!("{TRADE}::countries"), node_kind::STATE_VAR);
    let list = node(
        &nodes,
        "src::countries.service::CountriesService::list",
        node_kind::METHOD,
    );
    let edge = m
        .all_edges()
        .find(|e| e.from == countries && e.to == list && e.category == edge_category::CALLS)
        .expect("countries CALLS CountriesService::list");
    let ev = Evidence::of(edge).expect("the CALLS edge carries EVIDENCE");
    assert_eq!(ev.emitter, "graph:calls", "{ev:?}");
    assert_eq!(ev.rule.as_deref(), Some("receiver_type"), "{ev:?}");
    assert_eq!(ev.basis, Basis::Site, "{ev:?}");
    let (Some(file), Some(line)) = (ev.file.clone(), ev.line) else {
        panic!("site evidence without a file and line: {ev:?}");
    };
    assert_eq!(file, "src/trade.component.ts");
    let text = source_line(&repo, &file, line);
    assert!(
        text.contains("this.countriesApi.list()"),
        "{file}:{line} is `{}`",
        text.trim()
    );
}

#[test]
fn every_state_var_call_carries_evidence() {
    let (_tmp, _repo, m) = build();
    let nodes = by_qname(&m);
    let state: Vec<(&String, NodeId)> = nodes
        .iter()
        .filter(|(_, (_, k))| *k == node_kind::STATE_VAR)
        .map(|(q, (id, _))| (q, *id))
        .collect();
    let mut names: Vec<&str> = state
        .iter()
        .filter_map(|(q, _)| q.strip_prefix(&format!("{TRADE}::")))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "count$",
            "countries",
            "isLong",
            "logTotal",
            "page",
            "profile",
            "rows",
            "total"
        ],
        "the call-initialised fields, and no inject() / new / literal field"
    );
    let mut calls = 0;
    for (q, id) in &state {
        for e in m
            .all_edges()
            .filter(|e| e.from == *id && e.category == edge_category::CALLS)
        {
            let ev = Evidence::of(e).unwrap_or_else(|| panic!("{q} CALLS without EVIDENCE"));
            assert!(ev.line.is_some(), "{q}: {ev:?}");
            calls += 1;
        }
    }
    // total -> rows, page; isLong -> longest, rows; countries -> list;
    // profile -> GET /api/profile; logTotal -> log, total.
    assert_eq!(calls, 8, "CALLS edges out of the STATE_VARs");
}

#[test]
fn the_class_takes_none_of_the_initializers_calls() {
    let (_tmp, _repo, m) = build();
    let nodes = by_qname(&m);
    let class = node(&nodes, TRADE, node_kind::CLASS);
    let out: Vec<_> = m
        .all_edges()
        .filter(|e| e.from == class && e.category == edge_category::CALLS)
        .collect();
    assert!(out.is_empty(), "the class CALLS {out:?}");
    let total = node(&nodes, &format!("{TRADE}::total"), node_kind::STATE_VAR);
    assert!(
        m.all_edges()
            .any(|e| e.from == class && e.to == total && e.category == edge_category::DEFINES),
        "the class DEFINES its STATE_VAR"
    );
}
