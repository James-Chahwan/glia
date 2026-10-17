//! DT.5i — Riverpod provider receivers are typed, StateNotifier state is its
//! generic argument, and a Dio interceptor is a service.
//!
//! Built from `bench/substrate-gap/fixtures/dart-riverpod-receivers` (copied
//! into a tempdir), the quokka-stack quokka_android shapes of the 0.5.2
//! close-out dead_symbol audit. A call on a Riverpod provider read binds in
//! the type the provider's declaration names (`Provider<T>`,
//! `Provider.autoDispose<T>`, a generic-less `Provider((ref) => T())`, the
//! notifier N and the state S of `StateNotifierProvider<N, S>`), directly,
//! through a local bound to the read and inside closures, never by method
//! name: FriendService and GroupService both declare `listAll`. A getter read
//! on a provider-read local runs; `state.copyWith(..)` in a
//! `StateNotifier<AuthState>` subclass binds AuthState's; a Dio interceptor
//! carries ROLE SERVICE. A local bound twice in its scope is typed nowhere.
//!
//! At HEAD ad3064f0 the fixture had CALLS 0/9, ROLE 0/1 and 12 dead_symbol
//! rows (`[live] ... reached=8/44`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{edge_category, evidence::Evidence, node_kind};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const PAGE: &str = "lib::features::login_page::_LoginPageState";
const AUTH: &str = "lib::core::auth_service";
const FRIEND: &str = "lib::core::friend_service";
const GROUP: &str = "lib::core::group_service::GroupService";
const TOKEN: &str = "lib::core::token_storage::TokenStorage";
const INTERCEPTOR: &str = "lib::core::auth_interceptor::AuthInterceptor";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/dart-riverpod-receivers")
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

fn build_in(dir: &Path) -> MergedGraph {
    copy_tree(&fixture_dir(), dir);
    generate_one(&dir.display().to_string()).expect("fixture builds").merged
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let m = build_in(tmp.path());
    (tmp, m)
}

/// One edge as `(from qname, to qname)` with its evidence.
type Row = (String, String, Option<Evidence>);

/// Every edge of `category` as `(from qname, to qname)`, with its evidence.
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<Row> {
    let mut qname: HashMap<NodeId, String> = HashMap::new();
    for g in &m.graphs {
        for (id, q) in &g.nav.qname_by_id {
            qname.insert(*id, q.clone());
        }
    }
    let name = |id: &NodeId| qname.get(id).cloned().unwrap_or_else(|| format!("{id:?}"));
    let mut out: Vec<Row> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (name(&e.from), name(&e.to), Evidence::read(&e.cells)))
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out
}

/// The evidence (emitter, rule) of the one `from -> to` edge in `rows`.
fn evidence_of(rows: &[Row], from: &str, to: &str) -> (String, Option<String>) {
    let hits: Vec<&Row> = rows.iter().filter(|(f, t, _)| f == from && t == to).collect();
    assert_eq!(hits.len(), 1, "one edge {from} -> {to}: {rows:?}");
    let ev = hits[0].2.clone().unwrap_or_else(|| panic!("evidence on {from} -> {to}"));
    (ev.emitter, ev.rule)
}

fn has(rows: &[Row], from: &str, to: &str) -> bool {
    rows.iter().any(|(f, t, _)| f == from && t == to)
}

/// Every `graph:dart` / `provider_receiver` CALLS edge is one of the
/// fixture's seven provider calls (the notifier read in a callback closure
/// among them), the getter read on `final auth = ref.watch(..)` binds through
/// the member-read hook, and the look-alikes stay unbound: GroupService's
/// `listAll`, `_export`'s twice-bound `svc`, and no USES of a provider passed
/// to `ref.read`.
#[test]
fn provider_receivers_bind_in_the_declared_type() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    let mut provider: Vec<(String, String)> = calls
        .iter()
        .filter(|(_, _, ev)| {
            ev.as_ref().is_some_and(|ev| {
                ev.emitter == "graph:dart" && ev.rule.as_deref() == Some("provider_receiver")
            })
        })
        .map(|(f, t, _)| (f.clone(), t.clone()))
        .collect();
    provider.sort();
    let mut want: Vec<(String, String)> = [
        (format!("{PAGE}::_submit"), format!("{AUTH}::AuthService::login")),
        (format!("{PAGE}::_submit"), format!("{FRIEND}::FriendService::accept")),
        (format!("{FRIEND}::friendsProvider"), format!("{FRIEND}::FriendService::listAll")),
        (format!("{PAGE}::_make"), format!("{GROUP}::create")),
        (format!("{AUTH}::AuthService::login"), format!("{TOKEN}::write")),
        (format!("{AUTH}::AuthService::logout"), format!("{TOKEN}::clear")),
        (format!("{PAGE}::build"), format!("{AUTH}::AuthService::logout")),
    ]
    .into_iter()
    .collect();
    want.sort();
    assert_eq!(provider, want);
    assert_eq!(
        evidence_of(&calls, &format!("{PAGE}::build"), &format!("{AUTH}::AuthState::isAuthenticated")),
        ("graph:reads".to_string(), Some("accessor_read".to_string()))
    );
    for to in ["listAll", "leave", "create"] {
        assert!(!has(&calls, &format!("{PAGE}::_export"), &format!("{GROUP}::{to}")), "_export -> {to}");
    }
    assert!(!has(&calls, &format!("{FRIEND}::friendsProvider"), &format!("{GROUP}::listAll")));
    let uses = edges(&m, edge_category::USES);
    assert!(
        !uses.iter().any(|(_, t, _)| t == &format!("{FRIEND}::friendServiceProvider")),
        "a provider read is no USES of its STATE_VAR: {uses:?}"
    );
}

/// `state.copyWith(..)` in `class AuthService extends
/// StateNotifier<AuthState>` binds AuthState's `copyWith` through the
/// inherited field's type (graph:calls receiver_type).
#[test]
fn state_holder_state_is_its_generic_argument() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    assert_eq!(
        evidence_of(&calls, &format!("{AUTH}::AuthService::login"), &format!("{AUTH}::AuthState::copyWith")),
        ("graph:calls".to_string(), Some("receiver_type".to_string()))
    );
}

/// The Dio interceptor carries ROLE SERVICE on its CLASS (the services
/// extractor's `dio_interceptor` rule, folded by LB.3a): no SERVICE twin
/// survives.
#[test]
fn a_dio_interceptor_is_a_service() {
    let (_tmp, m) = build();
    let ids = m.qnames_exact(INTERCEPTOR);
    assert_eq!(ids.len(), 1, "one node {INTERCEPTOR}: {ids:?}");
    let id = ids[0];
    let (g, n) = m
        .graphs
        .iter()
        .find_map(|g| g.nodes.iter().find(|n| n.id == id).map(|n| (g, n)))
        .expect("the interceptor node");
    let kind = g.nav.kind_by_id.get(&id).copied();
    assert_eq!(kind, Some(node_kind::CLASS));
    assert_eq!(glia_graph::roles::roles_in(kind, &n.cells), vec![node_kind::SERVICE]);
}

/// dead_symbol is exactly the fixture's true-dead methods: GroupService's
/// look-alike `listAll`, `leave` (only `_export`'s twice-bound `svc` calls
/// it) and the unread `TokenStorage::readRefresh`. `AuthInterceptor::onRequest`
/// is live through DT.5l's `dio` INVOKED_MEMBERS row, keyed on the
/// package:dio base the class extends (not on the ROLE this packet writes).
#[test]
fn dead_symbol_is_the_look_alikes_and_the_unread() {
    let (_tmp, m) = build();
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let mut rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    rows.sort();
    let mut want = vec![
        format!("{GROUP}::leave"),
        format!("{GROUP}::listAll"),
        format!("{TOKEN}::readRefresh"),
    ];
    want.sort();
    assert_eq!(rows, want, "3 of HEAD's 12 rows stay");
    assert!(
        glia_code_domain::profile::INVOKED_MEMBERS.iter().any(|r| r.framework == "dio"),
        "DT.5l's dio row keeps the interceptor's override live"
    );
}

/// Set by [`markers_count_providers_and_reads`] on its child run.
const CHILD_ENV: &str = "GLIA_DT5I_CHILD_DIR";

/// A no-op unless run as the child of [`markers_count_providers_and_reads`],
/// whose stderr carries the parser's, the extractor's and the graph's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build_in(Path::new(&dir));
    }
}

#[test]
fn markers_count_providers_and_reads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, tmp.path())
        .env("GLIA_THREADS", "1")
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child failed: {}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines = |prefix: &str| -> Vec<String> {
        stderr.lines().filter(|l| l.starts_with(prefix)).map(String::from).collect()
    };
    assert_eq!(lines("[dart-scope]"), ["[dart-scope] provider receivers bound=7"]);
    assert_eq!(
        lines("[reads]"),
        ["[reads] member reads accessor_read=1 tear_off=0 static_member=1 unbound=1"]
    );
    let riverpod = lines("[dart-riverpod]");
    for want in [
        "[dart-riverpod] providers=1 locals=1 holders=1 file=lib/core/auth_service.dart",
        "[dart-riverpod] providers=0 locals=2 holders=0 file=lib/features/login_page.dart",
        "[dart-riverpod] providers=1 locals=0 holders=0 file=lib/core/token_storage.dart",
    ] {
        assert!(riverpod.iter().any(|l| l == want), "{want}: {riverpod:?}");
    }
    assert!(
        !riverpod.iter().any(|l| l.ends_with("file=lib/core/auth_interceptor.dart")),
        "{riverpod:?}"
    );
    assert_eq!(
        lines("[services] dart"),
        ["[services] dart classified=1 rules=dio_interceptor:1 module=lib::core::auth_interceptor"]
    );
}
