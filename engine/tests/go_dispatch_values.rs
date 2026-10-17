//! DT.5j — Go receiver typing, func / method values and package-function
//! handlers, end to end through `generate_one` over
//! `bench/substrate-gap/fixtures/go-dispatch-values` (the Kina /
//! quokka-stack dead_symbol false positives, Go cluster).
//!
//! A struct field keeps its package qualifier, read through the DECLARING
//! file's imports (`store docstore.Store` beside a second interface `Store`;
//! `Ledger`'s `reg` is tokens in ledger.go and screening in ledger_ops.go); a
//! `range` value variable is an element of its container (a slice field, a
//! map field's value), so interface dispatch binds the interface METHOD that
//! CH.1c's dead-dispatch rule reads; a func-literal local types its call and
//! a local two declarations agree on is typed (`o := NewOXR(id)` /
//! `o := src(..)`); a composite-literal receiver `(&pkg.T{..}).M()` is a value
//! of T; a method or func passed or used as a package var's initialiser by
//! value is a USES (rules `method_value` / `func_value`), never through a
//! name a local, a named result or a body `const` shadows; a `pkg.F` route
//! handler in another file of the imported package is HANDLED_BY (rule
//! `package_function`). The 2026-10-17 correction: a composite literal USES
//! the in-repo STRUCT it builds (rule `composite_literal`), so the runtime's
//! `Error() string` on a STRUCT only literals build leaves dead_symbol
//! (DT.5l's go_runtime row reads the owner's incoming carry edge).
//!
//! At HEAD 45fd6f62 the fixture had CALLS 1/12, USES 0/5, HANDLED_BY 1/3,
//! FORBID 0 and 31 dead_symbol rows.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_code_domain::edge_category;
use glia_code_domain::evidence::{Basis, Evidence};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const FIXTURE: &str = "go-dispatch-values";

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

/// The one node whose qname is exactly `qname`.
fn node(m: &MergedGraph, qname: &str) -> NodeId {
    let ids = m.qnames_exact(qname);
    assert_eq!(ids.len(), 1, "one node `{qname}`: {ids:?}");
    ids[0]
}

/// The evidence of the first `category` edge `from -> to`, if any.
fn edge_evidence(
    m: &MergedGraph,
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
) -> Option<Evidence> {
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

const HOOK: &str = "graph:go_packages";

/// The key.json expect_edges, each with the (emitter, rule) that binds it and
/// a fragment of its site line.
#[test]
fn dispatch_and_value_edges_bind() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |q: &str| node(&m, q);
    let (calls, uses, handled) =
        (edge_category::CALLS, edge_category::USES, edge_category::HANDLED_BY);
    let main = "cmd::server::main::main";
    let refusal_test = "apperr::apperr_test::TestRefusalText";
    let rows = [
        ("services::service::Service::Upload", "docstore::docstore::Store::Put", calls, HOOK, "receiver_field_chain", "s.store.Put(key)"),
        ("services::service::Service::Create", "tokens::registry::Registry::ActivePair", calls, HOOK, "receiver_field_chain", "s.tokens.ActivePair(pair)"),
        ("services::service::Service::Create", "screening::registry::Registry::Get", calls, HOOK, "receiver_field_chain", "s.lists.Get(pair)"),
        ("services::ledger_ops::Ledger::Pairs", "tokens::registry::Registry::ActivePair", calls, HOOK, "receiver_field_chain", "l.pairs.ActivePair(p)"),
        ("services::service::Service::Reasons", "services::service::SelfCheck::Check", calls, HOOK, "receiver_local", "c.Check(id)"),
        ("services::service::Service::Reasons", "services::service::SelfCheck::Name", calls, HOOK, "receiver_local", "c.Name()"),
        ("services::service::Service::Lookup", "services::service::SelfCheck::Name", calls, HOOK, "receiver_local", "return c.Name()"),
        ("fx::oxr_test::TestOXR", "fx::oxr::OXR::Provider", calls, HOOK, "receiver_local", "o.Provider()"),
        (refusal_test, "services::refusal::Refusal::Error", calls, HOOK, "receiver_return", "(&services.Refusal{"),
        (refusal_test, "services::refusal::Refusal::Text", calls, HOOK, "receiver_return", "(&services.Refusal{"),
        (main, "services::values::Topic::Describe", calls, HOOK, "receiver_return", "services.Topic{}.Describe()"),
        (main, "services::service::Service::Upload", calls, HOOK, "receiver_local", "svc.Upload("),
        (main, "services::service::Service::OnDone", uses, HOOK, "method_value", "idx.SetOnCompleted(svc.OnDone)"),
        ("indexer::indexer::defaultDone", "indexer::indexer::logDone", uses, HOOK, "func_value", "var defaultDone = logDone"),
        ("GET /metrics", "controllers::metrics::MetricsHandler", handled, HOOK, "package_function", "controllers.MetricsHandler"),
        ("GET /healthz", "controllers::health::HealthHandler", handled, HOOK, "package_function", "controllers.HealthHandler"),
        ("GET /ping", "controllers::activity::Ping", handled, "graph:refs", "attribute", "controllers.Ping"),
        // The 2026-10-17 correction: a composite literal USES its STRUCT.
        ("apperr::apperr::notAvailable", "apperr::apperr::Error", uses, HOOK, "composite_literal", "var notAvailable = &Error{"),
        ("indexer::indexer::New", "indexer::indexer::Indexer", uses, HOOK, "composite_literal", "return &Indexer{done: defaultDone}"),
        (main, "services::ledger::Ledger", uses, HOOK, "composite_literal", "(&services.Ledger{}).Pairs"),
    ];
    for (from, to, category, emitter, rule, site) in rows {
        let ev = edge_evidence(&m, n(from), n(to), category)
            .unwrap_or_else(|| panic!("{site}: no edge {from} -> {to}"));
        assert_eq!((ev.emitter.as_str(), ev.rule.as_deref()), (emitter, Some(rule)), "{site}: {ev:?}");
        let text = site_text(tmp.path(), &ev);
        assert!(text.contains(site), "{site}: the site line is `{}`", text.trim());
    }
}

/// The key.json forbid edges: the other Store / Registry, the call site
/// file's own `reg`, a map's key, a local whose declarations disagree, a
/// same-named type of the test's own directory, an unpassed method, a
/// goroutine call, a local func value, a named result and a body const
/// shadowing the package func, an unregistered handler, and the composite
/// literal of another package's same-named type.
#[test]
fn look_alikes_bind_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let n = |q: &str| node(&m, q);
    let (calls, uses, handled) =
        (edge_category::CALLS, edge_category::USES, edge_category::HANDLED_BY);
    let main = "cmd::server::main::main";
    let refusal_test = "apperr::apperr_test::TestRefusalText";
    let forbid = [
        ("services::service::Service::Upload", "fx::store::Store::Put", calls),
        ("services::service::Service::Create", "screening::registry::Registry::ActivePair", calls),
        ("services::ledger_ops::Ledger::Pairs", "screening::registry::Registry::ActivePair", calls),
        ("services::values::Rules::Topics", "services::values::Rule::Describe", calls),
        ("fx::oxr_test::TestMixed", "fx::oxr::Other::Provider", calls),
        (refusal_test, "services::refusal::Refusal::Detail", calls),
        (refusal_test, "apperr::apperr::Refusal::Text", calls),
        (main, "services::service::Service::OnFail", uses),
        (main, "indexer::indexer::Indexer::Run", uses),
        (main, "cmd::server::main::notify", uses),
        ("services::values::Wrap", "services::values::handle", uses),
        ("services::values::Pick", "services::values::handle", uses),
        ("GET /metrics", "controllers::metrics::MetricsDebug", handled),
        (refusal_test, "apperr::apperr::Refusal", uses),
    ];
    for (from, to, category) in forbid {
        assert!(edge_evidence(&m, n(from), n(to), category).is_none(), "{from} -> {to}");
    }
    // `handle` is referenced only through shadowing names: nothing but its
    // MODULE's DEFINES reaches it.
    let handle = n("services::values::handle");
    let into: Vec<NodeId> = m
        .all_edges()
        .filter(|e| e.to == handle && e.category != edge_category::DEFINES)
        .map(|e| e.from)
        .collect();
    assert!(into.is_empty(), "handle is referenced by nothing: {into:?}");
}

/// dead_symbol, exactly: the 18 rows the bound edges reach are gone, the 13
/// truly unused members stay, and no other row appears.
#[test]
fn fixed_rows_leave_dead_symbol() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path());
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let rows: BTreeSet<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    for gone in [
        "apperr::apperr::Error::Error",
        "controllers::health::HealthHandler",
        "controllers::metrics::MetricsHandler",
        "docstore::docstore::LocalStore::Put",
        "fx::oxr::OXR::Provider",
        "indexer::indexer::logDone",
        "screening::registry::Registry::Get",
        "services::ledger_ops::Ledger::Pairs",
        "services::refusal::Refusal::Error",
        "services::refusal::Refusal::Text",
        "services::service::Service::OnDone",
        "services::service::kycCheck::Check",
        "services::service::kycCheck::Name",
        "services::service::reviewCheck::Check",
        "services::service::reviewCheck::Name",
        "services::values::Rules::Topics",
        "services::values::Topic::Describe",
        "tokens::registry::Registry::ActivePair",
    ] {
        assert!(!rows.contains(gone), "{gone} is reached, no row: {rows:#?}");
    }
    let stays: BTreeSet<String> = [
        "apperr::apperr::Refusal::Text",
        "cmd::server::main::notify",
        "controllers::metrics::MetricsDebug",
        "docstore::docstore::LocalStore::Purge",
        "fx::oxr::Other::Provider",
        "fx::store::MemStore::Put",
        "screening::registry::Registry::ActivePair",
        "services::refusal::Refusal::Detail",
        "services::service::Service::OnFail",
        "services::service::tag::Label",
        "services::values::Rule::Describe",
        "services::values::handle",
        "tokens::registry::Registry::HasPair",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(rows, stays, "exactly the unused members are rows");
}
