//! GD.10 (FD.39) — the DT.5l / DT.5e liveness residuals, on the fixture
//! `bench/substrate-gap/fixtures/liveness-close-out-residuals` (one repo:
//! app/ Dart, go/ Go, web/ TypeScript), copied into a tempdir and built with
//! `generate_one`.
//!
//! - Dart: `Home extends BaseScreen`, `BaseScreen extends StatelessWidget` in
//!   the repo. graph::roles' `inherited_widget` lift gives Home ROLE
//!   COMPONENT, so Home, its constructor and its `build` are live; Plain,
//!   which extends nothing Flutter, stays dead.
//! - Go: a `func init()` is an entry only once its package is in use: used's
//!   (main imports it and calls Run) is live with what it calls; orphan's (no
//!   package imports orphan) is dead with its `setup`.
//! - TS: `AliasLoader implements TL` (`TranslocoLoader as TL` from
//!   `@jsverse/transloco`) and `AppErrorHandler extends BaseHandler`
//!   (`BaseHandler extends ErrorHandler` from `@angular/core`): their
//!   `getTranslation` / `handleError` are the library's invoked members.
//!
//! At the packet's base (0.5.6 wave 0) the fixture gave 7 dead_symbol rows
//! (Home, Home::Home, Home::build, Plain, Plain::build,
//! AppErrorHandler::handleError, AliasLoader::getTranslation), orphan's init
//! and setup live, and `[live] profile=code entrypoints=5 (kind=3 role=2
//! declared=0) owners=0 implementers=0 invoked=2 reached=13/35`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::node_kind;
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, GapsReport, gaps_report};
use glia_engine::persist::{load_layout, persist_result};
use glia_engine::{GenerateResult, entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const FIXTURE: &str = "liveness-close-out-residuals";

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

fn build(tmp: &Path) -> GenerateResult {
    copy_tree(&fixture_dir(), tmp);
    generate_one(&tmp.display().to_string()).unwrap_or_else(|e| panic!("generate_one: {e}"))
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

/// The live set as sorted qnames.
fn live_qnames(m: &MergedGraph) -> BTreeSet<String> {
    let live = entrypoint_reachable(m);
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(|n| live.contains(&n.id))
                .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect()
}

/// Plain is no widget; orphan's init is no entry (`setup` has its call, so
/// gaps withholds it, but it is not live).
const DEAD: [&str; 3] = [
    "app::lib::home::Plain",
    "app::lib::home::Plain::build",
    "go::orphan::orphan::init",
];

const LIVE: [&str; 9] = [
    "app::lib::home::Home",
    "app::lib::home::Home::Home",
    "app::lib::home::Home::build",
    "go::main::main",
    "go::used::used::init",
    "go::used::used::prepare",
    "go::used::used::Run",
    "web::src::app::core::app.handler::AppErrorHandler::handleError",
    "web::src::app::i18n::alias.loader::AliasLoader::getTranslation",
];

const NOT_LIVE: [&str; 4] = [
    "app::lib::home::Plain",
    "app::lib::home::Plain::build",
    "go::orphan::orphan::init",
    "go::orphan::orphan::setup",
];

#[test]
fn dead_rows_are_the_lookalikes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    assert_eq!(dead(&report(&m)), DEAD);
}

#[test]
fn residual_entries_and_members_are_live() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let live = live_qnames(&m);
    for q in LIVE {
        assert!(live.contains(q), "{q} live: {live:?}");
    }
    for q in NOT_LIVE {
        assert!(!live.contains(q), "{q} not live");
    }
}

#[test]
fn derived_widget_carries_the_component_role() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build(tmp.path()).merged;
    let roles = |q: &str| -> Vec<glia_core::NodeKindId> {
        let ids = m.qnames_exact(q);
        assert_eq!(ids.len(), 1, "one node `{q}`");
        m.graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(|n| n.id == ids[0])
                    .flat_map(move |n| {
                        glia_graph::roles::roles_in(g.nav.kind_by_id.get(&n.id).copied(), &n.cells)
                    })
            })
            .collect()
    };
    assert_eq!(roles("app::lib::home::Home"), [node_kind::COMPONENT]);
    assert_eq!(roles("app::lib::base::BaseScreen"), [node_kind::COMPONENT]);
    assert!(roles("app::lib::home::Plain").is_empty());
    assert!(
        roles("web::src::app::core::app.handler::AppErrorHandler").is_empty(),
        "the lift is Dart's alone"
    );
}

/// Every input is persisted: a loaded layout gives the same rows, live set
/// and roles.
#[test]
fn survives_a_layout_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = build(tmp.path());
    let built = (dead(&report(&r.merged)), live_qnames(&r.merged));
    let dir = tmp.path().join("layout");
    persist_result(&r, &dir, "test").expect("persist");
    let loaded = load_layout(&dir).expect("load").merged;
    assert_eq!((dead(&report(&loaded)), live_qnames(&loaded)), built);
}

const CHILD_ENV: &str = "GLIA_GD10_CHILD_REPORT";

/// The child half of [`child_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds the fixture under that dir and
/// reports its dead_symbol rows, so its stderr carries the markers.
#[test]
fn child_report_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        let m = build(Path::new(&dir)).merged;
        let mut opts = GapsOptions::default();
        opts.category = Some(DEAD_SYMBOL.to_string());
        gaps_report(&m, &[], &opts).expect("report");
    }
}

fn child_stderr() -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args([
            "--exact",
            &libtest_name("child_report_for_stderr"),
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
        with("[roles] inherited_widget="),
        ["[roles] inherited_widget=1 components=1"]
    );
    assert_eq!(
        with("[live] profile="),
        [
            "[live] profile=code entrypoints=5 (kind=2 role=3 declared=0) owners=0 implementers=0 invoked=6 reached=16/35 init_gated=1"
        ],
        "kind: main and used's init (orphan's gated); role: AppComponent, BaseScreen and Home"
    );
    assert_eq!(
        with("[invoked]"),
        [
            "[invoked] rows=6 angular=1 constructor=3 flutter=1 go_runtime=0 apple_app=0 transloco=1 dio=0 sig_rejected=0 aliased=1 intermediate=1"
        ]
    );
}

/// `name`'s libtest path in this test binary: `<file>::<name>` when this file is a module of
/// its crate's merged test target (FH.18), `<name>` when it is its own target.
fn libtest_name(name: &str) -> String {
    match module_path!().split_once("::") {
        Some((_, module)) => format!("{module}::{name}"),
        None => name.to_owned(),
    }
}
