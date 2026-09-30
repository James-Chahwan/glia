//! CD.4b — `hubs`: ranked, located hub rows.
//!
//! One Python repo: `util/log.py` `log()` is called from 12 functions across
//! `svc_a/` and `svc_b/`; `svc_a/main.py` `main()` calls 10 steps; `tests/`
//! holds 30 `test_*` functions (qnames `tests::...`, so ORIGIN `test_fixture`)
//! that each call `log` once. With tests left out (the default) `log` is a
//! 12-caller utility used by two services and `main` a 10-callee
//! orchestrator; with tests in, `log` has 42 callers.

use std::collections::BTreeMap;

use glia_engine::generate_one;
use glia_engine::hubs::{HubArgs, HubRow, HubsAnswer, hubs};
use glia_graph::MergedGraph;

const LOG: &str = "def log(msg):\n    return msg\n";

/// `<prefix>_handle_1..=6`, each returning `log(..)`.
fn handlers(prefix: &str) -> String {
    let mut s = String::from("from util.log import log\n\n");
    for i in 1..=6 {
        s.push_str(&format!(
            "\ndef {prefix}_handle_{i}():\n    return log(\"{prefix}{i}\")\n\n"
        ));
    }
    s
}

fn steps() -> String {
    (1..=10)
        .map(|i| format!("def step_{i}():\n    return {i}\n\n\n"))
        .collect()
}

fn main_py() -> String {
    let names: Vec<String> = (1..=10).map(|i| format!("step_{i}")).collect();
    let mut s = format!(
        "from svc_a.steps import {}\n\n\ndef main():\n",
        names.join(", ")
    );
    for n in &names {
        s.push_str(&format!("    {n}()\n"));
    }
    s
}

fn tests_py() -> String {
    let mut s = String::from("from util.log import log\n\n");
    for i in 1..=30 {
        s.push_str(&format!(
            "\ndef test_log_{i:02}():\n    assert log(\"t{i}\") == \"t{i}\"\n\n"
        ));
    }
    s
}

fn build() -> (tempfile::TempDir, MergedGraph, BTreeMap<u64, String>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let files = [
        ("util/log.py", LOG.to_string()),
        ("svc_a/handlers.py", handlers("a")),
        ("svc_b/handlers.py", handlers("b")),
        ("svc_a/steps.py", steps()),
        ("svc_a/main.py", main_py()),
        ("tests/test_log.py", tests_py()),
    ];
    for (rel, src) in &files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    (tmp, r.merged, r.repo_labels)
}

fn args() -> HubArgs {
    HubArgs::default()
}

fn row<'a>(rows: &'a [HubRow], qname: &str) -> Option<&'a HubRow> {
    rows.iter().find(|r| r.qname == qname)
}

fn json(a: &HubsAnswer) -> String {
    serde_json::to_string(a).expect("serialise")
}

#[test]
fn defaults() {
    let a = HubArgs::default();
    assert_eq!((a.top, a.min_degree, a.include_tests), (20, 5, false));
    assert_eq!(
        (a.scope.as_deref(), a.category.as_deref(), a.surface),
        (None, None, "engine")
    );
}

#[test]
fn utility_found() {
    let (_tmp, merged, labels) = build();
    let ans = hubs(&merged, &labels, &args());
    let top = ans.fan_in.first().expect("a fan-in row");
    assert_eq!(top.qname, "util::log::log", "{:#?}", ans.fan_in);
    assert_eq!(top.label, "utility");
    assert_eq!(top.kind, "FUNCTION");
    assert_eq!(top.fan_in, 12, "the 30 test calls are left out");
    assert_eq!(top.fan_out, 0);
    assert_eq!(top.caller_services, vec!["svc_a", "svc_b"]);
    assert!(top.callee_services.is_empty());
    assert_eq!(top.file.as_deref(), Some("util/log.py"));
    assert_eq!(top.line, Some(1), "1-based, at the def");
    assert_eq!(top.by_category, vec![("CALLS", 12, 0)]);
    assert_eq!(top.tier, "derived");
    assert!(top.authority > 0.0 && top.hub == 0.0, "{top:#?}");
    assert!(
        ans.fan_in.iter().all(|r| !r.qname.starts_with("tests::")),
        "{:#?}",
        ans.fan_in
    );
    assert!(
        ans.p99_in >= 1 && ans.nodes > 0 && ans.edges > 0,
        "{ans:#?}"
    );
    assert!(ans.absence.is_none());
}

#[test]
fn orchestrator_found() {
    let (_tmp, merged, labels) = build();
    let ans = hubs(&merged, &labels, &args());
    let top = ans.fan_out.first().expect("a fan-out row");
    assert_eq!(top.qname, "svc_a::main::main", "{:#?}", ans.fan_out);
    assert_eq!(top.label, "orchestrator");
    assert_eq!((top.fan_in, top.fan_out), (0, 10));
    assert_eq!(top.callee_services, vec!["svc_a"]);
    assert!(top.hub > 0.0 && top.authority == 0.0, "{top:#?}");
    assert!(top.live, "main is an entrypoint");
    assert_eq!(top.line, Some(4));
}

#[test]
fn cross_service() {
    let (_tmp, merged, labels) = build();
    let ans = hubs(&merged, &labels, &args());
    let log = row(&ans.cross_service, "util::log::log").expect("log joins two services");
    assert_eq!(
        log.label, "utility",
        "a qualifying row keeps its degree label"
    );
    assert!(
        row(&ans.cross_service, "svc_a::main::main").is_none(),
        "main's callees are one service: {:#?}",
        ans.cross_service
    );
    assert!(
        ans.cross_service
            .iter()
            .all(|r| r.caller_services.len() >= 2 || r.callee_services.len() >= 2)
    );
}

#[test]
fn include_tests() {
    let (_tmp, merged, labels) = build();
    let mut a = args();
    a.include_tests = true;
    let ans = hubs(&merged, &labels, &a);
    let log = row(&ans.fan_in, "util::log::log").expect("log");
    assert_eq!(log.fan_in, 42, "{log:#?}");
    assert_eq!(log.caller_services, vec!["svc_a", "svc_b", "tests"]);
    // TESTS edges stay uncounted even with test nodes indexed.
    assert_eq!(log.by_category, vec![("CALLS", 42, 0)]);
}

#[test]
fn category_filter() {
    let (_tmp, merged, labels) = build();
    let base = hubs(&merged, &labels, &args());
    let mut a = args();
    a.category = Some("CALLS".to_string());
    let calls = hubs(&merged, &labels, &a);
    assert_eq!(
        serde_json::to_string(&calls.fan_in).expect("json"),
        serde_json::to_string(&base.fan_in).expect("json")
    );
    assert!(calls.absence.is_none());

    a.category = Some("NO_SUCH".to_string());
    let none = hubs(&merged, &labels, &a);
    assert!(none.fan_in.is_empty() && none.fan_out.is_empty() && none.cross_service.is_empty());
    let why = none.absence.expect("an absence");
    assert_eq!(why.reason, "no_match");
    assert!(why.note.contains("NO_SUCH"), "{}", why.note);
}

#[test]
fn scope_keeps_rows_under_it() {
    let (_tmp, merged, labels) = build();
    let mut a = args();
    a.scope = Some("svc_a".to_string());
    let ans = hubs(&merged, &labels, &a);
    let main = row(&ans.fan_out, "svc_a::main::main").expect("main is under svc_a");
    assert_eq!(main.fan_out, 10);
    assert!(row(&ans.fan_in, "util::log::log").is_none(), "{ans:#?}");
    for r in ans
        .fan_in
        .iter()
        .chain(&ans.fan_out)
        .chain(&ans.cross_service)
    {
        assert!(
            r.file.as_deref().is_none_or(|f| f.starts_with("svc_a/")),
            "{r:#?}"
        );
    }

    a.scope = Some("nowhere".to_string());
    let empty = hubs(&merged, &labels, &a);
    assert_eq!(empty.absence.expect("an absence").reason, "no_match");
}

#[test]
fn top_cuts_each_list() {
    let (_tmp, merged, labels) = build();
    let mut a = args();
    a.include_tests = true;
    a.top = 1;
    let ans = hubs(&merged, &labels, &a);
    assert!(ans.fan_in.len() <= 1 && ans.fan_out.len() <= 1 && ans.cross_service.len() <= 1);
}

/// The measured real-repo check: `GLIA_HUBS_PROBE=<repo copy>
/// GLIA_NO_PERSIST=1 cargo test -p glia-engine --test hubs real_repo_probe --
/// --ignored --nocapture` prints each list as `label in out services
/// qname file:line`. Run it on a `git archive` copy, never a live checkout.
#[test]
#[ignore = "needs GLIA_HUBS_PROBE=<repo copy>"]
fn real_repo_probe() {
    let Ok(repo) = std::env::var("GLIA_HUBS_PROBE") else {
        panic!("set GLIA_HUBS_PROBE to a repo copy");
    };
    let r = generate_one(&repo).expect("generate_one");
    let ans = hubs(&r.merged, &r.repo_labels, &args());
    println!(
        "nodes={} edges={} p99_in={} p99_out={}",
        ans.nodes, ans.edges, ans.p99_in, ans.p99_out
    );
    for (name, rows) in [
        ("fan_in", &ans.fan_in),
        ("fan_out", &ans.fan_out),
        ("cross_service", &ans.cross_service),
    ] {
        println!("== {name}");
        for h in rows {
            println!(
                "{:12} in={:<4} out={:<4} callers={:?} callees={:?} {} {}:{}",
                h.label,
                h.fan_in,
                h.fan_out,
                h.caller_services,
                h.callee_services,
                h.qname,
                h.file.as_deref().unwrap_or("-"),
                h.line.unwrap_or(0)
            );
        }
    }
}

#[test]
fn deterministic() {
    let (_t1, m1, l1) = build();
    let (_t2, m2, l2) = build();
    for include_tests in [false, true] {
        let mut a = args();
        a.include_tests = include_tests;
        let first = json(&hubs(&m1, &l1, &a));
        assert_eq!(first, json(&hubs(&m1, &l1, &a)), "two calls");
        assert_eq!(first, json(&hubs(&m2, &l2, &a)), "two independent builds");
    }
}
