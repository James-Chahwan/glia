//! DT.5a — Angular inline `template:` and `host:` bindings bind the
//! component's own METHODs: a binding that calls a METHOD of its component is
//! a CALLS from the component CLASS (rule `template_call`, `host_call` from a
//! host entry), one that only names it a USES, EVIDENCE
//! `extractor:angular_template` at the binding's row (the lowest row when two
//! bindings name one method). Template locals are scoped: a `@for` loop
//! variable shadows a same-named method in its header (the track expression
//! included) and its block only, and template text binds nothing.
//!
//! The fixture is `bench/substrate-gap/fixtures/angular-inline-template-bindings`,
//! copied to a tempdir. It has no constructor, no receiver call and no
//! decorator argument, so the dead_symbol rows it keeps (`entry`, `reload`,
//! `exportCsv`) hold whatever DT.5c / DT.5d / DT.5g land.
//!
//! DT.5b — a component's `templateUrl` template (a `.component.html`, or any
//! `.html` beside a same-stem `.ts`) binds the same way through a post-cache
//! graft, EVIDENCE at the template's own file and row; the cases are below.

use std::path::Path;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::EdgeCategoryId;
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::watch::WatchSession;
use glia_engine::{BuildOptions, ParseCache, generate_one, generate_one_with_cache};
use glia_graph::MergedGraph;

const FIXTURE: &str = "../bench/substrate-gap/fixtures/angular-inline-template-bindings";
const FILES: [&str; 2] = ["package.json", "src/app/offers/offer-list.component.ts"];
const CLASS: &str = "src::app::offers::offer-list.component::OfferListComponent";

fn build() -> MergedGraph {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let dir = tempfile::tempdir().expect("tempdir");
    for f in FILES {
        let body = std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
        let path = dir.path().join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    generate_one(dir.path().to_str().expect("utf-8 temp path"))
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// Every edge out of the component CLASS of `cat`, as (method simple name,
/// evidence emitter, rule, 0-based line), sorted.
fn class_edges(m: &MergedGraph, cat: EdgeCategoryId) -> Vec<(String, String, String, u32)> {
    let mut out = Vec::new();
    for g in &m.graphs {
        for e in &g.edges {
            if e.category != cat {
                continue;
            }
            let from_is_class = g.nav.qname_by_id.get(&e.from).map(String::as_str) == Some(CLASS)
                && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::CLASS);
            if !from_is_class {
                continue;
            }
            let to = g.nav.qname_by_id.get(&e.to).expect("target qname");
            let ev = Evidence::read(&e.cells).expect("every edge carries EVIDENCE");
            out.push((
                to.rsplit("::").next().unwrap_or(to).to_string(),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.line.expect("located at the binding's row"),
            ));
        }
    }
    out.sort();
    out
}

#[test]
fn template_and_host_bindings_call_the_components_methods() {
    let m = build();
    let calls = class_edges(&m, edge_category::CALLS);
    let at = |name: &str, rule: &str, line: u32| {
        (
            name.to_string(),
            "extractor:angular_template".to_string(),
            rule.to_string(),
            line,
        )
    };
    // 0-based rows: the file's 1-based line minus one.
    assert_eq!(
        calls,
        vec![
            at("canEdit", "template_call", 14),
            at("isBusy", "host_call", 7),
            at("onEscape", "host_call", 7),
            at("refresh", "template_call", 11),
            at("remove", "template_call", 18),
            at("row", "template_call", 12),
            at("summary", "template_call", 15),
            at("trackById", "template_call", 17),
        ]
    );
    // `offers()` is a signal (STATE_VAR), `rows` a plain field: no METHOD, no edge.
    assert!(
        class_edges(&m, edge_category::USES).is_empty(),
        "no USES expected"
    );
}

#[test]
fn shadowed_locals_and_template_text_stay_dead() {
    let m = build();
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let mut dead: Vec<String> = rep
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.rsplit("::").next().unwrap_or(&r.qname).to_string())
        .collect();
    dead.sort();
    assert_eq!(dead, ["entry", "exportCsv", "reload"]);
    for cat in [edge_category::CALLS, edge_category::USES] {
        let names: Vec<String> = class_edges(&m, cat).into_iter().map(|e| e.0).collect();
        for gone in ["entry", "reload", "exportCsv"] {
            assert!(
                !names.iter().any(|n| n == gone),
                "{gone} must not be bound ({cat:?})"
            );
        }
    }
}

#[test]
fn two_builds_give_identical_template_edges() {
    let a = build();
    let b = build();
    for cat in [edge_category::CALLS, edge_category::USES] {
        assert_eq!(class_edges(&a, cat), class_edges(&b, cat));
    }
}

// ---------------------------------------------------------------------------
// DT.5b — `templateUrl` templates
// ---------------------------------------------------------------------------
//
// The fixture is `bench/substrate-gap/fixtures/angular-template-url-bindings`
// (its `web/` dir is the repo): `OrdersPage` names `./orders.page.html` (an
// Angular-style `<name>.page.html`, legacy *ngFor / trackBy), and
// `ProfileComponent` a `.component.html`. The walk admits a `.html` whose
// same-stem `.ts` sibling exists; the post-cache graft binds the template's
// member refs from the component CLASS, EVIDENCE at the template's file and
// row. `legacy/widget.html` sits beside `widget.ts` but no `templateUrl`
// names it, and `index.html` has no `.ts` sibling.

const URL_FIXTURE: &str = "../bench/substrate-gap/fixtures/angular-template-url-bindings/web";
const URL_FILES: [&str; 10] = [
    "package.json",
    "src/index.html",
    "src/main.ts",
    "src/app/app.module.ts",
    "src/app/orders/orders.page.html",
    "src/app/orders/orders.page.ts",
    "src/app/profile/profile.component.html",
    "src/app/profile/profile.component.ts",
    "src/legacy/widget.html",
    "src/legacy/widget.ts",
];
const ORDERS: &str = "src::app::orders::orders.page::OrdersPage";
const PROFILE: &str = "src::app::profile::profile.component::ProfileComponent";
const WIDGET: &str = "src::legacy::widget::Widget";

/// Copy the fixture's `web/` files for which `keep` holds into `dir`.
fn write_url_fixture(dir: &Path, keep: impl Fn(&str) -> bool) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(URL_FIXTURE);
    for f in URL_FILES.into_iter().filter(|f| keep(f)) {
        let body = std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
        let path = dir.join(f);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
}

fn url_build(keep: impl Fn(&str) -> bool) -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    write_url_fixture(dir.path(), keep);
    generate_one(dir.path().to_str().expect("utf-8 temp path"))
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// Every edge of `cat` out of CLASS `class`, as (method simple name, emitter,
/// rule, evidence file, 0-based line), sorted.
fn edges_from(
    m: &MergedGraph,
    class: &str,
    cat: EdgeCategoryId,
) -> Vec<(String, String, String, String, u32)> {
    let mut out = Vec::new();
    for g in &m.graphs {
        for e in &g.edges {
            let from_is_class = g.nav.qname_by_id.get(&e.from).map(String::as_str) == Some(class)
                && g.nav.kind_by_id.get(&e.from) == Some(&node_kind::CLASS);
            if e.category != cat || !from_is_class {
                continue;
            }
            let to = g.nav.qname_by_id.get(&e.to).expect("target qname");
            let ev = Evidence::read(&e.cells).expect("every edge carries EVIDENCE");
            out.push((
                to.rsplit("::").next().unwrap_or(to).to_string(),
                ev.emitter,
                ev.rule.unwrap_or_default(),
                ev.file.unwrap_or_default(),
                ev.line.expect("located at the binding's row"),
            ));
        }
    }
    out.sort();
    out
}

fn module_qnames(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == node_kind::MODULE)
                .filter_map(|(id, _)| g.nav.qname_by_id.get(id).cloned())
        })
        .collect();
    out.sort();
    out
}

fn bound(name: &str, rule: &str, file: &str, line: u32) -> (String, String, String, String, u32) {
    (
        name.to_string(),
        "extractor:angular_template".to_string(),
        rule.to_string(),
        file.to_string(),
        line,
    )
}

#[test]
fn template_url_templates_bind_their_components_methods() {
    let m = url_build(|_| true);
    let orders = "src/app/orders/orders.page.html";
    assert_eq!(
        edges_from(&m, ORDERS, edge_category::CALLS),
        vec![
            bound("cancel", "template_call", orders, 4),
            bound("isLocked", "template_call", orders, 3),
            bound("pay", "template_call", orders, 3),
            bound("totalLabel", "template_call", orders, 2),
        ],
        "`archive(order)` sits only in an HTML comment"
    );
    assert_eq!(
        edges_from(&m, ORDERS, edge_category::USES),
        vec![bound("trackByOrder", "template_ref", orders, 1)]
    );
    assert_eq!(
        edges_from(&m, PROFILE, edge_category::CALLS),
        vec![bound(
            "save",
            "template_call",
            "src/app/profile/profile.component.html",
            0
        )],
        "`reset` is never bound"
    );
    assert!(edges_from(&m, PROFILE, edge_category::USES).is_empty());
    for cat in [edge_category::CALLS, edge_category::USES] {
        assert!(
            edges_from(&m, WIDGET, cat).is_empty(),
            "widget.html is no component's templateUrl ({cat:?})"
        );
    }
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let dead: Vec<&str> = rep
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.as_str())
        .collect();
    for live in ["cancel", "isLocked", "pay", "totalLabel", "trackByOrder"] {
        let q = format!("{ORDERS}::{live}");
        assert!(
            !dead.contains(&q.as_str()),
            "{q} is bound from its template"
        );
    }
    assert!(!dead.contains(&format!("{PROFILE}::save").as_str()));
    for gone in [
        format!("{ORDERS}::archive"),
        format!("{PROFILE}::reset"),
        format!("{WIDGET}::open"),
    ] {
        assert!(
            dead.contains(&gone.as_str()),
            "{gone} stays a dead_symbol row: {dead:?}"
        );
    }
}

#[test]
fn a_walked_template_mints_no_module_and_moves_no_qname() {
    let with = module_qnames(&url_build(|_| true));
    assert_eq!(
        with.iter()
            .filter(|q| q.as_str() == "src::app::orders::orders.page")
            .count(),
        1,
        "{with:?}"
    );
    assert!(!with.iter().any(|q| q.ends_with(".html")), "{with:?}");
    let without = module_qnames(&url_build(|f| !f.ends_with(".html")));
    assert_eq!(with, without, "the html files move no MODULE qname");
}

#[test]
fn an_edit_to_the_template_alone_rebinds_a_cached_component() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_url_fixture(dir.path(), |_| true);
    let repo = dir.path().to_str().expect("utf-8 temp path");
    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(repo, &mut cache)
        .expect("cold build")
        .merged;
    assert_eq!(edges_from(&cold, ORDERS, edge_category::CALLS).len(), 4);
    // Only the template changes: `cancel` leaves it, `archive` comes out of
    // the comment and a `refresh()` that is no METHOD binds nothing.
    let html = dir.path().join("src/app/orders/orders.page.html");
    let text = std::fs::read_to_string(&html).expect("read template");
    let edited = text
        .replace(
            "<button (click)=\"cancel(order)\">Cancel</button>",
            "<button (click)=\"refresh()\">Reload</button>",
        )
        .replace(
            "<!-- archive(order) once the archive endpoint ships -->",
            "<button (click)=\"archive(order)\">Archive</button>",
        );
    assert_ne!(edited, text);
    std::fs::write(&html, edited).expect("write template");
    let warm = generate_one_with_cache(repo, &mut cache)
        .expect("warm build")
        .merged;
    let clean = generate_one(repo).expect("clean build").merged;
    for (class, cat) in [
        (ORDERS, edge_category::CALLS),
        (ORDERS, edge_category::USES),
        (PROFILE, edge_category::CALLS),
    ] {
        assert_eq!(
            edges_from(&warm, class, cat),
            edges_from(&clean, class, cat),
            "{class} {cat:?}"
        );
    }
    let calls: Vec<String> = edges_from(&warm, ORDERS, edge_category::CALLS)
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(calls, ["archive", "isLocked", "pay", "totalLabel"]);
}

/// A watch session's Low update of the template alone (the html is in its
/// walk table) re-runs the graft over the cache-served component and equals a
/// clean build of the edited tree.
#[test]
fn a_watch_update_of_the_template_equals_a_clean_build() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_url_fixture(dir.path(), |_| true);
    let repo = dir.path().to_str().expect("utf-8 temp path");
    let mut session = WatchSession::open(repo, &BuildOptions::default()).expect("open");
    let html = dir.path().join("src/app/orders/orders.page.html");
    let text = std::fs::read_to_string(&html).expect("read template");
    std::fs::write(&html, text.replace("pay(order)", "cancel(order)")).expect("write template");
    session.apply(std::slice::from_ref(&html)).expect("apply");
    let clean = generate_one(repo).expect("clean build").merged;
    for (class, cat) in [
        (ORDERS, edge_category::CALLS),
        (ORDERS, edge_category::USES),
        (PROFILE, edge_category::CALLS),
    ] {
        assert_eq!(
            edges_from(&session.result().merged, class, cat),
            edges_from(&clean, class, cat),
            "{class} {cat:?}"
        );
    }
    let calls: Vec<String> = edges_from(&clean, ORDERS, edge_category::CALLS)
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(calls, ["cancel", "isLocked", "totalLabel"]);
}
