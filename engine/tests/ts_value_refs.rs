//! DT.5g — TypeScript value references and getter reads, end to end through
//! `generate_one` over `bench/substrate-gap/fixtures/ts-value-references`
//! (Kina's frontend/src/app shape).
//!
//! Angular wires code by value: a functional guard in a route table
//! (`canActivate: [authGuard]`), a functional interceptor and a provider call
//! in `export const appConfig = { providers: [..] }`, a provider class
//! (`useClass: AuthInterceptor`), a standalone component's pipe
//! (`@Component({ imports: [TimeAgoPipe] })`), a comparator (`sort(byUrgency)`);
//! and a service reads its base URL through a getter (`${this.apiUrl}/..`).
//! A module-level const initialiser and a class decorator's arguments are
//! walked, a bare identifier passed by value is a USES of what it binds (rule
//! `value_ref` in-file, `import_binding` through an import), and a
//! `this.<getter>` read is a CALLS of the getter (rule `accessor_read`).
//!
//! At HEAD 5dfe4c72 the fixture had CALLS 1/3, USES 0/5 and 18 dead_symbol
//! rows. dead_symbol is asserted by membership, so it holds whatever order
//! DT.5d's invoked-member rule and this packet land in.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::{Basis, Evidence};
use glia_code_domain::edge_category;
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "ts-value-references";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures").join(FIXTURE)
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` / `.glia/`).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read fixture dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// The fixture copied under `tmp` and built as one repo.
fn build(tmp: &Path) -> MergedGraph {
    copy_tree(&fixture_dir(), tmp);
    let root = tmp.display().to_string();
    generate_one(&root).unwrap_or_else(|e| panic!("generate_one: {e}")).merged
}

const APP: &str = "src::app";

/// The one node whose qname is exactly `src::app::<rel>`.
fn node(m: &MergedGraph, rel: &str) -> NodeId {
    let qname = format!("{APP}::{rel}");
    let ids = m.qnames_exact(&qname);
    assert_eq!(ids.len(), 1, "one node `{qname}`: {ids:?}");
    ids[0]
}

/// The evidence of the one `category` edge `from -> to`, if any.
fn edge_evidence(m: &MergedGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) -> Option<Evidence> {
    m.all_edges()
        .find(|e| e.from == from && e.to == to && e.category == category)
        .map(|e| Evidence::of(e).expect("every edge carries EVIDENCE"))
}

/// The source text at the evidence's site, its file read under `root`.
fn site_text(root: &Path, ev: &Evidence) -> String {
    assert_eq!(ev.basis, Basis::Site, "{ev:?}");
    let (Some(file), Some(line)) = (ev.file.as_deref(), ev.line) else {
        panic!("site evidence without a file and line: {ev:?}");
    };
    let text = std::fs::read_to_string(root.join(file)).expect("read site file");
    text.lines().nth(line as usize).unwrap_or_default().to_string()
}

/// The eight key.json expect_edges, each with the rule that binds it and a
/// fragment of its site line.
#[test]
fn value_references_and_getter_reads_are_edges() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |rel: &str| node(&m, rel);
    let routes = n("app.routes::routes");
    let config = n("app.config::appConfig");
    let page = n("home::home.page::HomePage");
    let rows = [
        (routes, n("core::guards::auth.guard::authGuard"), edge_category::USES, "import_binding", "canActivate: [authGuard]"),
        (config, n("core::interceptors::jwt.interceptor::jwtInterceptor"), edge_category::USES, "import_binding", "withInterceptors([jwtInterceptor])"),
        (config, n("core::i18n::provideI18n"), edge_category::CALLS, "import_binding", "provideI18n()"),
        (config, n("core::interceptors::auth.interceptor::AuthInterceptor"), edge_category::USES, "import_binding", "useClass: AuthInterceptor"),
        (n("home::home.page::HomePage::sorted"), n("home::home.page::byUrgency"), edge_category::USES, "value_ref", "sort(byUrgency)"),
        (n("home::home.page::HomePage::sortedBy"), n("home::home.page::sortWith"), edge_category::CALLS, "intra_file", "sortWith(byUrgency"),
        (n("core::services::trade.service::TradeService::list"), n("core::services::trade.service::TradeService::apiUrl"), edge_category::CALLS, "accessor_read", "${this.apiUrl}/trades"),
        (page, n("shared::time-ago.pipe::TimeAgoPipe"), edge_category::USES, "import_binding", "imports: [TimeAgoPipe]"),
    ];
    for (from, to, category, rule, site) in rows {
        let ev = edge_evidence(&m, from, to, category)
            .unwrap_or_else(|| panic!("{site}: no edge {from:?} -> {to:?}"));
        assert_eq!(ev.rule.as_deref(), Some(rule), "{site}: {ev:?}");
        let text = site_text(tmp.path(), &ev);
        assert!(text.contains(site), "{site}: the site line is `{}`", text.trim());
    }
    // The lazy route's arrow parameter (`(m) => m.HomePage`) is a local: the
    // routes STATE_VAR USES its guard and nothing else.
    let routes_out: Vec<NodeId> = m
        .all_edges()
        .filter(|e| e.from == routes && e.category == edge_category::USES)
        .map(|e| e.to)
        .collect();
    assert_eq!(routes_out, [n("core::guards::auth.guard::authGuard")]);
}

/// The key.json forbid edges: a parameter shadowing a function, a name never
/// passed, a never-read getter and an unimported pipe bind nothing.
#[test]
fn shadowed_and_unreferenced_names_bind_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |rel: &str| node(&m, rel);
    let by_name = n("home::home.page::byName");
    let forbid = [
        (n("home::home.page::sortWith"), by_name, edge_category::USES),
        (n("home::home.page::HomePage::sorted"), by_name, edge_category::USES),
        (n("app.routes::routes"), n("core::guards::auth.guard::adminGuard"), edge_category::USES),
        (
            n("core::services::trade.service::TradeService::list"),
            n("core::services::trade.service::TradeService::unused"),
            edge_category::CALLS,
        ),
        (n("home::home.page::HomePage"), n("shared::initials.pipe::InitialsPipe"), edge_category::USES),
    ];
    for (from, to, category) in forbid {
        assert!(edge_evidence(&m, from, to, category).is_none(), "{from:?} -> {to:?}");
    }
    let into_by_name: Vec<NodeId> =
        m.all_edges().filter(|e| e.to == by_name && e.category != edge_category::DEFINES).map(|e| e.from).collect();
    assert!(into_by_name.is_empty(), "byName is referenced by nothing: {into_by_name:?}");
}

/// dead_symbol by membership: the by-value referenced guard, interceptors,
/// provider, comparator, pipe and the read getter are no rows; the truly
/// unused symbols stay.
#[test]
fn referenced_symbols_leave_dead_symbol() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    let q = |rel: &str| format!("{APP}::{rel}");
    for gone in [
        "core::guards::auth.guard::authGuard",
        "core::interceptors::jwt.interceptor::jwtInterceptor",
        "core::i18n::provideI18n",
        "core::interceptors::auth.interceptor::AuthInterceptor",
        "core::services::trade.service::TradeService::apiUrl",
        "home::home.page::byUrgency",
        "shared::time-ago.pipe::TimeAgoPipe",
    ] {
        assert!(!rows.contains(&q(gone)), "{gone} is referenced, no row: {rows:#?}");
    }
    for stays in [
        "core::guards::auth.guard::adminGuard",
        "core::services::trade.service::TradeService",
        "core::services::trade.service::TradeService::unused",
        "core::services::trade.service::TradeService::list",
        "home::home.page::byName",
        "home::home.page::HomePage::sorted",
        "home::home.page::HomePage::sortedBy",
        "shared::initials.pipe::InitialsPipe",
        "shared::initials.pipe::InitialsPipe::transform",
    ] {
        assert!(rows.contains(&q(stays)), "{stays} is used by nothing, a row: {rows:#?}");
    }
}
