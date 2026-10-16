//! DD.10: `blast_radius` walks the identity quotient and collapses rows. A
//! walk class (messages, contract ops, SCIP symbols, a package and its
//! project; DD.3's `walk` rules, read through DD.8's `IdentityView`) is ONE
//! node in the BFS and the PPR, so a change reaching one SCIP-identical copy
//! reaches the other copy's callers at zero hops, and a carry edge between
//! two members of one class (a SHARES_SCHEMA pair that is also one SCIP
//! class) is dropped, so the pair is walked once. A FACT / DERIVED class is
//! one row carrying `identity {tier, rules, diverged, also_in}`, folded
//! before `scope` and `top_k`; a `package_name` class folds but never walks.
//! `Collapse::off()` is the HEAD walk and rows; a graph with no SAME_AS edge
//! answers byte for byte as before.
//!
//! Real `generate_many` builds: DD.5's substrate-gap identity fixtures, and
//! two tempdir repos (`billing`, `shop`) with `.glia/scip-snapshot/`s written
//! the DD.7 way (`identity_scip.rs`: `write_scip`, one definition row per
//! symbol, the source hash of the file on disk).
//!
//! Baseline before DD.10 (HEAD b998d369): this file did not compile
//! (`BlastOptions` had no `collapse`, `BlastAnswer` / `BlastSeed` no
//! `identity`), and `glia blast-radius <identity-contract-ops> "GET
//! /api/orders" --direction backward --depth 1 --json` printed three
//! DOC_SECTION rows (docs/swagger.yaml:7, main.go:14,
//! features/orders/feature.yaml:4), each at depth 1 via DOCUMENTS, with no
//! `identity` key.

use std::path::{Path, PathBuf};

use glia_activation::algo::GraphSource;
use glia_code_domain::edge_category;
use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipSymbolRecord, source_hash, write_scip,
};
use glia_core::NodeId;
use glia_engine::identity::{Collapse, IdentityView};
use glia_engine::{BlastAnswer, BlastOptions, BlastRadius, GenerateResult, blast_radius, generate_many};
use glia_graph::Reach;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(name)
}

fn build_paths(roots: &[PathBuf]) -> GenerateResult {
    let paths: Vec<String> = roots.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap_or_else(|e| panic!("{paths:?} builds: {e}"))
}

fn build(name: &str, dirs: &[&str]) -> GenerateResult {
    let root = fixture(name);
    build_paths(&dirs.iter().map(|d| root.join(d)).collect::<Vec<_>>())
}

fn same_as_edges(r: &GenerateResult) -> usize {
    r.merged.cross_edges.iter().filter(|e| e.category == edge_category::SAME_AS).count()
}

/// The options of one blast: `direction`, `depth`, and collapse on with the
/// build's labels (`on`) or off.
fn opts(r: &GenerateResult, direction: Reach, depth: usize, on: bool) -> BlastOptions {
    let mut o = BlastOptions::default();
    o.direction = direction;
    o.depth = depth;
    o.collapse = if on { Collapse::with_labels(&r.repo_labels) } else { Collapse::off() };
    o
}

fn blast(r: &GenerateResult, queries: &[&str], o: &BlastOptions) -> BlastRadius {
    blast_radius(&r.merged, queries, o)
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("answer serialises")
}

fn qnames(rows: &[BlastAnswer]) -> Vec<&str> {
    let mut q: Vec<&str> = rows.iter().map(|a| a.qname.as_str()).collect();
    q.sort_unstable();
    q
}

fn row<'a>(rows: &'a [BlastAnswer], qname: &str) -> Option<&'a BlastAnswer> {
    rows.iter().find(|a| a.qname == qname)
}

/// The label of the repo whose graph holds `id`.
fn repo_of(r: &GenerateResult, id: u64) -> String {
    r.merged
        .graphs
        .iter()
        .find(|g| g.nav.qname_by_id.contains_key(&NodeId(id)))
        .and_then(|g| r.repo_labels.get(&g.repo.0).cloned())
        .unwrap_or_else(|| panic!("no graph holds {id}"))
}

// ---- the SCIP tempdir repos -------------------------------------------------

const BILLING_MONEY: &str = "def format_cents(cents):\n    return \"%.2f\" % (cents / 100)\n\n\ndef invoice(cents):\n    return format_cents(cents)\n";
const BILLING_ORDERS: &str = "class OrderDto:\n    def __init__(self, total):\n        self.total = total\n\n\ndef place(total):\n    return OrderDto(total)\n";
const SHOP_MONEY: &str = "def format_cents(cents):\n    return \"%.2f EUR\" % (cents / 100)\n\n\ndef checkout(cents):\n    return format_cents(cents)\n\n\ndef main():\n    checkout(100)\n";
const SHOP_ORDERS: &str = "class OrderDto:\n    def __init__(self, total):\n        self.total = total\n";

/// `scip-python python money 1.0.0 <descriptor>`: one package, so the two
/// repos' definitions of one descriptor are one `scip_symbol` class.
fn symbol(descriptor: &str) -> String {
    format!("scip-python python money 1.0.0 {descriptor}")
}

/// Two repos under a fresh directory, removed on drop:
/// - `billing`: `lib/money.py` (`format_cents`, `invoice` calls it),
///   `lib/orders.py` (`OrderDto`, `place` constructs it), `package.json`
///   (left-pad);
/// - `shop`: `app/money.py` (its own `format_cents`, ` EUR` appended;
///   `checkout` calls it, `main` calls `checkout`), `app/orders.py`
///   (`OrderDto`), `web/package.json` (left-pad).
///
/// Each repo's snapshot defines `format_cents` and `OrderDto` under one
/// package, so each pair is one FACT `scip_symbol` class (format_cents
/// `/diverged`); SharedSchemaResolver also pairs the two `OrderDto`s
/// (SHARES_SCHEMA, a carry edge). The qnames differ by directory, so every
/// seed names one node.
struct ScipRepos(PathBuf);

impl ScipRepos {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-dd10-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().expect("a parent")).expect("dir");
            std::fs::write(p, text).expect("write");
        };
        write("billing/lib/money.py", BILLING_MONEY);
        write("billing/lib/orders.py", BILLING_ORDERS);
        write("billing/package.json", "{\"name\": \"billing\", \"dependencies\": {\"left-pad\": \"1.3.0\"}}\n");
        write("shop/app/money.py", SHOP_MONEY);
        write("shop/app/orders.py", SHOP_ORDERS);
        write("shop/web/package.json", "{\"name\": \"shop-web\", \"dependencies\": {\"left-pad\": \"1.3.0\"}}\n");
        for (repo, dir) in [("billing", "lib"), ("shop", "app")] {
            snapshot(&root.join(repo), dir);
        }
        ScipRepos(root)
    }

    fn build(&self) -> GenerateResult {
        build_paths(&[self.0.join("billing"), self.0.join("shop")])
    }
}

impl Drop for ScipRepos {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `repo`'s snapshot: `<dir>/money.py` defines `format_cents` and
/// `<dir>/orders.py` defines `OrderDto`, each at row 0, under the package
/// `money` whatever the directory.
fn snapshot(repo: &Path, dir: &str) {
    let doc = |file: &str, s: u32, name: &str| {
        let path = format!("{dir}/{file}");
        let bytes = std::fs::read(repo.join(&path)).expect("source");
        ScipDocumentRecord {
            path,
            language: "python".to_string(),
            source_hash: source_hash(&bytes),
            defs: vec![ScipDefRow { s, line: 0, name: name.to_string() }],
            refs: Vec::new(),
        }
    };
    let documents = vec![doc("money.py", 0, "format_cents"), doc("orders.py", 1, "OrderDto")];
    let symbols = vec![
        ScipSymbolRecord { id: 0, symbol: symbol("`money`/format_cents()."), implements: Vec::new() },
        ScipSymbolRecord { id: 1, symbol: symbol("`orders`/OrderDto#"), implements: Vec::new() },
    ];
    let meta = ScipMeta::new("scip-python".to_string(), "0.6.0".to_string(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write snapshot");
}

// ---- tests ------------------------------------------------------------------

/// (1) The three ops of one contract class (swagger, swaggo, feature.yaml:
/// `contract_op` + `contract_route`, DERIVED) are three rows at HEAD and one
/// row with its identity by default.
#[test]
fn contract_ops_one_row() {
    let r = build("identity-contract-ops", &["."]);
    assert!(same_as_edges(&r) > 0, "control: the ops are one class");
    let q = ["GET /api/orders"];
    let docs = |a: &BlastRadius| -> Vec<BlastAnswer> {
        a.results.iter().filter(|a| a.kind == "DOC_SECTION").cloned().collect()
    };

    let off = blast(&r, &q, &opts(&r, Reach::Backward, 1, false));
    let off_docs = docs(&off);
    assert_eq!(
        qnames(&off_docs),
        [
            "contract::docs::swagger::GET:/orders",
            "contract::features::orders::feature::GET:/api/orders",
            "contract::main::GET:/orders",
        ],
        "HEAD shape: one DOC_SECTION row per op: {off:#?}"
    );
    assert!(off.results.iter().all(|a| a.identity.is_none() && a.depth == 1));
    assert!(!json(&off).contains("\"identity\""), "{}", json(&off));

    assert!(BlastOptions::default().collapse.enabled, "collapse is on by default");
    let on = blast(&r, &q, &opts(&r, Reach::Backward, 1, true));
    let on_docs = docs(&on);
    assert_eq!(on_docs.len(), 1, "{on:#?}");
    let shown = &on_docs[0];
    assert_eq!((shown.depth, shown.reason), (1, "DOCUMENTS"));
    let id = shown.identity.as_ref().expect("the class row carries its identity");
    assert_eq!(id.tier, "derived");
    assert_eq!(id.rules, ["contract_op", "contract_route"]);
    assert!(!id.diverged);
    assert_eq!(id.also_in.len(), 2, "{:?}", id.also_in);
    let mut members: Vec<&str> = id.also_in.iter().map(|a| a.qname.as_str()).collect();
    members.push(shown.qname.as_str());
    members.sort_unstable();
    assert_eq!(members, qnames(&off_docs), "the row and its also_in are the three ops");
    assert!(
        id.also_in.iter().all(|a| a.file.is_some() && a.line.is_some_and(|l| l >= 1)),
        "contract ops are located, 1-based: {:?}",
        id.also_in
    );
    assert!(json(&on).contains("\"identity\":{\"tier\":\"derived\""), "{}", json(&on));
    // The shown member is located at its own place (a row of the HEAD shape).
    let head = row(&off_docs, &shown.qname).expect("the shown member is a HEAD row");
    assert_eq!((&shown.file, shown.line), (&head.file, head.line));
    // The seed is in no class.
    assert!(on.seeds.iter().all(|s| s.identity.is_none()), "{:?}", on.seeds);
}

/// (2) Over the quotient one SCIP class is one node: a backward blast from
/// billing's `format_cents` reaches billing's `invoice` AND shop's
/// `checkout`, each one hop away, and never shop's copy itself (it is the
/// seed); the HEAD walk reaches only `invoice`.
#[test]
fn quotient_crosses_scip_copies() {
    let repos = ScipRepos::new("cross");
    let r = repos.build();
    let seed = ["lib::money::format_cents"];

    let off = blast(&r, &seed, &opts(&r, Reach::Backward, 4, false));
    assert_eq!(qnames(&off.results), ["lib::money::invoice"], "{off:#?}");

    let on = blast(&r, &seed, &opts(&r, Reach::Backward, 4, true));
    let invoice = row(&on.results, "lib::money::invoice").expect("invoice");
    let checkout = row(&on.results, "app::money::checkout").expect("checkout, through shop's copy");
    assert_eq!((invoice.depth, invoice.reason), (1, "CALLS"));
    assert_eq!((checkout.depth, checkout.reason), (1, "CALLS"), "zero hops across the class");
    assert_eq!(repo_of(&r, checkout.id), "shop");
    assert!(
        on.results.iter().all(|a| !a.qname.ends_with("::format_cents")),
        "the seed's class-mate is the seed, never a row: {on:#?}"
    );
    assert!(on.results.iter().all(|a| a.seed == "lib::money::format_cents"), "{on:#?}");
    assert!(invoice.identity.is_none() && checkout.identity.is_none());

    // The seed carries its class: shop's copy, FACT, scip_symbol, diverged.
    let s = &on.seeds[0];
    assert_eq!(s.qname, "lib::money::format_cents");
    let id = s.identity.as_ref().expect("the seed's identity");
    assert_eq!((id.tier, id.diverged), ("fact", true));
    assert_eq!(id.rules, ["scip_symbol"]);
    let also: Vec<(&str, &str)> = id.also_in.iter().map(|a| (a.repo.as_str(), a.qname.as_str())).collect();
    assert_eq!(also, [("shop", "app::money::format_cents")]);

    // Two seeds in one walk class are one seed, the first query kept.
    let both = blast(&r, &["lib::money::format_cents", "app::money::format_cents"], &opts(&r, Reach::Backward, 4, true));
    assert_eq!(both.seeds.len(), 1, "{:?}", both.seeds);
    assert_eq!(both.seeds[0].query, "lib::money::format_cents");
    assert_eq!(json(&both.results), json(&on.results));
    let raw_both = blast(&r, &["lib::money::format_cents", "app::money::format_cents"], &opts(&r, Reach::Backward, 4, false));
    assert_eq!(raw_both.seeds.len(), 2, "HEAD: two seeds");
}

/// (3) The carry-edge interplay: billing's and shop's `OrderDto` are one SCIP
/// class AND a SHARES_SCHEMA pair. Over the quotient that edge is
/// intra-class and dropped, so a forward blast from billing's `place` lists
/// the class once; the HEAD walk lists both DTOs.
#[test]
fn no_double_walk() {
    let repos = ScipRepos::new("schema");
    let r = repos.build();
    let schema: Vec<_> = r
        .merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::SHARES_SCHEMA)
        .collect();
    assert_eq!(schema.len(), 1, "control: SharedSchemaResolver pairs the DTOs");

    let seed = ["lib::orders::place"];
    let off = blast(&r, &seed, &opts(&r, Reach::Both, 4, false));
    let dtos = |a: &BlastRadius| a.results.iter().filter(|a| a.qname.ends_with("::OrderDto")).count();
    assert_eq!(dtos(&off), 2, "HEAD: both DTOs, one through SHARES_SCHEMA: {off:#?}");
    let shared = row(&off.results, "app::orders::OrderDto").expect("shop's DTO at HEAD");
    assert_eq!((shared.depth, shared.reason), (2, "SHARES_SCHEMA"));

    for direction in [Reach::Forward, Reach::Both] {
        let on = blast(&r, &seed, &opts(&r, direction, 4, true));
        assert_eq!(dtos(&on), 1, "{direction:?}: the class once: {on:#?}");
        let dto = on.results.iter().find(|a| a.qname.ends_with("::OrderDto")).expect("the DTO row");
        assert_eq!((dto.depth, dto.reason), (1, "CALLS"), "{direction:?}");
        let id = dto.identity.as_ref().expect("the DTO row's identity");
        assert_eq!((id.tier, id.rules.as_slice()), ("fact", ["scip_symbol"].as_slice()));
        let others: Vec<&str> = id.also_in.iter().map(|a| a.qname.as_str()).collect();
        let shown_billing = dto.qname == "lib::orders::OrderDto";
        let want = if shown_billing { "app::orders::OrderDto" } else { "lib::orders::OrderDto" };
        assert_eq!(others, [want], "{direction:?}");
        assert!(on.results.iter().all(|a| a.reason != "SHARES_SCHEMA"), "{direction:?}: {on:#?}");
    }

    // The quotient drops the two SCIP stars and the SHARES_SCHEMA edge (the
    // left-pad `package_name` star is no walk edge and stays).
    let view = IdentityView::new(&r.merged, &Collapse::default());
    let q = view.quotient(&r.merged).expect("SCIP classes walk");
    let kept_same_as = q.edges().filter(|e| e.category == edge_category::SAME_AS).count();
    let dropped_same_as = same_as_edges(&r) - kept_same_as;
    assert!(dropped_same_as >= 2, "the format_cents and OrderDto stars: {dropped_same_as}");
    assert!(q.collapsed_edges() >= 2, "collapsed {}", q.collapsed_edges());
    assert_eq!(q.collapsed_edges(), dropped_same_as + 1, "the stars and the SHARES_SCHEMA pair");
    assert!(
        q.edges().all(|e| e.category != edge_category::SHARES_SCHEMA),
        "the SHARES_SCHEMA pair is intra-class"
    );
}

/// (4) `package_name` folds but never walks: a blast from a manifest lists
/// one row per package, carrying the other repo's member, and never enters
/// the other repo through it (the walk is the HEAD walk). Two seeds reaching
/// both members of one package class fold to one row, before `top_k`.
#[test]
fn packages_fold_not_walk() {
    let r = build("identity-npm-package", &["web", "admin"]);
    assert!(same_as_edges(&r) > 0);
    let view = IdentityView::new(&r.merged, &Collapse::with_labels(&r.repo_labels));
    assert!(!view.is_empty() && !view.has_walk_classes(), "package_name collapses, never walks");

    for direction in [Reach::Forward, Reach::Both] {
        let on = blast(&r, &["package.json"], &opts(&r, direction, 4, true));
        let seed_repo = repo_of(&r, on.seeds[0].id);
        let react: Vec<&BlastAnswer> = on.results.iter().filter(|a| a.qname == "package:npm:react").collect();
        assert_eq!(react.len(), 1, "{direction:?}: one react row: {on:#?}");
        assert!(
            on.results.iter().all(|a| repo_of(&r, a.id) == seed_repo),
            "{direction:?}: the walk stays in {seed_repo}: {on:#?}"
        );
        let id = react[0].identity.as_ref().expect("react's identity");
        assert_eq!((id.tier, id.rules.as_slice()), ("fact", ["package_name"].as_slice()));
        assert_eq!(id.also_in.len(), 1);
        assert_ne!(id.also_in[0].repo, seed_repo, "also_in names the other repo");

        // The walk is the HEAD walk: the same rows, scores and order.
        let off = blast(&r, &["package.json"], &opts(&r, direction, 4, false));
        let strip = |a: &BlastRadius| -> Vec<(u64, String, usize, u64)> {
            a.results.iter().map(|x| (x.id, x.qname.clone(), x.depth, x.score.to_bits())).collect()
        };
        assert_eq!(strip(&on), strip(&off), "{direction:?}");
    }

    // Two seeds, one manifest per repo (the tempdir repos: billing's
    // package.json, shop's web/package.json), reach both left-pad members:
    // one row, the best-scored, and top_k counts it once.
    let repos = ScipRepos::new("pkg");
    let r = repos.build();
    let seeds = ["package.json", "web::package.json"];
    let off = blast(&r, &seeds, &opts(&r, Reach::Forward, 1, false));
    assert_eq!(off.seeds.len(), 2, "control: both manifests resolve: {:?}", off.seeds);
    let pads = |a: &BlastRadius| a.results.iter().filter(|x| x.qname == "package:npm:left-pad").count();
    assert_eq!(pads(&off), 2, "{off:#?}");
    let on = blast(&r, &seeds, &opts(&r, Reach::Forward, 1, true));
    assert_eq!(pads(&on), 1, "{on:#?}");
    assert_eq!(on.results.len(), off.results.len() - 1);
    let best = off.results.iter().find(|x| x.qname == "package:npm:left-pad").expect("a pad");
    let pad = on.results.iter().find(|x| x.qname == "package:npm:left-pad").expect("the pad row");
    assert_eq!(pad.id, best.id, "the best-scored member is the row");
    assert!(pad.identity.as_ref().is_some_and(|i| i.also_in.len() == 1));
    let mut top = opts(&r, Reach::Forward, 1, true);
    top.top_k = Some(1);
    let one = blast(&r, &seeds, &top);
    assert_eq!(one.results.len(), 1);
    assert_eq!(json(&one.results[0]), json(&on.results[0]));
}

/// (5) `live_only` keeps a walk class when ANY member is live: shop's
/// `main` reaches shop's `format_cents`, billing's (the display member, the
/// first graph's) is dead. A forward blast from billing's `invoice` keeps
/// the class row, flagged live; the HEAD walk drops billing's dead copy.
#[test]
fn live_any_member() {
    let repos = ScipRepos::new("live");
    let r = repos.build();
    let live = glia_engine::entrypoint_reachable(&r.merged);
    let id_of = |q: &str| {
        r.merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.iter().find(|(_, v)| v.as_str() == q).map(|(k, _)| *k))
            .unwrap_or_else(|| panic!("no node {q}"))
    };
    assert!(!live.contains(&id_of("lib::money::format_cents")), "control: billing's copy is dead");
    assert!(live.contains(&id_of("app::money::format_cents")), "control: main reaches shop's copy");

    let seed = ["lib::money::invoice"];
    let mut o = opts(&r, Reach::Forward, 4, true);
    o.live_only = true;
    let on = blast(&r, &seed, &o);
    assert_eq!(on.results.len(), 1, "{on:#?}");
    let fc = &on.results[0];
    assert_eq!(fc.qname, "lib::money::format_cents", "the display member, though dead");
    assert!(fc.live, "the class is live through shop's copy");
    assert!(fc.identity.is_some());

    // Without live_only the row is the same and flagged live.
    let all = blast(&r, &seed, &opts(&r, Reach::Forward, 4, true));
    assert_eq!(json(&all.results), json(&on.results));

    let mut raw = opts(&r, Reach::Forward, 4, false);
    raw.live_only = true;
    let off = blast(&r, &seed, &raw);
    assert!(off.results.is_empty(), "HEAD: billing's copy is dead: {off:#?}");
    assert_eq!(off.absence.as_ref().map(|a| a.reason), Some("no_match"));
}

/// (6) No SAME_AS edge: the default answer is the `Collapse::off()` answer,
/// byte for byte, in every direction and with every option.
#[test]
fn no_class_byte_identical() {
    let r = build("identity-npm-package", &["web"]);
    assert_eq!(same_as_edges(&r), 0, "control: one repo, no class");
    let repos = ScipRepos::new("one");
    let billing = build_paths(&[repos.0.join("billing")]);
    assert_eq!(same_as_edges(&billing), 0, "control: one SCIP repo, no class");
    for (r, queries) in [
        (&r, vec!["package.json"]),
        (&r, vec!["src::index::App", "package:npm:react"]),
        (&billing, vec!["lib::money::format_cents"]),
        (&billing, vec!["lib::orders::place", "nope"]),
        (&billing, vec!["zzqqxx"]),
    ] {
        for direction in [Reach::Forward, Reach::Backward, Reach::Both] {
            for (live_only, top_k) in [(false, None), (true, None), (false, Some(1))] {
                let mut on = opts(r, direction, 4, true);
                on.live_only = live_only;
                on.top_k = top_k;
                let mut off = on.clone();
                off.collapse = Collapse::off();
                let a = json(&blast(r, &queries, &on));
                let b = json(&blast(r, &queries, &off));
                assert_eq!(a, b, "{queries:?} {direction:?} live_only={live_only} top_k={top_k:?}");
                assert!(!a.contains("\"identity\""), "{a}");
            }
        }
    }
}
