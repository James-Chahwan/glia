//! LD.3b — `find::find_nodes`: tiered, explainable, degree-ranked, located.
//!
//! The fixture is the packet's acceptance build: a Python service and a
//! TypeScript client in two directories, merged with `generate_many`, so the
//! answer spans repos and both files define a `users` module. Degrees at the
//! time of writing (total, intra + cross): users.py MODULE 4, users::get_user
//! 3, users::UserService 3, users.ts MODULE 2, every other function /
//! method 2, both ENDPOINTs 1. The KEY is the contract, those numbers are its
//! instance, so besides the packet's literal rows every test re-checks the
//! whole answer against the key with degrees counted here, independently.

use std::path::Path;

use repo_graph_core::NodeId;
use repo_graph_engine::find::{FindOptions, FoundNode, find_nodes};
use repo_graph_engine::{generate_many, generate_one};
use repo_graph_graph::MergedGraph;

const USERS_PY: &str = "def get_user(uid):\n    return load(uid)\n\n\ndef load(uid):\n    return {\"id\": uid}\n\n\nclass UserService:\n    def get_user(self, uid):\n        return get_user(uid)\n\n\ndef user_service_helper():\n    return UserService()\n";
const USERS_TS: &str = "export function getUser(id: string) {\n  return fetch(`/users/${id}`);\n}\n\nexport function getUsers() {\n  return fetch('/users');\n}\n";

fn write_sources(root: &Path) {
    std::fs::create_dir_all(root.join("api")).unwrap();
    std::fs::create_dir_all(root.join("web")).unwrap();
    std::fs::write(root.join("api/users.py"), USERS_PY).unwrap();
    std::fs::write(root.join("web/users.ts"), USERS_TS).unwrap();
}

/// The two-directory merge: `api` and `web` are separate repos.
fn two_repo_build(root: &Path) -> MergedGraph {
    write_sources(root);
    let repos = [
        root.join("api").to_string_lossy().into_owned(),
        root.join("web").to_string_lossy().into_owned(),
    ];
    generate_many(&repos).expect("generate_many").merged
}

fn opts(top_k: usize) -> FindOptions {
    let mut o = FindOptions::default();
    o.top_k = top_k;
    o
}

fn find(m: &MergedGraph, q: &str) -> Vec<FoundNode> {
    find_nodes(m, q, &FindOptions::default()).results
}

fn degree(m: &MergedGraph, id: u64) -> usize {
    m.all_edges().filter(|e| e.from.0 == id || e.to.0 == id).count()
}

const TIERS: [&str; 9] = [
    "exact_qname",
    "exact_name",
    "exact_ci",
    "qname_suffix",
    "name_prefix",
    "name_word",
    "name_substring",
    "qname_substring",
    "subsequence",
];

fn tier_rank(t: &str) -> usize {
    TIERS.iter().position(|x| *x == t).unwrap_or_else(|| panic!("unknown tier {t}"))
}

const CONTAINERS: [&str; 5] = ["MODULE", "PACKAGE", "PROJECT", "REGION", "DOC_SPACE"];

/// Every adjacent pair of `rows` is in the documented order: tier first; in
/// the exact tiers pick_primary's (declaration, degree desc, id asc); in the
/// others (degree desc, qname length asc, qname asc, id asc).
fn assert_follows_the_key(m: &MergedGraph, rows: &[FoundNode]) {
    for w in rows.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let (ta, tb) = (tier_rank(a.r#match), tier_rank(b.r#match));
        assert!(ta <= tb, "tier order broken: {a:?} before {b:?}");
        if ta < tb {
            continue;
        }
        let (da, db) = (degree(m, a.id), degree(m, b.id));
        let ka = if ta <= 1 {
            let decl = |r: &FoundNode| !CONTAINERS.contains(&r.kind);
            (!decl(a), usize::MAX - da, 0, String::new(), a.id)
        } else {
            (false, usize::MAX - da, a.qname.chars().count(), a.qname.clone(), a.id)
        };
        let kb = if tb <= 1 {
            let decl = |r: &FoundNode| !CONTAINERS.contains(&r.kind);
            (!decl(b), usize::MAX - db, 0, String::new(), b.id)
        } else {
            (false, usize::MAX - db, b.qname.chars().count(), b.qname.clone(), b.id)
        };
        assert!(ka < kb, "key order broken: {a:?} (deg {da}) before {b:?} (deg {db})");
    }
}

fn is_module_in(r: &FoundNode, file: &str) -> bool {
    r.qname == "users" && r.kind == "MODULE" && r.file.as_deref() == Some(file)
}

#[test]
fn exact_name_rows_rank_by_pick_primary() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    let rows = find(&m, "get_user");
    assert!(rows.len() >= 2, "{rows:#?}");
    assert_eq!(rows[0].qname, "users::get_user", "{rows:#?}");
    assert_eq!(rows[0].r#match, "exact_name");
    assert_eq!(rows[0].kind, "FUNCTION");
    assert_eq!(rows[1].qname, "users::UserService::get_user", "{rows:#?}");
    assert_eq!(rows[1].r#match, "exact_name");
    assert_eq!(rows[1].kind, "METHOD");
    // Located, 1-based: `def get_user` is the file's first line.
    assert_eq!(rows[0].file.as_deref(), Some("users.py"));
    assert_eq!(rows[0].line, Some(1));
    assert_follows_the_key(&m, &rows);
}

#[test]
fn user_ranks_prefix_then_word_then_module_path() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    let rows = find(&m, "user");
    assert_eq!(rows.len(), 11, "{rows:#?}");
    assert!(is_module_in(&rows[0], "users.py"), "{rows:#?}");
    assert_eq!(rows[1].qname, "users::UserService");
    assert!(is_module_in(&rows[2], "users.ts"), "{rows:#?}");
    assert_eq!(rows[3].qname, "users::user_service_helper");
    for r in &rows[..4] {
        assert_eq!(r.r#match, "name_prefix", "{r:?}");
    }
    assert_eq!(rows[4].qname, "users::get_user");
    assert_eq!(rows[4].r#match, "name_word");
    let word: Vec<&str> =
        rows.iter().filter(|r| r.r#match == "name_word").map(|r| r.qname.as_str()).collect();
    assert_eq!(word.len(), 6, "{rows:#?}");
    let last = rows.last().unwrap();
    assert_eq!(last.qname, "users::load");
    assert_eq!(last.r#match, "qname_substring");
    assert_follows_the_key(&m, &rows);
}

#[test]
fn usr_falls_through_to_subsequence() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    let rows = find(&m, "usr");
    assert!(rows.len() >= 3, "{rows:#?}");
    assert!(rows.iter().all(|r| r.r#match == "subsequence"), "{rows:#?}");
    assert!(is_module_in(&rows[0], "users.py"), "{rows:#?}");
    assert_eq!(rows[1].qname, "users::get_user");
    assert_eq!(rows[2].qname, "users::UserService");
    assert!(rows.iter().all(|r| r.qname != "users::load"), "{rows:#?}");
    assert_follows_the_key(&m, &rows);

    // Two-char queries never reach the subsequence tier.
    assert!(find(&m, "ur").is_empty());
}

#[test]
fn class_qualified_method_matches_as_qname_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    let rows = find(&m, "UserService::get_user");
    assert_eq!(rows[0].qname, "users::UserService::get_user", "{rows:#?}");
    assert_eq!(rows[0].r#match, "qname_suffix");
    // A dotted path is read as a qname too.
    let dotted = find(&m, "UserService.get_user");
    assert_eq!(dotted[0].qname, "users::UserService::get_user", "{dotted:#?}");
    assert_eq!(dotted[0].r#match, "qname_suffix");
}

#[test]
fn top_row_is_the_single_node_resolution_on_an_exact_match() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    for q in ["get_user", "load", "UserService", "getUser"] {
        let top = find_nodes(&m, q, &opts(1)).results;
        assert_eq!(top.len(), 1, "{q}");
        assert_eq!(Some(NodeId(top[0].id)), m.resolve_name(q), "{q}: {top:?}");
    }
    // Two MODULEs share the qname `users`: node_id_by_qname's pick is ours.
    let top = find_nodes(&m, "users", &opts(1)).results;
    assert_eq!(top[0].r#match, "exact_qname");
    assert_eq!(Some(NodeId(top[0].id)), m.node_id_by_qname("users"));
}

/// pick_primary puts a declaration before a container whatever their degrees.
/// `svc/handler.py` (a MODULE named `handler`, with three children) out-
/// degrees the one-edge function `handler` in `app.py`, so a degree-only key
/// would rank the file first and drift from `resolve_name`. Both match
/// `exact_name` — neither qname is the bare `handler` — so the exact-tier key
/// alone decides.
#[test]
fn a_declaration_beats_a_busier_container_as_pick_primary_does() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("svc")).unwrap();
    std::fs::write(
        root.join("svc/handler.py"),
        "def a():\n    return 1\n\n\ndef b():\n    return 2\n\n\ndef c():\n    return 3\n",
    )
    .unwrap();
    std::fs::write(root.join("app.py"), "def handler():\n    return 0\n").unwrap();
    let m = generate_one(&root.to_string_lossy()).expect("generate_one").merged;

    let rows = find(&m, "handler");
    let module = rows.iter().find(|r| r.kind == "MODULE").expect("the handler module");
    let func = rows.iter().find(|r| r.kind == "FUNCTION").expect("the handler function");
    assert!(
        module.r#match == "exact_name" && func.r#match == "exact_name",
        "fixture must put both in exact_name: {rows:#?}"
    );
    assert!(
        degree(&m, module.id) > degree(&m, func.id),
        "fixture must make the container busier: {rows:#?}"
    );
    assert_eq!(rows[0].id, func.id, "{rows:#?}");
    assert_eq!(Some(NodeId(rows[0].id)), m.resolve_name("handler"));
    assert_follows_the_key(&m, &rows);
}

/// The tier outranks the key: a MODULE whose qname IS the query is
/// `exact_qname`, above a same-named FUNCTION's `exact_name`, and is the node
/// `node_id_by_qname` picks.
#[test]
fn exact_qname_outranks_exact_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("handler.py"), "def a():\n    return 1\n").unwrap();
    std::fs::write(root.join("app.py"), "def handler():\n    return 0\n").unwrap();
    let m = generate_one(&root.to_string_lossy()).expect("generate_one").merged;
    let rows = find(&m, "handler");
    assert_eq!(rows[0].r#match, "exact_qname", "{rows:#?}");
    assert_eq!(rows[0].kind, "MODULE");
    assert_eq!(Some(NodeId(rows[0].id)), m.node_id_by_qname("handler"));
    assert_eq!(rows[1].r#match, "exact_name", "{rows:#?}");
    assert_eq!(rows[1].kind, "FUNCTION");
}

#[test]
fn kinds_filter_before_ranking_and_top_k_truncates_after() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    let mut o = FindOptions::default();
    o.kinds = Some(vec![repo_graph_code_domain::node_kind::FUNCTION]);
    let rows = find_nodes(&m, "user", &o).results;
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.kind == "FUNCTION"), "{rows:#?}");
    assert_eq!(rows[0].qname, "users::user_service_helper", "{rows:#?}");

    let all = find(&m, "user");
    let two = find_nodes(&m, "user", &opts(2)).results;
    assert_eq!(two.as_slice(), &all[..2]);
    let uncapped = find_nodes(&m, "user", &opts(0)).results;
    assert_eq!(uncapped, all);
}

/// A8.3 rules: scope runs before `top_k` and keeps the unlocatable.
#[test]
fn scope_narrows_within_one_repo_before_top_k() {
    let dir = tempfile::tempdir().unwrap();
    write_sources(dir.path());
    let m = generate_one(&dir.path().to_string_lossy()).expect("generate_one").merged;
    let mut o = opts(3);
    o.scope = Some("web".to_string());
    let rows = find_nodes(&m, "user", &o).results;
    assert_eq!(rows.len(), 3, "{rows:#?}");
    for r in &rows {
        assert!(
            r.file.as_deref().is_none_or(|f| f.starts_with("web/")),
            "out of scope: {r:?}"
        );
    }
    assert!(rows.iter().any(|r| r.file.is_some()), "{rows:#?}");
}

#[test]
fn empty_and_unmatched_queries_return_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    assert!(find(&m, "").is_empty());
    assert!(find(&m, "   ").is_empty());
    assert!(find(&m, "zzzqqq").is_empty());
}

#[test]
fn repeated_calls_are_identical() {
    let dir = tempfile::tempdir().unwrap();
    let m = two_repo_build(dir.path());
    for q in ["user", "usr", "get_user", "UserService::get_user", "users"] {
        assert_eq!(find(&m, q), find(&m, q), "{q}");
    }
}
