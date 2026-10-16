//! CH.1c — `glia gaps` dead_symbol and dispatch: a METHOD that IMPLEMENTS a
//! METHOD with an incoming CALLS / USES is no dead_symbol row, although
//! nothing reaches the caller from an entrypoint. The call into the declared
//! member (an interface method, an abstract member) is exactly what dispatch
//! reaches the implementation through, so the row's own claim, "no incoming
//! call or use", is false for it. Liveness is untouched: such an
//! implementation stays outside `entrypoint_reachable` (A7.8 carries it only
//! when the declared member is itself reached).
//!
//! Controls: an uncalled declaration vouches for nothing, an implementation
//! whose only call into the declared member is its own delegation vouches for
//! itself no more than a self-call does, a reached override stays live, and
//! two reports over one graph give the same rows.
//!
//! DF.22c: an override of a CONCRETE TypeScript member IMPLEMENTS it too
//! (rule `concrete_override`), so a template method's call into its own
//! member vouches for the subclass's override, and a reached member makes its
//! overrides live. A `super.m()` (rule `super_method`) runs exactly the
//! parent's member: it vouches for no override, a sibling's included.
//!
//! DT.5d: a live owner's constructor is live (an invoked-member hop), so the
//! two TS fixtures' constructor rows are gone; the dispatch assertions are
//! unchanged.

use std::path::Path;

use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, GapsReport, gaps_report};
use glia_engine::{entrypoint_reachable, generate_one};
use glia_graph::MergedGraph;

const STORE_JAVA: &str =
    "package shop;\n\npublic interface Store {\n    void save(String id);\n}\n";
const PG_STORE_JAVA: &str = "package shop;\n\npublic class PgStore implements Store {\n    public void save(String id) {\n        System.out.println(id);\n    }\n}\n";
const SVC_JAVA: &str = "package shop;\n\npublic class Svc {\n    private final Store store;\n\n    public Svc(Store store) {\n        this.store = store;\n    }\n\n    public void run(String id) {\n        store.save(id);\n    }\n}\n";
/// [`SVC_JAVA`] with `run`'s body emptied: nothing calls `Store::save`.
const SVC_IDLE_JAVA: &str = "package shop;\n\npublic class Svc {\n    private final Store store;\n\n    public Svc(Store store) {\n        this.store = store;\n    }\n\n    public void run(String id) {\n    }\n}\n";
/// A decorator whose `save` delegates to the interface member it implements.
const LOGGING_STORE_JAVA: &str = "package shop;\n\npublic class LoggingStore implements Store {\n    private final Store inner;\n\n    public LoggingStore(Store inner) {\n        this.inner = inner;\n    }\n\n    public void save(String id) {\n        inner.save(id);\n    }\n}\n";

/// CH.1b's `ts-inherited-calls` fixture: `BaseRepo::load` calls the abstract
/// `fetchOne`, `UserRepo::fetchOne` implements it.
const TS_FIXTURE: &str = "../bench/substrate-gap/fixtures/ts-inherited-calls";
const TS_FILES: [&str; 5] = [
    "package.json",
    "src/base-repo.ts",
    "src/user-repo.ts",
    "src/admin-repo.ts",
    "src/users.listener.ts",
];

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

/// Build a tempdir repo from `(path, body)` pairs.
fn build(files: &[(&str, &str)]) -> MergedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, body) in files {
        write(dir.path(), rel, body);
    }
    let root = dir.path().to_str().expect("utf-8 temp path").to_string();
    generate_one(&root)
        .unwrap_or_else(|e| panic!("generate_one: {e}"))
        .merged
}

/// The TS fixture copied into a tempdir, `src/users.listener.ts` (the one
/// entry) kept or left out.
fn build_ts(with_listener: bool) -> MergedGraph {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(TS_FIXTURE);
    let bodies: Vec<(&str, String)> = TS_FILES
        .iter()
        .filter(|f| with_listener || !f.ends_with("users.listener.ts"))
        .map(|f| {
            let body =
                std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
            (*f, body)
        })
        .collect();
    let files: Vec<(&str, &str)> = bodies.iter().map(|(f, b)| (*f, b.as_str())).collect();
    build(&files)
}

/// DF.22c's `ts-concrete-override-dispatch` fixture: `BaseStore::load` /
/// `save` call `key` / `persist`, `UserStore::key` and `AuditStore::persist`
/// override them, the `@OnEvent` listener is the one entry.
const TS_CONCRETE_FIXTURE: &str = "../bench/substrate-gap/fixtures/ts-concrete-override-dispatch";
const TS_CONCRETE_FILES: [&str; 5] = [
    "package.json",
    "src/base-store.ts",
    "src/user-store.ts",
    "src/audit-store.ts",
    "src/stores.listener.ts",
];

/// A template method calling its own concrete member, overridden below.
const RENDERER_TS: &str = "export class Renderer {\n  render(): string {\n    return this.paint();\n  }\n\n  paint(): string {\n    return 'base';\n  }\n}\n";
const FANCY_TS: &str = "import { Renderer } from './renderer';\n\nexport class FancyRenderer extends Renderer {\n  paint(): string {\n    return 'fancy';\n  }\n}\n";
/// A concrete `Base::m`, one override calling `super.m()`, one not.
const BASE_TS: &str = "export class Base {\n  m(): void {}\n}\n";
const A_TS: &str = "import { Base } from './base';\n\nexport class A extends Base {\n  m(): void {\n    super.m();\n  }\n}\n";
const B_TS: &str = "import { Base } from './base';\n\nexport class B extends Base {\n  m(): void {}\n}\n";
/// A virtual call into `Base::m` through a constructor-parameter field.
const CALLER_TS: &str = "import { Base } from './base';\n\nexport class Caller {\n  constructor(private readonly base: Base) {}\n\n  go(): void {\n    this.base.m();\n  }\n}\n";

/// The `files` of the fixture at `fixture` (relative to the engine crate)
/// copied into a tempdir and built.
fn build_fixture(fixture: &str, files: &[&str]) -> MergedGraph {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
    let bodies: Vec<(&str, String)> = files
        .iter()
        .map(|f| {
            let body =
                std::fs::read_to_string(src.join(f)).unwrap_or_else(|e| panic!("read {f}: {e}"));
            (*f, body)
        })
        .collect();
    let files: Vec<(&str, &str)> = bodies.iter().map(|(f, b)| (*f, b.as_str())).collect();
    build(&files)
}

fn java(svc: &str) -> MergedGraph {
    build(&[
        ("src/main/java/shop/Store.java", STORE_JAVA),
        ("src/main/java/shop/PgStore.java", PG_STORE_JAVA),
        ("src/main/java/shop/Svc.java", svc),
    ])
}

fn report(m: &MergedGraph) -> GapsReport {
    gaps_report(m, &[], &GapsOptions::default()).expect("known categories")
}

/// The dead_symbol rows' qnames, in report order.
fn dead(rep: &GapsReport) -> Vec<String> {
    rep.rows
        .iter()
        .filter(|r| r.category == DEAD_SYMBOL)
        .map(|r| r.qname.clone())
        .collect()
}

fn has(rows: &[String], suffix: &str) -> bool {
    rows.iter().any(|q| q.ends_with(suffix))
}

/// The id of the node whose qname ends with `suffix` (the first one).
fn node(m: &MergedGraph, suffix: &str) -> glia_core::NodeId {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .find(|(g, n)| {
            g.nav
                .qname_by_id
                .get(&n.id)
                .is_some_and(|q| q.ends_with(suffix))
        })
        .map(|(_, n)| n.id)
        .unwrap_or_else(|| panic!("a node ending `{suffix}`"))
}

#[test]
fn java_implementation_of_a_called_interface_method_is_not_dead() {
    let m = java(SVC_JAVA);
    let rows = dead(&report(&m));

    assert!(
        !has(&rows, "::PgStore::save"),
        "Svc::run calls Store::save, so dispatch reaches PgStore::save: {rows:?}"
    );
    for control in ["::Svc::run", "::Svc", "::PgStore"] {
        assert!(
            has(&rows, control),
            "nothing calls {control}: still a row, got {rows:?}"
        );
    }
    assert!(
        !entrypoint_reachable(&m).contains(&node(&m, "::PgStore::save")),
        "liveness is untouched: nothing reaches Svc::run"
    );
}

#[test]
fn uncalled_declaration_vouches_for_nothing() {
    let rows = dead(&report(&java(SVC_IDLE_JAVA)));
    assert!(
        has(&rows, "::PgStore::save"),
        "nothing calls Store::save, so PgStore::save stays a row: {rows:?}"
    );
}

#[test]
fn self_delegation_vouches_for_nothing() {
    // LoggingStore::save's own `inner.save(id)` is the only call into
    // Store::save: dispatch from it reaches LoggingStore::save only once
    // LoggingStore::save runs, so it vouches for itself no more than a
    // self-call does. It still vouches for the other implementation.
    let m = build(&[
        ("src/main/java/shop/Store.java", STORE_JAVA),
        ("src/main/java/shop/PgStore.java", PG_STORE_JAVA),
        ("src/main/java/shop/LoggingStore.java", LOGGING_STORE_JAVA),
        ("src/main/java/shop/Svc.java", SVC_IDLE_JAVA),
    ]);
    let rows = dead(&report(&m));
    assert!(
        has(&rows, "::LoggingStore::save"),
        "a self-delegating implementation is still a row: {rows:?}"
    );
    assert!(
        !has(&rows, "::PgStore::save"),
        "LoggingStore::save calls Store::save, which dispatches to PgStore::save: {rows:?}"
    );
}

#[test]
fn ts_override_of_a_called_abstract_member_is_not_dead() {
    let rows = dead(&report(&build_ts(false)));
    assert!(
        !has(&rows, "::UserRepo::fetchOne"),
        "BaseRepo::load calls the abstract fetchOne, so dispatch reaches UserRepo::fetchOne: {rows:?}"
    );
    for control in ["::AdminRepo::findAdmin", "::AdminRepo::audit"] {
        assert!(
            has(&rows, control),
            "nothing calls {control}: still a row, got {rows:?}"
        );
    }
}

#[test]
fn ts_reached_override_is_live() {
    let m = build_ts(true);
    assert!(
        entrypoint_reachable(&m).contains(&node(&m, "::UserRepo::fetchOne")),
        "the listener reaches find -> load -> fetchOne -> UserRepo::fetchOne (A7.8)"
    );
    // DT.5d: the live listener's constructor runs (an invoked-member hop),
    // so it is live and no row.
    assert!(
        entrypoint_reachable(&m).contains(&node(&m, "::UsersListener::constructor")),
        "a live owner's constructor is live (DT.5d)"
    );
    let mut rows = dead(&report(&m));
    rows.sort();
    let mut want = ["::AdminRepo", "::AdminRepo::audit", "::AdminRepo::findAdmin"];
    want.sort();
    assert_eq!(rows.len(), want.len(), "exactly the three rows: {rows:?}");
    for suffix in want {
        assert!(has(&rows, suffix), "{suffix} is a row: {rows:?}");
    }
}

#[test]
fn withheld_rows_are_deterministic() {
    let m = java(SVC_JAVA);
    let a = report(&m);
    let b = report(&m);
    let ids = |r: &GapsReport| r.rows.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&a), ids(&b), "row ids");
    assert_eq!(a.counts, b.counts, "counts");
    assert_eq!(a, b, "whole report");
}

#[test]
fn ts_template_method_override_is_not_dead() {
    // Renderer::render's `this.paint()` binds Renderer::paint; at runtime a
    // FancyRenderer runs its own override, which IMPLEMENTS Renderer::paint
    // (concrete_override), so dispatch reaches it.
    let m = build(&[("src/renderer.ts", RENDERER_TS), ("src/fancy.ts", FANCY_TS)]);
    let rows = dead(&report(&m));
    assert!(
        !has(&rows, "::FancyRenderer::paint"),
        "Renderer::render calls Renderer::paint, which dispatches to the override: {rows:?}"
    );
    assert!(has(&rows, "::Renderer::render"), "nothing calls render: still a row, got {rows:?}");
    assert!(
        !entrypoint_reachable(&m).contains(&node(&m, "::FancyRenderer::paint")),
        "liveness is untouched: no entry reaches render"
    );
}

#[test]
fn super_call_vouches_for_no_override() {
    // Base::m's only caller is A::m's `super.m()` (rule super_method): it runs
    // Base::m itself, never B::m, so neither override is vouched for. This
    // pins both ends of the rule string (graph writes it, gaps reads it).
    let m = build(&[("src/base.ts", BASE_TS), ("src/a.ts", A_TS), ("src/b.ts", B_TS)]);
    let rows = dead(&report(&m));
    for row in ["::A::m", "::B::m"] {
        assert!(has(&rows, row), "a super call dispatches to no override: {row} in {rows:?}");
    }
}

#[test]
fn a_virtual_call_vouches_for_every_override() {
    // Caller::go's `this.base.m()` binds Base::m: dispatch may run either
    // override, so neither is a row.
    let m = build(&[
        ("src/base.ts", BASE_TS),
        ("src/a.ts", A_TS),
        ("src/b.ts", B_TS),
        ("src/caller.ts", CALLER_TS),
    ]);
    let rows = dead(&report(&m));
    for row in ["::A::m", "::B::m"] {
        assert!(!has(&rows, row), "Caller::go calls Base::m: {row} not in {rows:?}");
    }
    assert!(has(&rows, "::Caller::go"), "nothing calls go: still a row, got {rows:?}");
}

#[test]
fn ts_concrete_override_reached_is_live() {
    let m = build_fixture(TS_CONCRETE_FIXTURE, &TS_CONCRETE_FILES);
    let live = entrypoint_reachable(&m);
    for reached in ["::UserStore::key", "::AuditStore::persist"] {
        assert!(
            live.contains(&node(&m, reached)),
            "the listener reaches load -> key / save -> persist, then the override (A7.8): {reached}"
        );
    }
    // DT.5d: the three constructors were the rows until a live owner's
    // constructor became live (an invoked-member hop).
    for ctor in [
        "::BaseStore::constructor",
        "::StoresListener::constructor",
        "::UserStore::constructor",
    ] {
        assert!(live.contains(&node(&m, ctor)), "a live owner's constructor is live: {ctor}");
    }
    let rows = dead(&report(&m));
    assert!(rows.is_empty(), "no dead row: {rows:?}");
}
