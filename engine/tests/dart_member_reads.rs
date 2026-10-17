//! DT.5h — Dart names and member reads become edges.
//!
//! Built from `bench/substrate-gap/fixtures/dart-member-reads` (copied into
//! a tempdir), the quokka-stack quokka_android shapes of the 0.5.2 close-out
//! dead_symbol audit. A member named without a call is a reference: a
//! getter / setter runs (CALLS: a bare `_canSubmit`, `this._canSubmit`,
//! `$_title`, a `_mode = 1` write, `Env.baseUrl`, `_friends.isReady` on a
//! typed field), a method torn off is used (USES: `.map(Friend.fromJson)`,
//! and `_submit` / `_cancel` / `_google` / `_make` in a conditional's
//! branches or an if-null operand), a static field read (`Env.clientId`, no
//! node of its own) uses its class, `FriendService.new` builds a
//! FriendService, a class field's initialiser is its class's calls, and a
//! one-parameter closure over a `List<T>` field's elements has a T
//! parameter. The look-alikes stay unbound: a same-named factory, an int
//! loop variable's `isEven`, a shadowing local, an untyped local shadowing a
//! typed field, an `onPressed:` label and a cascade's `..onPressed`.
//!
//! At HEAD 621f8213 the fixture had CALLS 0/11, USES 0/6 and 20 dead_symbol
//! rows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{edge_category, evidence::Evidence};
use glia_core::{EdgeCategoryId, NodeId};
use glia_engine::gaps::{DEAD_SYMBOL, GapsOptions, gaps_report};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

const PAGE: &str = "lib::features::friends_page::_FriendsPageState";
const ENV: &str = "lib::core::env::Env";
const SERVICE: &str = "lib::core::friend_service::FriendService";
const MODELS: &str = "lib::core::models";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures/dart-member-reads")
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

/// `Type.member` / `x.member` reads bind through the reader's own scope in
/// the graph (`graph:reads`): a getter's CALLS, a factory tear-off's USES, a
/// static field's USES of its CLASS, a getter read on a field declared
/// FriendService.
#[test]
fn member_reads_bind_by_what_they_name() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    let uses = edges(&m, edge_category::USES);
    let reads = "graph:reads".to_string();
    for (rows, from, to, rule) in [
        (&calls, format!("{SERVICE}::isReady"), format!("{ENV}::baseUrl"), "accessor_read"),
        (&calls, format!("{PAGE}::build"), format!("{ENV}::port"), "accessor_read"),
        (&calls, format!("{PAGE}::_canSubmit"), format!("{SERVICE}::isReady"), "accessor_read"),
        (&uses, format!("{SERVICE}::listAll"), format!("{MODELS}::Friend::fromJson"), "tear_off"),
        (&uses, format!("{PAGE}::build"), ENV.to_string(), "static_member"),
    ] {
        assert_eq!(
            evidence_of(rows, &from, &to),
            (reads.clone(), Some(rule.to_string())),
            "{from} -> {to}"
        );
    }
    // Friend.fromJson tears off Friend's factory, never a same-named one.
    assert!(!has(&uses, &format!("{SERVICE}::listAll"), &format!("{MODELS}::Interest::fromJson")));
}

/// A same-class name in any value position is a reference the parser binds
/// (`parser:dart`): methods in a conditional's branches (a parenthesised
/// nested one too) and an if-null operand are torn off (method_ref); a
/// getter in a condition, behind `this.` and in an interpolation runs
/// (accessor_read); a setter assignment runs (accessor_write). The fixture's
/// six forbid edges are absent.
#[test]
fn names_in_any_value_position_are_references() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    let uses = edges(&m, edge_category::USES);
    let dart = "parser:dart".to_string();
    for to in ["_submit", "_cancel", "_google", "_make"] {
        assert_eq!(
            evidence_of(&uses, &format!("{PAGE}::build"), &format!("{PAGE}::{to}")),
            (dart.clone(), Some("method_ref".to_string())),
            "{to}"
        );
    }
    for (from, to, rule) in [
        ("_submit", "_canSubmit", "accessor_read"),
        ("build", "_canSubmit", "accessor_read"),
        ("build", "_title", "accessor_read"),
        ("_google", "_mode", "accessor_write"),
    ] {
        assert_eq!(
            evidence_of(&calls, &format!("{PAGE}::{from}"), &format!("{PAGE}::{to}")),
            (dart.clone(), Some(rule.to_string())),
            "{from} -> {to}"
        );
    }
    // The six forbid rows of the fixture's key.json.
    assert!(!has(&calls, &format!("{PAGE}::_dump"), &format!("{MODELS}::Interest::isEven")));
    assert!(!has(&calls, &format!("{PAGE}::_shadow"), &format!("{PAGE}::_canSubmit")));
    assert!(!has(&calls, &format!("{PAGE}::_shadow"), &format!("{SERVICE}::isReady")));
    assert!(!has(&uses, &format!("{PAGE}::_shadow"), &format!("{PAGE}::onPressed")));
    assert!(!has(&uses, &format!("{PAGE}::build"), &format!("{PAGE}::onPressed")));
    assert!(!has(&uses, &format!("{SERVICE}::listAll"), &format!("{MODELS}::Interest::fromJson")));
}

/// `FriendService.new` builds a FriendService (CALLS of the CLASS), a class
/// field's initialisers are the class's calls (`FriendService()`,
/// `[_Tab(..)]`), and `_interests.map((i) => i.toJson())` over a
/// `List<Interest>` field binds Interest's `toJson`.
#[test]
fn tear_offs_field_initialisers_and_element_closures() {
    let (_tmp, m) = build();
    let calls = edges(&m, edge_category::CALLS);
    for (from, to, why) in [
        (format!("{PAGE}::build"), SERVICE.to_string(), "_register(FriendService.new)"),
        (PAGE.to_string(), SERVICE.to_string(), "final FriendService _friends = FriendService()"),
        (PAGE.to_string(), "lib::features::friends_page::_Tab".to_string(), "static const _tabs"),
        (format!("{PAGE}::build"), format!("{MODELS}::Interest::toJson"), "an element closure"),
    ] {
        assert!(has(&calls, &from, &to), "{why}: CALLS {from} -> {to}; CALLS = {calls:?}");
    }
    // `_dump`'s closure `i` shares its name with an int loop variable that
    // receives `.isEven`: it is typed nowhere, so `i.toJson()` binds nothing.
    assert!(!has(&calls, &format!("{PAGE}::_dump"), &format!("{MODELS}::Interest::toJson")));
}

#[test]
fn dead_symbol_is_the_look_alikes_and_the_unread() {
    let (_tmp, m) = build();
    let rep = gaps_report(&m, &[], &GapsOptions::default()).expect("known categories");
    let mut rows: Vec<String> =
        rep.rows.iter().filter(|r| r.category == DEAD_SYMBOL).map(|r| r.qname.clone()).collect();
    rows.sort();
    let mut want = vec![
        format!("{ENV}::debugBanner"),
        format!("{MODELS}::Interest::fromJson"),
        format!("{MODELS}::Interest::isEven"),
        format!("{PAGE}::onPressed"),
    ];
    want.sort();
    assert_eq!(rows, want, "4 of HEAD's 20 rows stay");
}

/// Set by [`markers_count_the_reads`] on its child run.
const CHILD_ENV: &str = "GLIA_DT5H_CHILD_DIR";

/// A no-op unless run as the child of [`markers_count_the_reads`], whose
/// stderr carries the parser's and the graph's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build_in(Path::new(&dir));
    }
}

#[test]
fn markers_count_the_reads() {
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
    assert_eq!(
        lines("[reads]"),
        ["[reads] member reads accessor_read=3 tear_off=1 static_member=1 unbound=2"]
    );
    let calls = lines("[dart-calls]");
    let page = calls
        .iter()
        .find(|l| l.ends_with("file=lib/features/friends_page.dart"))
        .unwrap_or_else(|| panic!("friends_page: {calls:?}"));
    assert!(
        page.contains(
            " method_refs=4 accessor_refs=4 tear_offs=1 field_inits=5 member_reads=4 elements=1 "
        ),
        "{page}"
    );
}
