//! GB.1 — `trace::concept_flows`: an entry flow matches a word through what it
//! TOUCHES (entry name, hop names and qname words, node kinds, mechanisms,
//! data entities, external hosts, doc headings), ranked by match tier then the
//! matching hop's depth, the matching hop shown.
//!
//! Two fixtures:
//!
//! - `shop`, a real build of one Flask service: `POST /orders` is handled by
//!   `create_order`, which calls `place_order` (a SQL INSERT into
//!   `order_lines`) and `notify` (POSTs an external https host);
//!   `GET /orders/<oid>/comments` reaches the `issue_comments` table;
//!   `GET /health` touches nothing; the README's `## Caching` section
//!   documents `place_order()`.
//! - `cached`, a hand-built graph for the shape a build does not anchor yet: a
//!   data source (the CACHE `data_source:redis`) read over ACCESSES_DATA by
//!   the method a route reaches. Today's extractors tie a data source to the
//!   MODULE that names it, so no real flow reaches a CACHE; the tiers are
//!   graph-level and this pins them.
//!
//! Before GB.1 a flow was found by its entry's name alone (LD.4b's keys):
//! `cache` or `IssueComment` named no flow.

use std::path::Path;

use glia_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use glia_core::{Confidence, Edge, Node, NodeId, NodeKindId, RepoId};
use glia_engine::trace::{ConceptHit, ConceptOptions, ConceptTier, concept_flows, entry_flows};
use glia_engine::{GenerateResult, generate_one};
use glia_graph::{MergedGraph, RepoGraph, SymbolTable};

pub(crate) const APP_PY: &str = "import sqlite3

import requests
from flask import Flask

app = Flask(__name__)


def place_order(order):
    db = sqlite3.connect('shop.db')
    db.execute(\"INSERT INTO order_lines (body) VALUES (?)\", (order,))
    return order


def list_comments(order):
    db = sqlite3.connect('shop.db')
    return db.execute(\"SELECT body FROM issue_comments WHERE id = ?\", (order,))


def notify(order):
    return requests.post('https://hooks.example-chat.com/services/orders', json=order)


@app.route('/orders', methods=['POST'])
def create_order():
    order = {}
    place_order(order)
    notify(order)
    return order


@app.route('/orders/<oid>/comments')
def order_comments(oid):
    return list_comments(oid)


@app.route('/health')
def health():
    return 'ok'
";

pub(crate) const README_MD: &str = "# Shop

## Caching

Every order is kept by `place_order()` before it is confirmed.
";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
    std::fs::write(p, text).expect("write fixture file");
}

/// The `shop` build (module doc).
pub(crate) fn shop() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().join("shop");
    write(&root, "app.py", APP_PY);
    write(&root, "README.md", README_MD);
    let r = generate_one(&root.to_string_lossy()).expect("generate_one");
    (td, r)
}

/// The `cached` graph (module doc): `POST /orders` -HANDLED_BY->
/// `svc::create_order` -CALLS-> `svc::OrderService::place` -ACCESSES_DATA->
/// CACHE `data_source:redis`; `GET /health` -HANDLED_BY-> `svc::health`.
pub(crate) fn cached() -> MergedGraph {
    let r = RepoId::from_canonical("test://concept-cache");
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    let mut mk = |kind: NodeKindId, name: &str, qname: &str| {
        let id = NodeId::from_parts(GRAPH_TYPE, r, kind, qname);
        nav.record(id, name, qname, kind, None);
        nodes.push(Node {
            id,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![],
        });
        id
    };
    let orders = mk(node_kind::ROUTE, "POST /orders", "POST /orders");
    let create = mk(node_kind::FUNCTION, "create_order", "svc::create_order");
    let place = mk(node_kind::METHOD, "place", "svc::OrderService::place");
    let redis = mk(node_kind::CACHE, "redis", "data_source:redis");
    let health = mk(node_kind::ROUTE, "GET /health", "GET /health");
    let health_fn = mk(node_kind::FUNCTION, "health", "svc::health");
    let edge = |from: NodeId, to: NodeId, category| Edge {
        from,
        to,
        category,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    };
    let edges = vec![
        edge(orders, create, edge_category::HANDLED_BY),
        edge(create, place, edge_category::CALLS),
        edge(place, redis, edge_category::ACCESSES_DATA),
        edge(health, health_fn, edge_category::HANDLED_BY),
    ];
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

fn ask(r: &GenerateResult, word: &str) -> Vec<ConceptHit> {
    let a = concept_flows(&r.merged, &r.repo_labels, word, &ConceptOptions::default());
    assert_eq!(a.absence.is_some(), a.results.is_empty());
    a.results
}

fn ask_cached(m: &MergedGraph, word: &str, depth: usize) -> Vec<ConceptHit> {
    let mut opts = ConceptOptions::default();
    opts.depth = depth;
    concept_flows(m, &Default::default(), word, &opts).results
}

fn hit<'a>(hits: &'a [ConceptHit], key: &str) -> &'a ConceptHit {
    hits.iter()
        .find(|h| h.key == key)
        .unwrap_or_else(|| panic!("no `{key}` hit in {hits:#?}"))
}

#[test]
fn cache_finds_the_flow_through_its_cache_kind() {
    let m = cached();
    // The before-state: no entry flow's key names `cache`.
    let flows = entry_flows(&m, &Default::default(), 6);
    assert!(flows.iter().all(|f| !f.key.contains("cache")), "{flows:#?}");
    let cache_hop = flows
        .iter()
        .find(|f| f.key == "post_/orders")
        .and_then(|f| f.hops.iter().find(|h| h.to_kind == "CACHE"))
        .unwrap_or_else(|| panic!("the orders flow reaches a CACHE: {flows:#?}"));

    let hits = ask_cached(&m, "cache", 6);
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let h = &hits[0];
    assert_eq!((h.rank, h.key.as_str()), (1, "post_/orders"));
    assert_eq!(h.tier, ConceptTier::Kind);
    assert_eq!(h.matched, ["cache -> kind CACHE"]);
    assert_eq!(h.hop.as_ref(), Some(cache_hop));
    assert_eq!((h.depth, cache_hop.depth), (3, 3));
    assert_eq!((h.entry.kind, h.reach), ("ROUTE", 3));
    // Plurals match by a trailing `s` / `es`.
    let plural = ask_cached(&m, "caches", 6);
    assert_eq!(plural[0].matched, ["caches -> kind CACHE"]);
    assert_eq!(
        (plural.len(), plural[0].tier, &plural[0].hop),
        (1, h.tier, &h.hop)
    );
    // `orders cache` needs both words: the weaker tier speaks, the deeper
    // match shows its hop.
    let both = ask_cached(&m, "orders cache", 6);
    assert_eq!(both.len(), 1, "{both:#?}");
    assert_eq!(both[0].tier, ConceptTier::Kind);
    assert_eq!(
        both[0].matched,
        ["orders -> entry POST /orders", "cache -> kind CACHE"]
    );
    assert_eq!(both[0].hop.as_ref(), Some(cache_hop));
    // `health` names the flow that touches no cache: no flow holds both.
    let none = concept_flows(
        &m,
        &Default::default(),
        "health cache",
        &ConceptOptions::default(),
    );
    assert!(none.results.is_empty());
    let why = none.absence.expect("no flow touches both words");
    assert_eq!((why.tier, why.reason), ("fact", "no_match"));
}

#[test]
fn a_data_node_one_hop_out_matches_at_the_data_entity_tier() {
    let m = cached();
    // Within 3 hops the CACHE is a hop: its own name is a HopName.
    let walked = ask_cached(&m, "redis", 3);
    assert_eq!(walked[0].tier, ConceptTier::HopName);
    assert_eq!(walked[0].depth, 3);
    // Cut at 2 hops the walk stops at `place`, whose ACCESSES_DATA neighbour
    // still names the flow: read one hop out, never walked.
    let read = ask_cached(&m, "redis", 2);
    assert_eq!(read.len(), 1, "{read:#?}");
    assert_eq!(read[0].tier, ConceptTier::DataEntity);
    assert_eq!(read[0].matched, ["redis -> data entity data_source:redis"]);
    assert_eq!(
        read[0].hop.as_ref().map(|h| h.to_qname.as_str()),
        Some("svc::OrderService::place")
    );
    assert_eq!(read[0].depth, 2);
}

#[test]
fn a_data_entity_hop_matches_by_its_name_words() {
    let (_td, r) = shop();
    // The before-state: no node is named the query and no key holds it.
    let flows = entry_flows(&r.merged, &r.repo_labels, 6);
    assert!(flows.iter().all(|f| !f.key.contains("issuecomment")));
    let hits = ask(&r, "IssueComment");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let h = &hits[0];
    assert_eq!(h.key, "get_/orders/<oid>/comments");
    assert_eq!(h.tier, ConceptTier::HopName);
    assert_eq!(
        h.matched,
        [
            "issue -> name issue_comments",
            "comment -> entry GET /orders/<oid>/comments"
        ]
    );
    let hop = h.hop.as_ref().expect("the table hop");
    assert_eq!(
        (hop.to_kind, hop.to_qname.as_str(), hop.depth, h.depth),
        ("DATA_ENTITY", "data_entity:sql:issue_comments", 3, 3)
    );
}

#[test]
fn an_entry_name_matches_at_the_entry_tier() {
    let (_td, r) = shop();
    let hits = ask(&r, "health");
    let h = hit(&hits, "get_/health");
    assert_eq!(h.rank, 1);
    assert_eq!(h.tier, ConceptTier::Entry);
    assert_eq!((h.depth, h.hop.is_none()), (0, true));
    assert_eq!(h.matched, ["health -> entry GET /health"]);
}

#[test]
fn a_hop_name_a_qname_and_a_mechanism_match() {
    let (_td, r) = shop();
    let by_name = ask(&r, "notify");
    assert_eq!(by_name.len(), 1, "{by_name:#?}");
    assert_eq!(by_name[0].tier, ConceptTier::HopName);
    assert_eq!(
        by_name[0].hop.as_ref().map(|x| x.to_qname.as_str()),
        Some("app::notify")
    );
    // `sql` is a qname segment of the table hops, nothing's name.
    let sql = ask(&r, "sql");
    assert_eq!(sql.len(), 2, "{sql:#?}");
    assert!(sql.iter().all(|h| h.tier == ConceptTier::HopQname));
    // An edge category's words.
    let accesses = ask(&r, "accesses");
    assert_eq!(accesses.len(), 2, "{accesses:#?}");
    let h = hit(&accesses, "post_/orders");
    assert_eq!(h.tier, ConceptTier::Mechanism);
    assert_eq!(h.matched, ["accesses -> mechanism ACCESSES_DATA"]);
}

#[test]
fn a_doc_heading_documenting_a_hop_matches_at_the_doc_tier() {
    let (_td, r) = shop();
    let hits = ask(&r, "caching");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let h = &hits[0];
    assert_eq!(h.key, "post_/orders");
    assert_eq!(h.tier, ConceptTier::Doc);
    assert_eq!(
        h.hop.as_ref().map(|x| x.to_qname.as_str()),
        Some("app::place_order")
    );
    assert!(
        h.matched[0].starts_with("caching -> doc "),
        "{:?}",
        h.matched
    );
}

#[test]
fn an_external_host_matches_at_the_host_tier() {
    let (_td, r) = shop();
    let hits = ask(&r, "chat");
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let h = &hits[0];
    assert_eq!(h.key, "post_/orders");
    assert_eq!(h.tier, ConceptTier::ExternalHost);
    assert_eq!(h.matched, ["chat -> host hooks.example-chat.com"]);
    assert_eq!(h.hop.as_ref().map(|x| x.to_kind), Some("ENDPOINT"));
}

#[test]
fn a_word_nothing_touches_is_a_no_match_absence() {
    let (_td, r) = shop();
    let a = concept_flows(
        &r.merged,
        &r.repo_labels,
        "zebra",
        &ConceptOptions::default(),
    );
    assert!(a.results.is_empty());
    let why = a.absence.expect("an absence");
    assert_eq!((why.tier, why.reason), ("fact", "no_match"));
    assert!(why.note.contains("`zebra`"), "{}", why.note);
    // A query with no word of two characters says so.
    let empty = concept_flows(&r.merged, &r.repo_labels, " - ", &ConceptOptions::default());
    let why = empty.absence.expect("an absence");
    assert_eq!(why.reason, "no_match");
    assert!(why.note.contains("no word"), "{}", why.note);
}

#[test]
fn the_order_is_deterministic_and_the_limit_cuts_it() {
    let (_td, r) = shop();
    let all = ask(&r, "app");
    assert_eq!(
        all.len(),
        3,
        "every flow hops through an app:: qname: {all:#?}"
    );
    assert_eq!(
        serde_json::to_string(&all).expect("json"),
        serde_json::to_string(&ask(&r, "app")).expect("json")
    );
    for (i, h) in all.iter().enumerate() {
        assert_eq!(h.rank, i + 1);
    }
    // (tier, depth asc, reach desc, key): one tier and depth, so reach
    // decides first.
    let keys: Vec<&str> = all.iter().map(|h| h.key.as_str()).collect();
    assert_eq!(
        keys,
        ["post_/orders", "get_/orders/<oid>/comments", "get_/health"]
    );
    let mut one = ConceptOptions::default();
    one.limit = 1;
    let cut = concept_flows(&r.merged, &r.repo_labels, "app", &one).results;
    assert_eq!(cut, all[..1]);
    // A hit's JSON names its tier in snake_case and leaves out an empty
    // `blind`.
    let text = serde_json::to_string(&all[0]).expect("json");
    assert!(text.contains("\"tier\":\"hop_qname\""), "{text}");
    assert!(!text.contains("\"blind\""), "{text}");
}
