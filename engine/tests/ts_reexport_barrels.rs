//! DT.1q — TS / JS re-export barrels resolve as ECMA-262 ResolveExport, over
//! a REAL build of `bench/substrate-gap/fixtures/ts-reexport-barrels`
//! (quokka-stack's shape: `@app/core` -> `core/index.ts` -> `models|services/
//! index.ts` stars, a namespace re-export, a renamed named re-export, a
//! from-less `export { User as Customer }`, a `.js` star barrel behind a
//! directory import, and a same-named `legacy/` decoy). Before DT.1q the
//! barrels gave no IMPORTS edge and no binding, so none of the 16 key rows
//! existed. `bench/` is outside the cargo workspace's crates, so the fixture
//! is copied into a tempdir (never its key.json) and built there.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{GapsOptions, gaps_report};
use glia_engine::persist::{load_layout, persist_result};
use glia_engine::{GenerateResult, ParseCache, generate_one, generate_one_with_cache};
use glia_graph::{MergedGraph, RepoGraph};

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/ts-reexport-barrels",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn build() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let r = generate_one(&td.path().to_string_lossy()).expect("ts-reexport-barrels builds");
    (td, r)
}

const APP: &str = "web::src::app";

/// `web::src::app::<rest>`.
fn q(rest: &str) -> String {
    format!("{APP}::{rest}")
}

/// A node's qname without its ` @<project>` owner segment.
fn qname_of(m: &MergedGraph, id: NodeId) -> Option<String> {
    m.graphs.iter().find_map(|g| {
        g.nav.qname_by_id.get(&id).map(|q| q.split(" @").next().unwrap_or(q).to_string())
    })
}

/// The id of the node whose qname (owner segment stripped) is `qname`.
fn id_of(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find(|(_, q)| q.split(" @").next() == Some(qname))
                .map(|(id, _)| *id)
        })
        .unwrap_or_else(|| panic!("no node {qname}"))
}

/// The per-language graph holding MODULE `qname` (the TS family's).
fn graph_of<'a>(m: &'a MergedGraph, qname: &str) -> &'a RepoGraph {
    let id = id_of(m, qname);
    m.graphs
        .iter()
        .find(|g| g.symbols.module_by_qname.values().any(|v| *v == id))
        .unwrap_or_else(|| panic!("no graph holds {qname}"))
}

type Row = (u32, String, String, String, String, Option<u32>);

/// Every non-structural edge as `(category id, from, to, emitter, rule, line)`.
fn rows(m: &MergedGraph) -> BTreeSet<Row> {
    m.graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(m.cross_edges.iter())
        .filter(|e| e.category != edge_category::DEFINES && e.category != edge_category::CONTAINS)
        .filter_map(|e| {
            let ev = Evidence::of(e)?;
            Some((
                e.category.0,
                qname_of(m, e.from)?,
                qname_of(m, e.to)?,
                ev.emitter.clone(),
                ev.rule.clone().unwrap_or_default(),
                ev.line,
            ))
        })
        .collect()
}

/// A symbol table (`module_symbols` / `module_import_bindings`) in id order.
fn table(t: &HashMap<NodeId, HashMap<String, NodeId>>) -> BTreeMap<u64, BTreeMap<String, u64>> {
    t.iter()
        .map(|(m, s)| (m.0, s.iter().map(|(k, v)| (k.clone(), v.0)).collect()))
        .collect()
}

fn has_edge(m: &MergedGraph, cat: EdgeCategoryId, from: &str, to: &str) -> bool {
    rows(m).iter().any(|(c, f, t, ..)| *c == cat.0 && f == from && t == to)
}

#[test]
fn barrels_bind_the_declaring_symbols() {
    let (_td, r) = build();
    let got = rows(&r.merged);
    let imports = [
        ("core::index", "core::models::index", 0),
        ("core::index", "core::services::index", 1),
        ("core::index", "core::util::format", 2),
        ("core::models::index", "core::models::user.model", 0),
        ("core::models::index", "core::models::order.model", 1),
        ("core::services::index", "core::services::user.service", 0),
        ("core::services::index", "core::services::audit", 1),
        ("shared::index", "shared::badge", 0),
    ];
    // (category, from, to, emitter, rule, line).
    type Want<'a> = (EdgeCategoryId, String, String, &'a str, &'a str, Option<u32>);
    let mut want: Vec<Want<'_>> = imports
        .iter()
        .map(|(f, t, line)| {
            (edge_category::IMPORTS, q(f), q(t), "graph:imports", "reexport", Some(*line))
        })
        .collect();
    let page = "features::profile::profile.page::ProfilePage";
    let load = "features::profile::profile.page::ProfilePage::load";
    let total = "features::orders::order-list.service::OrderListService::total";
    let label = "features::orders::order-list.service::OrderListService::label";
    let vip = "features::profile::vip::VipCustomer";
    let (refs, calls, bind) = ("graph:refs", "graph:calls", "import_binding");
    for (cat, from, to, emitter, rule) in [
        (edge_category::INJECTS, page, "core::services::user.service::UserService", refs, bind),
        (edge_category::INJECTS, page, "core::services::audit::AuditLog", refs, bind),
        (edge_category::CALLS, load, "core::services::user.service::UserService::find", calls, "receiver_type"),
        (edge_category::CALLS, load, "core::services::audit::AuditLog::record", calls, "receiver_type"),
        (edge_category::CALLS, load, "core::util::format::shout", calls, "attribute"),
        (edge_category::CALLS, total, "core::models::order.model::orderTotal", calls, bind),
        (edge_category::CALLS, label, "shared::badge::badgeLabel", calls, bind),
        (edge_category::INHERITS_FROM, vip, "core::models::user.model::User", refs, bind),
    ] {
        want.push((cat, q(from), q(to), emitter, rule, None));
    }
    assert_eq!(want.len(), 16);
    for (cat, from, to, emitter, rule, line) in want {
        let hit = got.iter().find(|(c, f, t, ..)| *c == cat.0 && *f == from && *t == to);
        let Some((_, _, _, e, r, l)) = hit else {
            panic!("missing {cat:?} {from} -> {to}");
        };
        assert_eq!((e.as_str(), r.as_str()), (emitter, rule), "{from} -> {to}");
        if line.is_some() {
            assert_eq!(*l, line, "{from} -> {to}");
        }
    }
}

#[test]
fn ambiguity_and_name_guesses_bind_nothing() {
    let (_td, r) = build();
    let m = &r.merged;
    let load = q("features::profile::profile.page::ProfilePage::load");
    let page = q("features::profile::profile.page::ProfilePage");
    let forbid = [
        (edge_category::CALLS, load.clone(), q("core::models::user.model::displayName")),
        (edge_category::CALLS, load.clone(), q("core::models::order.model::displayName")),
        (edge_category::INJECTS, page.clone(), q("legacy::user.service::UserService")),
        (edge_category::CALLS, load.clone(), q("legacy::user.service::UserService::find")),
        (edge_category::CALLS, load, q("legacy::user.service::shout")),
        (edge_category::IMPORTS, q("core::index"), q("core::models::user.model")),
        (edge_category::IMPORTS, q("features::profile::profile.page"), q("core::services::user.service")),
        (
            edge_category::INJECTS,
            q("features::reports::report.page::ReportPage"),
            q("core::services::audit::AuditLog"),
        ),
        (
            edge_category::CALLS,
            q("features::reports::report.page::ReportPage::ngOnInit"),
            q("core::services::audit::AuditLog::record"),
        ),
    ];
    for (cat, from, to) in &forbid {
        assert!(!has_edge(m, *cat, from, to), "forbidden {cat:?} {from} -> {to}");
    }

    let g = graph_of(m, &q("core::index"));
    let syms = |module: &str| -> BTreeMap<String, NodeId> {
        let id = id_of(m, &q(module));
        let table = g.symbols.module_symbols.get(&id);
        table.map(|s| s.iter().map(|(k, v)| (k.clone(), *v)).collect()).unwrap_or_default()
    };
    let core = syms("core::index");
    for (name, decl) in [
        ("User", "core::models::user.model::User"),
        ("Order", "core::models::order.model::Order"),
        ("orderTotal", "core::models::order.model::orderTotal"),
        ("UserService", "core::services::user.service::UserService"),
    ] {
        assert_eq!(core.get(name), Some(&id_of(m, &q(decl))), "{name}");
    }
    for name in ["Customer", "Journal", "fmt", "displayName"] {
        assert!(!core.contains_key(name), "core::index holds {name}");
    }
    assert!(!syms("core::models::index").contains_key("displayName"));

    let bound = |module: &str, name: &str| {
        g.symbols.module_import_bindings.get(&id_of(m, &q(module)))?.get(name).copied()
    };
    let profile = "features::profile::profile.page";
    assert_eq!(bound(profile, "Journal"), Some(id_of(m, &q("core::services::audit::AuditLog"))));
    assert_eq!(bound(profile, "fmt"), Some(id_of(m, &q("core::util::format"))));
    assert_eq!(bound(profile, "displayName"), None);
    let user = id_of(m, &q("core::models::user.model::User"));
    assert_eq!(bound("features::profile::vip", "Customer"), Some(user));
}

#[test]
fn liveness_follows_barrel_bindings() {
    let (_td, r) = build();
    let roots: Vec<(u64, PathBuf)> = r.repo_roots.iter().map(|(id, p)| (*id, PathBuf::from(p))).collect();
    let report = gaps_report(&r.merged, &roots, &GapsOptions::default()).expect("known categories");
    let dead: BTreeSet<String> = report
        .rows
        .iter()
        .filter(|row| row.category == "dead_symbol")
        .map(|row| row.qname.split(" @").next().unwrap_or(&row.qname).to_string())
        .collect();
    // Reached from the live ProfilePage, called, or inherited from.
    for live in [
        "core::services::user.service::UserService",
        "core::services::user.service::UserService::find",
        "core::services::audit::AuditLog",
        "core::services::audit::AuditLog::record",
        "core::util::format::shout",
        "core::models::order.model::orderTotal",
        "shared::badge::badgeLabel",
        "core::models::user.model::User",
        "core::models::user.model::User::constructor",
    ] {
        assert!(!dead.contains(&q(live)), "{live} is dead");
    }
    for still_dead in [
        "core::models::order.model::Order",
        "core::models::order.model::Order::constructor",
        "core::models::user.model::displayName",
        "core::models::order.model::displayName",
        "features::orders::order-list.service::OrderListService",
        "features::orders::order-list.service::OrderListService::total",
        "features::orders::order-list.service::OrderListService::label",
        "features::profile::vip::VipCustomer",
        "legacy::user.service::UserService",
        "legacy::user.service::UserService::find",
        "legacy::user.service::shout",
    ] {
        assert!(dead.contains(&q(still_dead)), "{still_dead} is live");
    }
    assert_eq!(dead.len(), 11, "{dead:#?}");
}

#[test]
fn the_layout_keeps_the_export_tables() {
    let (td, r) = build();
    let dir = td.path().join("layout");
    persist_result(&r, &dir, "test").expect("persist");
    let loaded = load_layout(&dir).expect("load");
    let core = q("core::index");
    let (built, back) = (graph_of(&r.merged, &core), graph_of(&loaded.merged, &core));
    assert_eq!(table(&back.symbols.module_symbols), table(&built.symbols.module_symbols));
    assert_eq!(table(&back.symbols.module_import_bindings), table(&built.symbols.module_import_bindings));
    // The barrel's same-name rows and the importers' bindings are among them.
    let barrel = id_of(&loaded.merged, &core);
    assert!(back.symbols.module_symbols[&barrel].contains_key("UserService"));
    let vip = id_of(&loaded.merged, &q("features::profile::vip"));
    assert_eq!(
        back.symbols.module_import_bindings[&vip].get("Customer"),
        Some(&id_of(&loaded.merged, &q("core::models::user.model::User")))
    );
}

#[test]
fn the_parse_cache_keeps_the_export_facts() {
    // The facts ride the parse cache: a warm build read back from the saved
    // sidecar binds exactly what a clean build does.
    let (td, clean) = build();
    let repo = td.path().to_string_lossy().into_owned();
    let mut cache = ParseCache::new();
    generate_one_with_cache(&repo, &mut cache).expect("fill");
    cache.save(&repo).expect("save");
    let mut cache = ParseCache::load(&repo);
    let warm = generate_one_with_cache(&repo, &mut cache).expect("warm");
    assert!(cache.stats.reused > 0 && cache.stats.reparsed == 0, "{:?}", cache.stats);
    assert_eq!(rows(&warm.merged), rows(&clean.merged));
    let core = q("core::index");
    let (a, b) = (graph_of(&warm.merged, &core), graph_of(&clean.merged, &core));
    assert_eq!(table(&a.symbols.module_symbols), table(&b.symbols.module_symbols));
    assert_eq!(table(&a.symbols.module_import_bindings), table(&b.symbols.module_import_bindings));
}
