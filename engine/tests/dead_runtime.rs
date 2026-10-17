//! DT.5l — entries and members the runtime invokes with no source line
//! calling them leave dead_symbol, beside look-alikes that stay dead.
//!
//! - A Go `func init()` is an entry: the code domain's entry rule has a named
//!   row limited to `.go` files (`NamedEntry.exts`), so a TS or Python
//!   function named `init` and a Go METHOD named `init` stay ordinary names.
//! - `INVOKED_MEMBERS` gains signature-checked Go runtime-interface rows
//!   (`String` / `MarshalJSON` of a live Point; `Point::Scan() bool` is no
//!   `sql.Scanner`, a lone `Queue::Len() int` is no `sort.Interface`), an
//!   `@main` app-delegate row, and external-base rows read off the class's
//!   unbound heritage ref plus the library its file imports (a Transloco
//!   loader, an Angular ErrorHandler, a dio Interceptor).
//! - A Swift `@main` type and a Kotlin `FlutterActivity` under an
//!   `io.flutter` import carry ROLE COMPONENT, so they are entries.
//! - The calls in a Python `if __name__ == "__main__":` guard are the
//!   MODULE's, the MODULE carries an ENTRYPOINT cell (`source` `main_guard`),
//!   and a bare call binds the caller's own nested def first (LEGB).
//!
//! The fixture is `bench/substrate-gap/fixtures/runtime-invoked-entries`
//! (go, web, admin, mobile, scripts), copied into a tempdir and built with
//! `generate_many`. At tag 0.5.2-complete it had 30 dead_symbol rows and
//! `[live] profile=code entrypoints=4 (kind=2 role=2 declared=0) owners=2
//! implementers=0 invoked=3 reached=19/80`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, GapsReport, gaps_report};
use glia_engine::persist::{load_layout, persist_result};
use glia_engine::{GenerateResult, entrypoint_reachable, generate_many};
use glia_graph::MergedGraph;

const FIXTURE: &str = "runtime-invoked-entries";
const DIRS: [&str; 5] = ["go", "web", "admin", "mobile", "scripts"];

const MAIN_ACTIVITY: &str = "android::app::src::main::kotlin::com::example::app::MainActivity";
const SHADOW: &str = "android::app::src::main::kotlin::com::example::app::Shadow";
const HTTP_LOADER: &str = "src::app::i18n::http.loader::HttpLoader";
const AUTH: &str = "lib::http::auth_interceptor::AuthInterceptor";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures")
        .join(FIXTURE)
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

/// The fixture copied under `tmp` and built as one merge of its five repos.
fn build(tmp: &Path) -> GenerateResult {
    copy_tree(&fixture_dir(), tmp);
    let roots: Vec<String> = DIRS
        .iter()
        .map(|d| tmp.join(d).display().to_string())
        .collect();
    generate_many(&roots).unwrap_or_else(|e| panic!("generate_many: {e}"))
}

fn report(m: &MergedGraph) -> GapsReport {
    gaps_report(m, &[], &GapsOptions::default()).expect("known categories")
}

/// The dead_symbol rows' qnames, sorted.
fn dead(rep: &GapsReport) -> Vec<String> {
    let mut rows: Vec<String> = rep
        .rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.clone())
        .collect();
    rows.sort();
    rows
}

/// The one node whose qname is exactly `qname`. A Swift file's MODULE and
/// the type it declares share a qname (`ios::Runner::AppDelegate`: the type
/// is module-scoped, LB.7c), so the MODULE gives way to a declaration.
fn node(m: &MergedGraph, qname: &str) -> NodeId {
    let mut ids = m.qnames_exact(qname);
    if ids.len() > 1 {
        let kind = |id: &NodeId| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.kind_by_id.get(id).copied())
        };
        ids.retain(|id| kind(id) != Some(node_kind::MODULE));
    }
    assert_eq!(ids.len(), 1, "one node `{qname}`: {ids:?}");
    ids[0]
}

/// The live set as sorted qnames.
fn live_qnames(m: &MergedGraph) -> BTreeSet<String> {
    let live = entrypoint_reachable(m);
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| live.contains(&n.id))
                .map(|n| n.id)
        })
        .filter_map(|id| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        })
        .collect()
}

/// The 20 rows the fix leaves: every look-alike, the module-level `double`
/// the nested def took its only caller from, and the uncalled script defs.
const DEAD: [&str; 20] = [
    SHADOW,
    "geo::geo::Orphan::String",
    "geo::geo::Point::Scan",
    "geo::geo::Point::Stringify",
    "geo::geo::Point::init",
    "geo::geo::Queue::Len",
    "ios::Runner::AppDelegate::helper",
    "ios::Runner::SceneHelper",
    "ios::Runner::SceneHelper::application",
    "lib::http::auth_interceptor::AuthInterceptor::onRequestLater",
    "lib::http::logger::Logger::onRequest",
    "notmain::work",
    "src::app::core::boot::init",
    "src::app::i18n::fake.loader::FakeLoader::getTranslation",
    "src::app::i18n::http.loader::HttpLoader::getTranslationLater",
    "src::app::i18n::local.loader::LocalLoader::getTranslation",
    "tool::compute::unused_inner",
    "tool::double",
    "tool::helper",
    "tool::init",
];

#[test]
fn dead_rows_are_exactly_the_lookalikes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    assert_eq!(dead(&report(&m)), DEAD, "30 rows at the tag, 20 after");
}

#[test]
fn runtime_entries_and_members_are_live() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let live = entrypoint_reachable(&m);
    for q in [
        "geo::geo::init",
        "geo::geo::register",
        "geo::geo::Point::String",
        "geo::geo::Point::MarshalJSON",
        "geo::geo::Queue::Push",
        "src::app::i18n::http.loader::HttpLoader::getTranslation",
        "src::app::core::error.handler::AppErrorHandler::handleError",
        "lib::http::auth_interceptor::AuthInterceptor::onRequest",
        "ios::Runner::AppDelegate",
        "ios::Runner::AppDelegate::application",
        MAIN_ACTIVITY,
        "tool",
        "tool::compute",
        "tool::compute::double",
    ] {
        assert!(live.contains(&node(&m, q)), "{q} is live");
    }
    for q in [
        "geo::geo::Point::Scan",
        "geo::geo::Point::Stringify",
        "geo::geo::Point::init",
        "geo::geo::Queue::Len",
        "geo::geo::Orphan::String",
        "src::app::i18n::fake.loader::FakeLoader::getTranslation",
        "src::app::i18n::http.loader::HttpLoader::getTranslationLater",
        "src::app::i18n::local.loader::LocalLoader::getTranslation",
        "lib::http::logger::Logger::onRequest",
        "lib::http::auth_interceptor::AuthInterceptor::onRequestLater",
        "ios::Runner::AppDelegate::helper",
        "ios::Runner::SceneHelper",
        "ios::Runner::SceneHelper::application",
        SHADOW,
        "src::app::core::boot::init",
        "tool::init",
        "tool::double",
        "tool::helper",
        "notmain::work",
    ] {
        assert!(
            !live.contains(&node(&m, q)),
            "{q} stays outside the live set"
        );
    }
}

/// Every hop is walk-local: none is an edge, and liveness adds none.
#[test]
fn no_hop_is_stored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let before = m.all_edges().count();
    let _ = entrypoint_reachable(&m);
    let _ = report(&m);
    assert_eq!(m.all_edges().count(), before);
    for (from, to) in [
        ("geo::geo::Point", "geo::geo::Point::String"),
        (
            HTTP_LOADER,
            "src::app::i18n::http.loader::HttpLoader::getTranslation",
        ),
        (
            "ios::Runner::AppDelegate",
            "ios::Runner::AppDelegate::application",
        ),
        (
            AUTH,
            "lib::http::auth_interceptor::AuthInterceptor::onRequest",
        ),
    ] {
        let (f, t) = (node(&m, from), node(&m, to));
        assert!(
            !m.all_edges().any(|e| e.from == f
                && e.to == t
                && (e.category == edge_category::CALLS || e.category == edge_category::USES)),
            "no stored edge {from} -> {to}"
        );
    }
}

/// The CALLS edge `from -> to` with its EVIDENCE rule, if one is stored.
fn calls_rule(m: &MergedGraph, from: &str, to: &str) -> Option<String> {
    let (f, t) = (node(m, from), node(m, to));
    m.all_edges()
        .find(|e| e.from == f && e.to == t && e.category == edge_category::CALLS)
        .map(|e| Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default())
}

/// The ENTRYPOINT payloads of the node `qname`.
fn entry_cells(m: &MergedGraph, qname: &str) -> Vec<String> {
    let id = node(m, qname);
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| n.id == id))
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ENTRYPOINT)
        .map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => s.clone(),
            CellPayload::Bytes(_) => String::new(),
        })
        .collect()
}

#[test]
fn python_script_shapes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    assert_eq!(
        calls_rule(&m, "tool", "tool::compute").as_deref(),
        Some("intra_file")
    );
    assert_eq!(
        calls_rule(&m, "tool::compute", "tool::compute::double").as_deref(),
        Some("nested_def")
    );
    assert!(
        calls_rule(&m, "tool::compute", "tool::double").is_none(),
        "the nested def shadows it"
    );
    assert!(
        calls_rule(&m, "tool", "tool::helper").is_none(),
        "a def inside the guard is not walked"
    );
    assert!(
        calls_rule(&m, "notmain", "notmain::work").is_none(),
        "`name == ..` is no guard"
    );
    assert!(
        m.qnames_exact("tool::later").is_empty(),
        "no node for the guard's inner def"
    );
    assert_eq!(
        entry_cells(&m, "tool"),
        [r#"{"decl":"tool.py:26","source":"main_guard"}"#]
    );
    assert!(entry_cells(&m, "notmain").is_empty());
}

#[test]
fn roles() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let roles = |q: &str| -> Vec<glia_core::NodeKindId> {
        let id = node(&m, q);
        m.graphs
            .iter()
            .flat_map(|g| {
                g.nodes.iter().filter(move |n| n.id == id).map(move |n| {
                    glia_graph::roles::roles_in(g.nav.kind_by_id.get(&n.id).copied(), &n.cells)
                })
            })
            .flatten()
            .collect()
    };
    assert!(
        roles("ios::Runner::AppDelegate").contains(&node_kind::COMPONENT),
        "@main"
    );
    assert!(
        roles(MAIN_ACTIVITY).contains(&node_kind::COMPONENT),
        "FlutterActivity"
    );
    assert!(!roles("ios::Runner::SceneHelper").contains(&node_kind::COMPONENT));
    assert!(
        !roles(SHADOW).contains(&node_kind::COMPONENT),
        "FlutterActivity from elsewhere"
    );
}

/// Every input the rules read is persisted (cells, unresolved refs,
/// children_of, CODE text), never build-time nav: a loaded layout gives the
/// same rows and the same live set.
#[test]
fn signals_survive_a_layout_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = build(tmp.path());
    let built = (dead(&report(&r.merged)), live_qnames(&r.merged));
    let dir = tmp.path().join("layout");
    persist_result(&r, &dir, "test").expect("persist");
    let loaded = load_layout(&dir).expect("load").merged;
    assert_eq!(dead(&report(&loaded)), DEAD);
    assert_eq!((dead(&report(&loaded)), live_qnames(&loaded)), built);
}

#[test]
fn two_reports_over_one_graph_agree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let (a, b) = (report(&m), report(&m));
    assert_eq!(a, b, "whole report");
    assert_eq!(entrypoint_reachable(&m), entrypoint_reachable(&m));
}

const CHILD_ENV: &str = "GLIA_DT5L_CHILD_REPORT";

/// The child half of [`child_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds the fixture under that dir and
/// reports its dead_symbol rows, so its stderr (with `--nocapture`) carries
/// the markers.
#[test]
fn child_report_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let m = build(Path::new(&dir)).merged;
        let mut opts = GapsOptions::default();
        opts.category = Some(DEAD_SYMBOL.to_string());
        gaps_report(&m, &[], &opts).expect("report");
    }
}

/// The stderr of one child build + report.
fn child_stderr() -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            "child_report_for_stderr",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, tmp.path())
        .output()
        .expect("re-run the test binary");
    assert!(
        out.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn markers() {
    let lines = child_stderr();
    let with = |prefix: &str| -> Vec<&str> {
        lines
            .iter()
            .map(String::as_str)
            .filter(|l| l.starts_with(prefix))
            .collect()
    };
    assert_eq!(
        with("[live] profile="),
        [
            "[live] profile=code entrypoints=8 (kind=3 role=4 declared=1) owners=2 implementers=0 invoked=9 reached=32/80"
        ],
        "kind: two mains and geo's init; role: two Angular components, @main, MainActivity; declared: tool"
    );
    assert_eq!(
        with("[invoked]"),
        [
            "[invoked] rows=10 angular=1 constructor=3 flutter=0 go_runtime=3 apple_app=1 transloco=1 dio=1 sig_rejected=2"
        ],
        "Point::Scan refused by its signature, Queue::Len by its missing siblings"
    );
    assert_eq!(
        with("[swift-app]"),
        ["[swift-app] entry types=1 file=ios/Runner/AppDelegate.swift"]
    );
    let kotlin = with("[kotlin/retrofit]");
    assert_eq!(kotlin.len(), 1, "{kotlin:?}");
    assert!(
        kotlin[0].starts_with("[kotlin/retrofit] endpoints=0 components=1 spring_clients=0 repo="),
        "{kotlin:?}"
    );
    assert_eq!(
        with("[py-main]"),
        ["[py-main] guard module=tool calls=3 file=tool.py"]
    );
    assert_eq!(
        with("[py-nested]"),
        ["[py-nested] nested-def calls bound=1 file=tool.py"]
    );
}
