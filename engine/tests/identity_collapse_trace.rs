//! DD.11: `cross_stack_trace` and `entry_flows` walk the identity quotient
//! and tag hops. A walk class (messages, contract ops, SCIP symbols, a package
//! and its project; DD.3's `walk` rules, read through DD.8's `IdentityView`)
//! is ONE node of the carry index and the successor map, so a path reaching
//! one SCIP-identical copy goes on through every copy's successors at zero
//! hops, two copies' identical paths are one path, and two entries in one
//! class are one flow. A hop into a FACT / DERIVED class shows the class's
//! display member and carries `to_identity {tier, rules, diverged, also_in}`.
//! `Collapse::off()` is the HEAD walk; a graph with no SAME_AS edge answers
//! byte for byte as before.
//!
//! The repos are two tempdir Flask apps (`billing`, `shop`) whose
//! `.glia/scip-snapshot/`s are written the DD.7 way (`identity_scip.rs`,
//! `identity_collapse_blast.rs`): one package `money`, so `format_cents`,
//! `main` and `OrderDto` are each one FACT `scip_symbol` class across the two
//! repos (`format_cents` `/diverged`: shop's appends ` EUR` through `_eur`);
//! SharedSchemaResolver also pairs the two `OrderDto`s (SHARES_SCHEMA, a
//! carry edge).
//!
//! - billing `lib/money.py`: `GET /invoice` -> `invoice` -> `format_cents`
//!   and `lib/orders.py::place` -> `OrderDto`; `main` -> `format_cents`.
//! - shop `app/money.py`: `GET /checkout` -> `checkout` -> `format_cents` ->
//!   `_eur`; `main` -> `format_cents`; `app/orders.py::OrderDto`.
//!
//! Baseline before DD.11 (HEAD b5b4fcc2, the same repos through the HEAD
//! API): `trace "GET /invoice" --to app::money::format_cents` was ONE path,
//! length 3, `directed: false` (the undirected fallback over the SAME_AS
//! edge); `trace "GET /invoice"` ranked 2 paths, the format_cents one ending
//! at billing's copy (no `_eur`) and the DTO one listing both DTOs through
//! SHARES_SCHEMA (length 4); `entry_flows` gave 4 flows, `main` twice
//! (`app::money::main` reach 2, `lib::money::main` reach 1), and `GET
//! /invoice` reach 5 with both DTOs. No hop had any identity, and this file
//! did not compile (`TraceOptions` had no `collapse`, `TraceHop` no
//! `to_identity`, `entry_flows_with_live` no `collapse` argument).

use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
};
use glia_engine::identity::Collapse;
use glia_engine::trace::{
    EntryFlow, TraceAnswer, TraceHop, TraceOptions, cross_stack_trace, entry_flows,
    entry_flows_with_live,
};
use glia_engine::{GenerateResult, entrypoint_reachable, generate_many};

const BILLING_MONEY: &str = "from flask import Flask\n\nfrom lib.orders import place\n\napp = Flask(__name__)\n\n\ndef format_cents(cents):\n    return \"%.2f\" % (cents / 100)\n\n\n@app.route(\"/invoice\")\ndef invoice():\n    place(100)\n    return format_cents(100)\n\n\ndef main():\n    return format_cents(1)\n";
const BILLING_ORDERS: &str = "class OrderDto:\n    def __init__(self, total):\n        self.total = total\n\n\ndef place(total):\n    return OrderDto(total)\n";
const SHOP_MONEY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\ndef format_cents(cents):\n    return _eur(\"%.2f\" % (cents / 100))\n\n\ndef _eur(text):\n    return text + \" EUR\"\n\n\n@app.route(\"/checkout\")\ndef checkout():\n    return format_cents(100)\n\n\ndef main():\n    return format_cents(1)\n";
const SHOP_ORDERS: &str = "class OrderDto:\n    def __init__(self, total):\n        self.total = total\n";

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, text).expect("write");
}

/// The 0-based row of the line of `text` starting with `needle`.
fn row_of(text: &str, needle: &str) -> u32 {
    text.lines().position(|l| l.starts_with(needle)).unwrap_or_else(|| panic!("no `{needle}`")) as u32
}

/// `repo`'s snapshot: `<dir>/money.py` defines `format_cents` and `main`,
/// `<dir>/orders.py` `OrderDto`, under the package `money` whatever the
/// directory, so each name is one class across the repos.
fn snapshot(repo: &Path, dir: &str, money: &str) {
    let doc = |file: &str, defs: Vec<ScipDefRow>| {
        let path = format!("{dir}/{file}");
        let bytes = std::fs::read(repo.join(&path)).expect("source");
        ScipDocumentRecord {
            path,
            language: "python".to_string(),
            source_hash: source_hash(&bytes),
            defs,
            refs: Vec::new(),
        }
    };
    let def = |s: u32, line: u32, name: &str| ScipDefRow { s, line, name: name.to_string() };
    let documents = vec![
        doc(
            "money.py",
            vec![def(0, row_of(money, "def format_cents"), "format_cents"), def(1, row_of(money, "def main"), "main")],
        ),
        doc("orders.py", vec![def(2, 0, "OrderDto")]),
    ];
    let sym = |d: &str| format!("scip-python python money 1.0.0 {d}");
    let symbols = vec![
        ScipSymbolRecord { id: 0, symbol: sym("`money`/format_cents()."), implements: Vec::new() },
        ScipSymbolRecord { id: 1, symbol: sym("`money`/main()."), implements: Vec::new() },
        ScipSymbolRecord { id: 2, symbol: sym("`orders`/OrderDto#"), implements: Vec::new() },
    ];
    let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write snapshot");
}

/// The two repos in a fresh tempdir (removed on drop), snapshots written.
fn repos() -> tempfile::TempDir {
    let td = tempfile::tempdir().expect("tempdir");
    write(td.path(), "billing/lib/money.py", BILLING_MONEY);
    write(td.path(), "billing/lib/orders.py", BILLING_ORDERS);
    write(td.path(), "shop/app/money.py", SHOP_MONEY);
    write(td.path(), "shop/app/orders.py", SHOP_ORDERS);
    snapshot(&td.path().join("billing"), "lib", BILLING_MONEY);
    snapshot(&td.path().join("shop"), "app", SHOP_MONEY);
    td
}

fn build_paths(roots: &[PathBuf]) -> GenerateResult {
    let paths: Vec<String> = roots.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

/// billing then shop: billing is the first graph, so its member of each
/// class is the display member.
fn build_both(td: &tempfile::TempDir) -> GenerateResult {
    build_paths(&[td.path().join("billing"), td.path().join("shop")])
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures").join(name)
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged.cross_edges.iter().filter(|e| e.category == edge_category::SAME_AS).count()
}

/// Trace options: `to`, and collapse on with the build's labels or off.
fn opts(r: &GenerateResult, to: Option<&str>, on: bool) -> TraceOptions {
    let mut o = TraceOptions::default();
    o.to = to.map(str::to_string);
    o.collapse = if on { Collapse::with_labels(&r.repo_labels) } else { Collapse::off() };
    o
}

fn trace(r: &GenerateResult, feature: &str, to: Option<&str>, on: bool) -> TraceAnswer {
    cross_stack_trace(&r.merged, feature, &opts(r, to, on))
}

fn flows(r: &GenerateResult, on: bool) -> Vec<EntryFlow> {
    let collapse = if on { Collapse::with_labels(&r.repo_labels) } else { Collapse::off() };
    entry_flows_with_live(&r.merged, &entrypoint_reachable(&r.merged), &r.repo_labels, 6, &collapse)
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("answer serialises")
}

/// A path as `from -> to` qname pairs.
fn steps(hops: &[TraceHop]) -> Vec<(&str, &str)> {
    hops.iter().map(|h| (h.from_qname.as_str(), h.to_qname.as_str())).collect()
}

fn flow<'a>(flows: &'a [EntryFlow], qname: &str) -> &'a EntryFlow {
    flows
        .iter()
        .find(|f| f.entry.qname == qname)
        .unwrap_or_else(|| panic!("no `{qname}` flow in {flows:#?}"))
}

/// The repo label of each `also_in` member, with its qname.
fn also(h: &TraceHop) -> Vec<(&str, &str)> {
    h.to_identity
        .as_ref()
        .map(|id| id.also_in.iter().map(|a| (a.repo.as_str(), a.qname.as_str())).collect())
        .unwrap_or_default()
}

// ---- tests ------------------------------------------------------------------

/// (1) Over the quotient the format_cents copies are one node: billing's
/// route reaches shop's `format_cents` in 2 directed hops (route -> handler
/// -> the class); the HEAD walk has no directed path, only the undirected
/// fallback over SAME_AS. A path continues through the other copy's
/// callees, and two class-mates' identical paths are one path.
#[test]
fn one_path_for_copies() {
    let td = repos();
    let r = build_both(&td);
    assert_eq!(same_as_edges(&r), 3, "control: format_cents, main and OrderDto are classes");

    // Two-node mode.
    let on = trace(&r, "GET /invoice", Some("app::money::format_cents"), true);
    assert_eq!(on.target.as_ref().map(|t| t.qname.as_str()), Some("app::money::format_cents"), "target is the query's node");
    assert_eq!(on.paths.len(), 1, "{on:#?}");
    let p = &on.paths[0];
    assert!(p.directed, "{p:#?}");
    assert_eq!(p.length, 2, "route -> handler -> the format_cents class: {p:#?}");
    assert_eq!(p.mechanisms, ["HANDLED_BY", "CALLS"]);
    assert_eq!(
        steps(&p.hops),
        [("GET /invoice", "lib::money::invoice"), ("lib::money::invoice", "lib::money::format_cents")],
        "the class shows its display member, billing's copy (the first graph's)"
    );
    assert!(p.hops.iter().all(|h| !h.cross_repo && !h.cross_service), "{p:#?}");

    let off = trace(&r, "GET /invoice", Some("app::money::format_cents"), false);
    assert!(!off.paths.is_empty(), "HEAD: the undirected fallback: {off:#?}");
    assert!(off.paths.iter().all(|p| !p.directed), "HEAD: no directed path to shop's copy: {off:#?}");
    assert_eq!(off.paths[0].length, 3);
    assert_eq!(off.paths[0].hops[2].mechanism, "SAME_AS");

    // A target in the seed's walk class is the zero-length path.
    let same = trace(&r, "lib::money::format_cents", Some("app::money::format_cents"), true);
    assert_eq!(same.paths.len(), 1);
    assert_eq!((same.paths[0].length, same.paths[0].directed), (0, true), "{same:#?}");
    let raw_same = trace(&r, "lib::money::format_cents", Some("app::money::format_cents"), false);
    assert!(raw_same.paths.iter().all(|p| !p.directed && p.length == 1), "{raw_same:#?}");

    // One-node mode: the format_cents path goes on through shop's copy's
    // callee `_eur`; the HEAD path stops at billing's copy.
    let on = trace(&r, "GET /invoice", None, true);
    let to_eur = on
        .paths
        .iter()
        .find(|p| p.hops.last().is_some_and(|h| h.to_qname == "app::money::_eur"))
        .unwrap_or_else(|| panic!("a path into shop's _eur: {on:#?}"));
    assert_eq!(
        steps(&to_eur.hops),
        [
            ("GET /invoice", "lib::money::invoice"),
            ("lib::money::invoice", "lib::money::format_cents"),
            ("lib::money::format_cents", "app::money::_eur"),
        ]
    );
    assert!(to_eur.directed);
    assert!(to_eur.hops[2].cross_repo, "the shown ends sit in two repos");
    let off = trace(&r, "GET /invoice", None, false);
    assert!(
        off.paths.iter().all(|p| p.hops.iter().all(|h| h.to_qname != "app::money::_eur")),
        "HEAD never reaches _eur: {off:#?}"
    );
    assert!(on.hops.iter().any(|h| h.to_qname == "app::money::_eur"), "the BFS tree reaches it too");

    // Two class-mates are one seed: billing's and shop's `main` answer alike,
    // one path (main -> format_cents -> _eur), where HEAD has a different
    // path per copy.
    let a = trace(&r, "lib::money::main", None, true);
    let b = trace(&r, "app::money::main", None, true);
    assert_eq!(json(&a.paths), json(&b.paths));
    assert_eq!(json(&a.hops), json(&b.hops));
    assert_eq!(a.paths.len(), 1, "{a:#?}");
    assert_eq!(
        steps(&a.paths[0].hops),
        [("lib::money::main", "lib::money::format_cents"), ("lib::money::format_cents", "app::money::_eur")]
    );
    let (ra, rb) = (trace(&r, "lib::money::main", None, false), trace(&r, "app::money::main", None, false));
    assert_ne!(json(&ra.paths), json(&rb.paths), "HEAD: one path per copy");
    assert_eq!(steps(&ra.paths[0].hops), [("lib::money::main", "lib::money::format_cents")]);
    assert_eq!(a.seed.as_ref().map(|s| s.qname.as_str()), Some("lib::money::main"));
    assert_eq!(b.seed.as_ref().map(|s| s.qname.as_str()), Some("app::money::main"), "seed is the query's node");
}

/// (2) A hop into a class carries its identity: FACT, `scip_symbol`,
/// diverged (the bodies differ), and `also_in` naming shop's copy; a hop
/// outside every class has none, and its JSON no `to_identity` key.
#[test]
fn hop_identity() {
    let td = repos();
    let r = build_both(&td);
    let on = trace(&r, "GET /invoice", Some("app::money::format_cents"), true);
    let last = on.paths[0].hops.last().expect("a hop");
    let id = last.to_identity.as_ref().expect("the class hop's identity");
    assert_eq!((id.tier, id.diverged), ("fact", true));
    assert_eq!(id.rules, ["scip_symbol"]);
    assert_eq!(also(last), [("shop", "app::money::format_cents")], "also_in names the other repo");
    assert!(id.also_in.iter().all(|a| a.file.is_some() && a.line.is_some_and(|l| l >= 1)), "located, 1-based");
    assert_eq!((last.to_file.as_deref(), last.to_line), (Some("lib/money.py"), Some(8)), "the display member's place");

    let first = &on.paths[0].hops[0];
    assert!(first.to_identity.is_none());
    let text = json(first);
    assert!(!text.contains("to_identity"), "{text}");
    assert!(json(&on).contains("\"to_identity\":{\"tier\":\"fact\""), "{}", json(&on));

    // The BFS tree tags the same way: the DTO class shows billing's DTO.
    let tree = trace(&r, "GET /invoice", None, true);
    let dto: Vec<&TraceHop> = tree.hops.iter().filter(|h| h.to_qname.ends_with("::OrderDto")).collect();
    assert_eq!(dto.len(), 1, "the DTO class once: {:#?}", tree.hops);
    assert_eq!(dto[0].to_qname, "lib::orders::OrderDto");
    assert_eq!(also(dto[0]), [("shop", "app::orders::OrderDto")]);
    assert!(tree.hops.iter().all(|h| h.mechanism != "SHARES_SCHEMA"), "the pair is intra-class: {:#?}", tree.hops);

    // Collapse off: no hop carries an identity.
    let off = trace(&r, "GET /invoice", None, false);
    assert!(off.hops.iter().chain(off.paths.iter().flat_map(|p| &p.hops)).all(|h| h.to_identity.is_none()));
    assert!(!json(&off).contains("to_identity"));
}

/// (3) Flows dedupe: the two `main` entries are one walk class, so one flow,
/// shown as billing's (the display member); every flow lists a class once
/// (the DTO pair is one node, its SHARES_SCHEMA edge intra-class). The
/// identity-contract-ops class never enters a flow (its DOCUMENTS edges run
/// doc -> route, against the flow), so that fixture's flows are the HEAD
/// flows.
#[test]
fn flows_dedupe() {
    let td = repos();
    let r = build_both(&td);
    let off = flows(&r, false);
    let mains_off: Vec<&str> = off.iter().filter(|f| f.key == "main").map(|f| f.entry.qname.as_str()).collect();
    assert_eq!(mains_off, ["app::money::main", "lib::money::main"], "HEAD: one flow per entry");
    assert_eq!(off.len(), 4);

    let on = flows(&r, true);
    assert_eq!(json(&on), json(&entry_flows(&r.merged, &r.repo_labels, 6)), "entry_flows collapses with its labels");
    let mains: Vec<&EntryFlow> = on.iter().filter(|f| f.key == "main").collect();
    assert_eq!(mains.len(), 1, "{on:#?}");
    let main = mains[0];
    assert_eq!(main.entry.qname, "lib::money::main", "the display member");
    assert_eq!(main.entry.file.as_deref(), Some("lib/money.py"));
    assert_eq!(
        steps(&main.hops),
        [("lib::money::main", "lib::money::format_cents"), ("lib::money::format_cents", "app::money::_eur")]
    );
    assert_eq!(main.services, ["billing", "shop"], "the flow touches shop through the class");
    assert!(main.cross_service);
    assert_eq!(on.len(), 3);

    let invoice = flow(&on, "GET /invoice");
    let dtos = invoice.hops.iter().filter(|h| h.to_qname.ends_with("::OrderDto")).count();
    assert_eq!(dtos, 1, "{invoice:#?}");
    let fcs = invoice.hops.iter().filter(|h| h.to_qname.ends_with("::format_cents")).count();
    assert_eq!(fcs, 1);
    let raw_invoice = flow(&off, "GET /invoice");
    assert_eq!(raw_invoice.hops.iter().filter(|h| h.to_qname.ends_with("::OrderDto")).count(), 2, "HEAD: both DTOs");
    for f in &on {
        let mut seen: Vec<&str> = f.hops.iter().map(|h| h.to_qname.as_str()).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "{}: each node once: {f:#?}", f.key);
    }

    // identity-contract-ops: one DERIVED walk class of three DOC_SECTION ops,
    // never on the route's forward flow.
    let ops = build_paths(&[fixture("identity-contract-ops")]);
    assert!(same_as_edges(&ops) > 0, "control: the ops are one class");
    let on = flows(&ops, true);
    let route = flow(&on, "GET /api/orders");
    let docs = route.hops.iter().filter(|h| h.to_kind == "DOC_SECTION").count();
    assert!(docs <= 1, "the contract class at most once: {route:#?}");
    assert_eq!(json(&on), json(&flows(&ops, false)), "a flow no class enters is its HEAD self");
}

/// (4) `package_name` classes collapse but never walk: the hop into
/// `package:npm:react` shows the node the walk reached (its own repo, so
/// never `cross_repo`) and carries the class's identity; the walk itself is
/// the HEAD walk.
#[test]
fn package_hop_shows_the_reached_node() {
    let r = build_paths(&[fixture("identity-npm-package").join("web"), fixture("identity-npm-package").join("admin")]);
    assert!(same_as_edges(&r) > 0);
    let on = trace(&r, "package.json", None, true);
    let off = trace(&r, "package.json", None, false);
    let react = on.hops.iter().find(|h| h.to_qname == "package:npm:react").unwrap_or_else(|| panic!("{on:#?}"));
    assert!(!react.cross_repo, "{react:#?}");
    let id = react.to_identity.as_ref().expect("react's identity");
    assert_eq!((id.tier, id.rules.as_slice()), ("fact", ["package_name"].as_slice()));
    assert_eq!(id.also_in.len(), 1);
    let strip = |a: &TraceAnswer| -> Vec<(String, String, bool)> {
        a.hops.iter().map(|h| (h.from_qname.clone(), h.to_qname.clone(), h.cross_repo)).collect()
    };
    assert_eq!(strip(&on), strip(&off), "the HEAD walk");
    assert_eq!(on.paths.len(), off.paths.len());
}

/// (5) No SAME_AS edge: the default trace and flows are the
/// `Collapse::off()` answers, byte for byte, with no identity key.
#[test]
fn class_free_identical() {
    let td = repos();
    let billing = build_paths(&[td.path().join("billing")]);
    assert_eq!(same_as_edges(&billing), 0, "control: one SCIP repo, no class");
    let npm = build_paths(&[fixture("identity-npm-package").join("web")]);
    assert_eq!(same_as_edges(&npm), 0);
    for (r, feature, to) in [
        (&billing, "GET /invoice", None),
        (&billing, "GET /invoice", Some("lib::orders::OrderDto")),
        (&billing, "lib::money::main", None),
        (&billing, "lib::orders::OrderDto", Some("GET /invoice")),
        (&billing, "zzqqxx", None),
        (&npm, "package.json", None),
    ] {
        let a = json(&trace(r, feature, to, true));
        let b = json(&trace(r, feature, to, false));
        assert_eq!(a, b, "{feature} -> {to:?}");
        assert!(!a.contains("to_identity"), "{a}");
    }
    for r in [&billing, &npm] {
        let a = json(&flows(r, true));
        assert_eq!(a, json(&flows(r, false)));
        assert_eq!(a, json(&entry_flows(&r.merged, &r.repo_labels, 6)));
        assert!(!a.contains("to_identity"));
    }
}
