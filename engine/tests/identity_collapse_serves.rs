//! DD.13: `serves` collapses identity classes among servers and their
//! handlers. A server row that is one FACT / DERIVED class with another folds
//! to the first and carries `identity`; within a server, handlers of one class
//! keep the first, and the server's `handler_identity` names each handler that
//! stands for a class with that class's `identity {tier, rules, diverged,
//! also_in}` (DD.8's `IdentityView`). `Collapse::off()` is the HEAD answer; a
//! graph with no SAME_AS edge answers byte for byte as before.
//!
//! No shipped identity rule pairs two ROUTEs or QUEUE_CONSUMERs (a route is
//! per service), so a real build only tags handlers: two repos running one
//! SCIP-identical Flask handler (`billing`, `shop`, each `app.py` with `POST
//! /orders` -> `create_order`, byte-identical, both `.glia/scip-snapshot/`s
//! written the DD.7 way, `identity_scip.rs`) answer two servers, each handler
//! tagged FACT `scip_symbol` with the other repo's copy in `also_in`. The
//! server fold and the in-server handler fold are pinned on the same build
//! with test-only SAME_AS / HANDLED_BY cross edges, the mechanism a future
//! rule (or a route bound to two copies) would use.
//!
//! Baseline before DD.13 (HEAD 7466a4c5): this file did not compile
//! (`serves::serves_with` and `serves::HandlerIdentity` did not exist,
//! `Server` had no `identity` / `handler_identity`), and `serves(.., "POST
//! /orders", "auto")` over the two repos listed two ROUTE rows, each with its
//! own `app::create_order` handler and no identity field.

use std::collections::HashSet;
use std::path::Path;

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
};
use glia_code_domain::{edge_category, same_as};
use glia_core::{Confidence, Edge, NodeId};
use glia_engine::identity::{Collapse, IdentityView};
use glia_engine::serves::{Server, serves, serves_with, serves_with_live};
use glia_engine::{GenerateResult, entrypoint_reachable, generate_many};

/// One Flask app: `POST /orders` handled by `create_order` (row 6, 0-based).
const APP: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
@app.route(\"/orders\", methods=[\"POST\"])\ndef create_order():\n    return {\"ok\": True}\n";
const DEF_ROW: u32 = 6;
const SYMBOL: &str = "scip-python python shop-svc 1.0 `app`/create_order().";
const HANDLER: &str = "app::create_order";
const CHANNEL: &str = "POST /orders";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

/// `repo`'s snapshot: `app.py` defines `create_order` under [`SYMBOL`].
fn snapshot(repo: &Path) {
    let bytes = std::fs::read(repo.join("app.py")).expect("source");
    let documents = vec![ScipDocumentRecord {
        path: "app.py".to_string(),
        language: "python".to_string(),
        source_hash: source_hash(&bytes),
        defs: vec![ScipDefRow { s: 0, line: DEF_ROW, name: "create_order".to_string() }],
        refs: Vec::new(),
    }];
    let symbols = vec![ScipSymbolRecord { id: 0, symbol: SYMBOL.to_string(), implements: Vec::new() }];
    let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write snapshot");
}

/// `billing` and `shop` in a fresh tempdir (removed on drop); with `scip`,
/// both indexed.
fn repos(scip: bool) -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("tempdir");
    for repo in ["billing", "shop"] {
        write(&td.path().join(repo), "app.py", APP);
        if scip {
            snapshot(&td.path().join(repo));
        }
    }
    td
}

fn build(td: &tempfile::TempDir, repos: &[&str]) -> GenerateResult {
    let paths: Vec<String> = repos
        .iter()
        .map(|r| td.path().join(r).to_string_lossy().into_owned())
        .collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged.cross_edges.iter().filter(|e| e.category == edge_category::SAME_AS).count()
}

fn on(r: &GenerateResult) -> Collapse {
    Collapse::with_labels(&r.repo_labels)
}

fn answer(r: &GenerateResult, collapse: &Collapse) -> Vec<Server> {
    let live = entrypoint_reachable(&r.merged);
    let a = serves_with(&r.merged, &live, CHANNEL, "auto", collapse).expect("http is a mechanism");
    assert!(a.absence.is_none(), "{:?}", a.absence);
    a.results
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("answer serialises")
}

/// The build label of the repo whose graph names `id`.
fn repo_of(r: &GenerateResult, id: u64) -> String {
    let g = r
        .merged
        .graphs
        .iter()
        .find(|g| g.nav.qname_by_id.contains_key(&NodeId(id)))
        .expect("a node of the build");
    r.repo_labels.get(&g.repo.0).cloned().expect("a labelled repo")
}

/// The handler node of `server` (its only HANDLED_BY target).
fn handler_of(s: &Server) -> u64 {
    assert_eq!(s.handlers.len(), 1, "{s:#?}");
    s.handlers[0].id
}

/// A test-only cross edge carrying its EVIDENCE cell.
fn cross(r: &mut GenerateResult, from: u64, to: u64, category: glia_core::EdgeCategoryId, ev: Evidence) {
    let mut e = Edge::new(NodeId(from), NodeId(to), category, Confidence::Strong);
    evidence::attach(&mut e, ev);
    r.merged.cross_edges.push(e);
}

// ---- tests ------------------------------------------------------------------

/// Two repos running one SCIP-identical handler: two ROUTE servers, kept
/// (no rule makes routes one class), each handler tagged FACT `scip_symbol`
/// with the other repo's copy in `also_in`. `Collapse::off()` has no
/// `handler_identity` key and is otherwise the same rows.
#[test]
fn scip_identical_handlers() {
    let td = repos(true);
    let r = build(&td, &["billing", "shop"]);
    assert!(same_as_edges(&r) > 0, "control: the create_order copies are one SCIP class");

    let rows = answer(&r, &on(&r));
    assert_eq!(rows.len(), 2, "two ROUTEs, kept: {rows:#?}");
    let mut repos_seen: Vec<String> = Vec::new();
    for s in &rows {
        assert_eq!((s.kind, s.r#match), ("ROUTE", "exact"), "{s:#?}");
        assert!(s.identity.is_none(), "no rule pairs routes: {s:#?}");
        let own = repo_of(&r, handler_of(s));
        assert_eq!(own, repo_of(&r, s.id), "a server's handler is its own repo's");
        assert_eq!(s.handlers[0].qname, HANDLER);
        assert_eq!(s.handler_identity.len(), 1, "{s:#?}");
        let hi = &s.handler_identity[0];
        assert_eq!(hi.qname, HANDLER);
        assert_eq!(hi.identity.tier, "fact", "{hi:#?}");
        assert_eq!(hi.identity.rules, [same_as::RULE_SCIP_SYMBOL]);
        assert!(!hi.identity.diverged, "the two bodies are byte-identical");
        let other = if own == "billing" { "shop" } else { "billing" };
        let also: Vec<(&str, &str, Option<&str>, Option<i64>)> = hi
            .identity
            .also_in
            .iter()
            .map(|a| (a.repo.as_str(), a.qname.as_str(), a.file.as_deref(), a.line))
            .collect();
        assert_eq!(
            also,
            [(other, HANDLER, Some("app.py"), s.handlers[0].line)],
            "the other repo's copy, located (identical files: the same line)"
        );
        repos_seen.push(own);
    }
    repos_seen.sort();
    assert_eq!(repos_seen, ["billing", "shop"]);
    let text = json(&rows);
    assert!(
        text.contains("\"handler_identity\":[{\"qname\":\"app::create_order\",\"identity\":{\"tier\":\"fact\""),
        "{text}"
    );
    assert!(!text.contains("\"identity\":null"), "an unset identity is no key: {text}");

    // Off: the same servers and handlers, no identity keys.
    let off = answer(&r, &Collapse::off());
    let off_text = json(&off);
    assert!(!off_text.contains("identity"), "{off_text}");
    let ids = |rows: &[Server]| rows.iter().map(|s| (s.id, handler_of(s))).collect::<Vec<_>>();
    assert_eq!(ids(&off), ids(&rows));

    // serves / serves_with_live delegate with Collapse::default(): on, no
    // labels, so also_in names the repo by its `repo#<id>` form.
    assert!(Collapse::default().enabled);
    let plain = serves(&r.merged, CHANNEL, "auto").expect("an answer");
    let live = entrypoint_reachable(&r.merged);
    let cached = serves_with_live(&r.merged, &live, CHANNEL, "auto").expect("an answer");
    let dflt = serves_with(&r.merged, &live, CHANNEL, "auto", &Collapse::default()).expect("an answer");
    assert_eq!(json(&plain), json(&dflt));
    assert_eq!(json(&cached), json(&dflt));
    for s in &dflt.results {
        let also = &s.handler_identity[0].identity.also_in;
        assert!(also.len() == 1 && also[0].repo.starts_with("repo#"), "{also:?}");
    }

    // Unknown mechanism is still an error.
    let err = serves_with(&r.merged, &live, CHANNEL, "smtp", &on(&r)).expect_err("unknown");
    assert!(err.contains("smtp"), "{err}");
}

/// A server row in a FACT / DERIVED class with another folds to the first
/// row (its own handlers), carrying the class: two routes made one class by a
/// test-only `resolver:identity` `contract_route` SAME_AS (derived). Off,
/// both rows.
#[test]
fn a_server_class_folds_to_one_row() {
    let td = repos(false);
    let mut r = build(&td, &["billing", "shop"]);
    assert!(
        IdentityView::new(&r.merged, &on(&r)).is_empty(),
        "control: no SCIP snapshot, no collapse class (the copies are only heuristic candidates)"
    );
    let raw = answer(&r, &Collapse::off());
    assert_eq!(raw.len(), 2, "{raw:#?}");
    let (a, b) = (raw[0].id, raw[1].id);
    cross(
        &mut r,
        a,
        b,
        edge_category::SAME_AS,
        Evidence::emitter("resolver:identity").rule(same_as::RULE_CONTRACT_ROUTE),
    );

    let rows = answer(&r, &on(&r));
    assert_eq!(rows.len(), 1, "one row for the class: {rows:#?}");
    let s = &rows[0];
    assert_eq!(s.id, a, "the first row in answer order is kept");
    assert_eq!(handler_of(s), handler_of(&raw[0]), "with its own handlers");
    assert!(s.handler_identity.is_empty(), "the handler is in no class: {s:#?}");
    let id = s.identity.as_ref().expect("the kept row carries its class");
    assert_eq!((id.tier, id.rules.as_slice()), ("derived", [same_as::RULE_CONTRACT_ROUTE].as_slice()));
    let also: Vec<(String, &str)> = id.also_in.iter().map(|m| (m.repo.clone(), m.qname.as_str())).collect();
    assert_eq!(also, [(repo_of(&r, b), raw[1].qname.as_str())]);
    assert!(json(&rows).contains("\"identity\":{\"tier\":\"derived\""), "{}", json(&rows));

    let off = answer(&r, &Collapse::off());
    assert_eq!(json(&off), json(&raw), "off is the raw rows");
}

/// Handlers of one server that are one class keep the first: billing's route
/// also HANDLED_BY shop's SCIP-identical copy (test-only edge) lists billing's
/// handler alone, its `handler_identity` naming shop's copy. Off, both.
#[test]
fn one_servers_handlers_of_one_class_fold() {
    let td = repos(true);
    let mut r = build(&td, &["billing", "shop"]);
    let raw = answer(&r, &Collapse::off());
    let billing = raw.iter().find(|s| repo_of(&r, s.id) == "billing").expect("billing's route");
    let shop = raw.iter().find(|s| repo_of(&r, s.id) == "shop").expect("shop's route");
    let (route, own, copy) = (billing.id, handler_of(billing), handler_of(shop));
    cross(
        &mut r,
        route,
        copy,
        edge_category::HANDLED_BY,
        Evidence::emitter("test:dd13").rule("second_handler"),
    );

    let off = answer(&r, &Collapse::off());
    let off_b = off.iter().find(|s| s.id == route).expect("billing's route");
    let hs: Vec<u64> = off_b.handlers.iter().map(|h| h.id).collect();
    assert_eq!(hs, [own, copy], "off: both handlers, in edge order");

    let rows = answer(&r, &on(&r));
    let s = rows.iter().find(|s| s.id == route).expect("billing's route");
    let hs: Vec<u64> = s.handlers.iter().map(|h| h.id).collect();
    assert_eq!(hs, [own], "the class's first handler kept: {s:#?}");
    assert_eq!(s.handler_identity.len(), 1, "{s:#?}");
    let also: Vec<&str> = s.handler_identity[0].identity.also_in.iter().map(|m| m.repo.as_str()).collect();
    assert_eq!(also, ["shop"]);
    assert_eq!(rows.len(), 2, "the servers themselves stay two");
}

/// No collapse class: the default answer is the `Collapse::off()` answer,
/// byte for byte, with no identity key — for a served channel, a bare path,
/// an unserved verb and a queue topic. One repo has no SAME_AS edge at all;
/// the two unindexed repos' byte-identical copies are SAME_AS only as
/// HEURISTIC candidates (`body_hash` / `path_suffix`), which never fold or
/// tag (Q18).
#[test]
fn class_free_identical() {
    let td = repos(false);
    let one = build(&td, &["billing"]);
    assert_eq!(same_as_edges(&one), 0, "control: one repo, no SAME_AS");
    let two = build(&td, &["billing", "shop"]);
    assert!(same_as_edges(&two) > 0, "control: the unindexed copies are heuristic candidates");
    assert!(IdentityView::new(&two.merged, &on(&two)).is_empty(), "control: no collapse class");
    for r in [&one, &two] {
        let live: HashSet<NodeId> = entrypoint_reachable(&r.merged);
        for channel in [CHANNEL, "/orders", "DELETE /orders", "orders.created"] {
            let a = serves_with(&r.merged, &live, channel, "auto", &on(r)).expect("an answer");
            let b = serves_with(&r.merged, &live, channel, "auto", &Collapse::off()).expect("an answer");
            assert_eq!(json(&a), json(&b), "{channel}");
            assert!(!json(&a).contains("identity"), "{}", json(&a));
        }
    }
}
