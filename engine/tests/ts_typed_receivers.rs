//! DT.5k — TypeScript receivers the graph could not type, end to end through
//! `generate_one` over `bench/substrate-gap/fixtures/ts-typed-receivers`
//! (Kina's and quokka-stack's Angular shapes).
//!
//! An Angular signal query field (`form = viewChild(FilingForm)`,
//! `viewChild<Toggle>('sw')`, `contentChild.required(Dropzone)`; a `read`
//! option yields its token's type) types a read `this.form()`; an inline
//! `inject(T)` types its receiver and is an INJECTS of its first argument from
//! the scope that runs it; a static read `T.X` USES the class; a getter /
//! setter accessed on another typed object is a CALLS of the accessor (rules
//! `receiver_accessor_read` / `receiver_accessor_write`); a `const x = f()`
//! local takes f's declared return type (rule `receiver_return`). Each shape
//! sits beside a look-alike that must stay unbound, three of them (the legacy
//! `Router` rows) a repo class sharing a third-party type's simple name.
//!
//! At HEAD 784d8553 the fixture had CALLS 0/10, INJECTS 0/2, USES 0/2 and 26
//! dead_symbol rows.

use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_code_domain::evidence::{Basis, Evidence};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "ts-typed-receivers";

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

/// The evidence of the first `category` edge `from -> to`, if any.
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

/// The 14 key.json expect_edges, each with the rule that binds it and a
/// fragment of its site line.
#[test]
fn typed_receivers_are_edges() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |rel: &str| node(&m, rel);
    let save = n("pages::filing.page::FilingPage::save");
    let countries = n("pages::register.page::RegisterPage::countries");
    let service = n("core::countries.service::CountriesService");
    let guard = n("core::auth.guard::authGuard");
    let auth = |member: &str| n(&format!("core::auth.service::AuthService::{member}"));
    let init = n("pages::connect.component::ConnectComponent::ngOnInit");
    let spec = n("features::drafts::draft.store.spec");
    let store = |member: &str| n(&format!("features::drafts::draft.store::DraftStore::{member}"));
    let inject_line = "inject(CountriesService).list()";
    let rows = [
        (save, n("shared::filing-form.component::FilingForm::reset"), edge_category::CALLS, "receiver_type", "this.form()?.reset()"),
        (save, n("shared::toggle.component::Toggle::sync"), edge_category::CALLS, "receiver_type", "this.sw()?.sync(true)"),
        (save, n("shared::dropzone.component::Dropzone::clear"), edge_category::CALLS, "receiver_type", "this.zone().clear()"),
        (countries, n("core::countries.service::CountriesService::list"), edge_category::CALLS, "receiver_type", inject_line),
        (countries, service, edge_category::INJECTS, "import_binding", inject_line),
        (countries, service, edge_category::USES, "static_member", "CountriesService.FALLBACK"),
        (guard, n("core::auth.service::AuthService"), edge_category::INJECTS, "import_binding", "inject(AuthService, OPTIONAL)"),
        (guard, auth("isLoggedOut"), edge_category::CALLS, "receiver_accessor_read", "auth.isLoggedOut"),
        (guard, auth("isAuthenticated"), edge_category::CALLS, "receiver_accessor_read", "auth.isAuthenticated"),
        (guard, n("core::auth.guard::OPTIONAL"), edge_category::USES, "value_ref", "inject(AuthService, OPTIONAL)"),
        (init, auth("isAuthenticated"), edge_category::CALLS, "receiver_accessor_read", "this.auth.isAuthenticated"),
        (init, auth("session"), edge_category::CALLS, "receiver_accessor_write", "this.auth.session = 'guest'"),
        (spec, store("reset"), edge_category::CALLS, "receiver_return", "store.reset()"),
        (spec, store("patch"), edge_category::CALLS, "receiver_return", "other.patch('x')"),
    ];
    for (from, to, category, rule, site) in rows {
        let ev = edge_evidence(&m, from, to, category)
            .unwrap_or_else(|| panic!("{site}: no edge {from:?} -> {to:?}"));
        assert_eq!(ev.rule.as_deref(), Some(rule), "{site}: {ev:?}");
        let text = site_text(tmp.path(), &ev);
        assert!(text.contains(site), "{site}: the site line is `{}`", text.trim());
    }
}

/// The 11 key.json forbid edges: a `read` option's token, an untyped and a
/// list query, a class's own static read, a method read by value, a
/// parameter shadowing a factory, an unannotated factory, a project-local
/// `inject`, an `inject` options argument, and every third-party `Router`
/// reading bind nothing.
#[test]
fn look_alikes_bind_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |rel: &str| node(&m, rel);
    let reload = n("pages::filing.page::FilingPage::reload");
    let guard = n("core::auth.guard::authGuard");
    let spec = n("features::drafts::draft.store.spec");
    let drop = n("features::drafts::draft.store::DraftStore::drop");
    let router = n("legacy::router::Router");
    let url = n("legacy::router::Router::url");
    let forbid = [
        (reload, n("shared::dropzone.component::Dropzone::clear"), edge_category::CALLS),
        (n("core::countries.service::LegacyCodes::first"), n("core::countries.service::LegacyCodes"), edge_category::USES),
        (n("core::auth.guard::describeAuth"), n("core::auth.service::AuthService::check"), edge_category::CALLS),
        (n("features::drafts::draft.store.spec::withFactory"), drop, edge_category::CALLS),
        (spec, drop, edge_category::CALLS),
        (n("legacy::probe::probe"), n("core::auth.service::AuthService"), edge_category::INJECTS),
        (reload, n("shared::chip.component::Chip::filter"), edge_category::CALLS),
        (guard, router, edge_category::INJECTS),
        (guard, url, edge_category::CALLS),
        (spec, url, edge_category::CALLS),
        (guard, n("core::auth.guard::OPTIONAL"), edge_category::INJECTS),
    ];
    for (from, to, category) in forbid {
        assert!(edge_evidence(&m, from, to, category).is_none(), "{from:?} -> {to:?} ({category:?})");
    }
    let into_router: Vec<NodeId> = m
        .all_edges()
        .filter(|e| (e.to == router || e.to == url) && e.category != edge_category::DEFINES)
        .map(|e| e.from)
        .collect();
    assert!(into_router.is_empty(), "the legacy Router is used by nothing: {into_router:?}");
}

/// dead_symbol by membership: the 10 members and classes the new edges
/// reach are no rows; the 16 that nothing runs stay.
#[test]
fn typed_receiver_targets_leave_dead_symbol() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    let q = |rel: &str| format!("{APP}::{rel}");
    for gone in [
        "core::auth.service::AuthService::isAuthenticated",
        "core::auth.service::AuthService::isLoggedOut",
        "core::auth.service::AuthService::session",
        "core::countries.service::CountriesService",
        "core::countries.service::CountriesService::list",
        "features::drafts::draft.store::DraftStore::patch",
        "features::drafts::draft.store::DraftStore::reset",
        "shared::dropzone.component::Dropzone::clear",
        "shared::filing-form.component::FilingForm::reset",
        "shared::toggle.component::Toggle::sync",
    ] {
        assert!(!rows.contains(&q(gone)), "{gone} is run, no row: {rows:#?}");
    }
    for stays in [
        "core::auth.guard::authGuard",
        "core::auth.guard::describeAuth",
        "core::auth.service::AuthService::check",
        "core::auth.service::AuthService::unusedFlag",
        "core::countries.service::LegacyCodes",
        "core::countries.service::LegacyCodes::first",
        "features::drafts::draft.store::DraftStore",
        "features::drafts::draft.store::DraftStore::drop",
        "legacy::probe::probe",
        "legacy::router::Router",
        "legacy::router::Router::url",
        "pages::filing.page::FilingPage::reload",
        "pages::filing.page::FilingPage::save",
        "shared::chip.component::Chip::filter",
        "shared::filing-form.component::FilingForm::submit",
        "shared::toggle.component::Toggle::flip",
    ] {
        assert!(rows.contains(&q(stays)), "{stays} is run by nothing, a row: {rows:#?}");
    }
}
