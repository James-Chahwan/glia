//! CD.4e — `duplicate_flows`: entry flows whose reached sets are identical
//! (exact, DERIVED) or overlap at a Jaccard threshold (near, HEURISTIC), with
//! utility hubs and test entries left out.
//!
//! One Flask repo. `app/views.py` stacks `@app.route('/orders')` and
//! `@app.route('/v2/orders')` on ONE handler `list_orders`, which calls
//! `repo.find()` and `repo.count()` (`app/repo.py`). `create_order` (POST
//! /orders) and `update_order` (PUT /orders/<id>) each call the same 14 plain
//! helpers plus one private helper each (`audit_create` / `audit_update`), so
//! their flows share 14 of 18 nodes (Jaccard 14/18 = 0.778). GET /health is
//! unrelated; `tests/test_orders.py` `test_list()` calls `list_orders()`. No
//! helper is named publish / emit / send, which would mint a one-sided USES
//! edge and skew the sets.
//!
//! Before CD.4e `glia_engine::duplicate_flows` was a doc-only slot and
//! `glia duplicate-flows` an unrecognised subcommand (exit 2).

use std::collections::{BTreeMap, BTreeSet};

use glia_engine::duplicate_flows::{DupFlowArgs, DupFlowGroup, DuplicateFlows, duplicate_flows};
use glia_engine::generate_one;
use glia_engine::trace::entry_flows;
use glia_graph::MergedGraph;

const HELPERS: [&str; 14] = [
    "validate",
    "price",
    "tax",
    "discount",
    "stock",
    "reserve",
    "ship_date",
    "currency",
    "rounding",
    "fraud_check",
    "ledger_line",
    "audit_row",
    "receipt",
    "totals",
];

/// `app/views.py`. With `log`, every function calls `app.log.log` first, so
/// `log` is a utility hub (fan-in 22, CD.4b's p99 threshold).
fn views_py(log: bool) -> String {
    let call_log = |arg: &str| {
        if log {
            format!("    log({arg})\n")
        } else {
            String::new()
        }
    };
    let mut s = String::from("from flask import Flask\n\nfrom app import repo\n");
    if log {
        s.push_str("from app.log import log\n");
    }
    s.push_str("\napp = Flask(__name__)\n\n\n");
    s.push_str("@app.route('/orders')\n@app.route('/v2/orders')\ndef list_orders():\n");
    s.push_str(&call_log("'list'"));
    s.push_str("    rows = repo.find()\n    return {'rows': rows, 'n': repo.count()}\n\n\n");
    for h in HELPERS.iter().chain(&["audit_create", "audit_update"]) {
        s.push_str(&format!("def {h}(x):\n{}    return x\n\n\n", call_log("x")));
    }
    for (route, method, name, audit) in [
        ("/orders", "POST", "create_order", "audit_create"),
        ("/orders/<id>", "PUT", "update_order", "audit_update"),
    ] {
        s.push_str(&format!(
            "@app.route('{route}', methods=['{method}'])\ndef {name}():\n{}    x = {{}}\n",
            call_log("'write'")
        ));
        for h in HELPERS {
            s.push_str(&format!("    x = {h}(x)\n"));
        }
        s.push_str(&format!("    return {audit}(x)\n\n\n"));
    }
    s.push_str(&format!(
        "@app.route('/health')\ndef health():\n{}    return 'ok'\n",
        call_log("'health'")
    ));
    s
}

fn repo_py(log: bool) -> String {
    if log {
        "from app.log import log\n\n\ndef find():\n    log('find')\n    return []\n\n\ndef count():\n    log('count')\n    return 0\n".to_string()
    } else {
        "def find():\n    return []\n\n\ndef count():\n    return 0\n".to_string()
    }
}

const TESTS_PY: &str =
    "from app.views import list_orders\n\n\ndef test_list():\n    list_orders()\n";
const LOG_PY: &str = "def log(msg):\n    return msg\n";

struct Built {
    _tmp: tempfile::TempDir,
    merged: MergedGraph,
    labels: BTreeMap<u64, String>,
    /// 1-based line of `def list_orders` in `app/views.py`.
    list_orders_line: i64,
}

fn build(log: bool) -> Built {
    let tmp = tempfile::tempdir().expect("tempdir");
    let views = views_py(log);
    let mut files = vec![
        ("app/__init__.py", String::new()),
        ("app/views.py", views.clone()),
        ("app/repo.py", repo_py(log)),
        ("tests/test_orders.py", TESTS_PY.to_string()),
    ];
    if log {
        files.push(("app/log.py", LOG_PY.to_string()));
    }
    for (rel, src) in &files {
        let p = tmp.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("mkdir");
        std::fs::write(p, src).expect("write source");
    }
    let list_orders_line = views
        .lines()
        .position(|l| l.starts_with("def list_orders"))
        .expect("the handler def") as i64
        + 1;
    let r = generate_one(tmp.path().to_str().expect("utf-8 temp path")).expect("generate_one");
    Built {
        _tmp: tmp,
        merged: r.merged,
        labels: r.repo_labels,
        list_orders_line,
    }
}

fn run(b: &Built, f: impl FnOnce(&mut DupFlowArgs)) -> DuplicateFlows {
    let mut a = DupFlowArgs::default();
    f(&mut a);
    duplicate_flows(&b.merged, &b.labels, &a)
}

fn entries(g: &DupFlowGroup) -> Vec<&str> {
    g.entries.iter().map(|e| e.qname.as_str()).collect()
}

fn differing(g: &DupFlowGroup) -> BTreeSet<&str> {
    g.differing.iter().map(|e| e.qname.as_str()).collect()
}

fn views(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| format!("app::views::{n}")).collect()
}

/// (kind, tier, entries, jaccard, shared, union, differing).
type Shape = (
    String,
    String,
    Vec<String>,
    String,
    usize,
    usize,
    Vec<String>,
);

/// The groups without node ids or locations: what a hub must not move.
fn shape(a: &DuplicateFlows) -> Vec<Shape> {
    a.groups
        .iter()
        .map(|g| {
            (
                g.kind.to_string(),
                g.tier.to_string(),
                entries(g).iter().map(|s| s.to_string()).collect(),
                format!("{:.6}", g.jaccard),
                g.shared,
                g.union,
                differing(g).iter().map(|s| s.to_string()).collect(),
            )
        })
        .collect()
}

fn json(a: &DuplicateFlows) -> String {
    serde_json::to_string(a).expect("serialise")
}

#[test]
fn defaults() {
    let a = DupFlowArgs::default();
    assert_eq!(
        (
            a.depth,
            a.threshold,
            a.min_size,
            a.include_tests,
            a.keep_hubs
        ),
        (6, 0.8, 3, false, false)
    );
    assert_eq!((a.scope.as_deref(), a.surface), (None, "engine"));
    assert_eq!(a.seed, glia_engine::duplicate_flows::DEFAULT_SEED);
}

/// The fixture's flow sets are exactly the ones the Jaccard values below are
/// computed from: an extractor change that adds an edge fails here, loudly,
/// instead of moving a Jaccard.
#[test]
fn flow_sets_are_the_measured_shape() {
    let b = build(false);
    let flows = entry_flows(&b.merged, &b.labels, 6);
    let set = |qname: &str| -> BTreeSet<String> {
        flows
            .iter()
            .find(|f| f.entry.qname == qname)
            .unwrap_or_else(|| panic!("no flow for {qname}: {flows:#?}"))
            .hops
            .iter()
            .map(|h| h.to_qname.clone())
            .collect()
    };
    let list: BTreeSet<String> = [
        "app::views::list_orders",
        "app::repo::find",
        "app::repo::count",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(set("GET /orders"), list);
    assert_eq!(set("GET /v2/orders"), list);
    assert_eq!(set("tests::test_orders::test_list"), list);
    let mut post: Vec<&str> = HELPERS.to_vec();
    post.extend(["create_order", "audit_create"]);
    let mut put: Vec<&str> = HELPERS.to_vec();
    put.extend(["update_order", "audit_update"]);
    let (post, put) = (set("POST /orders"), set("PUT /orders/<id>"));
    assert_eq!(post.len(), 16, "{post:#?}");
    assert_eq!(
        post,
        views(&[&HELPERS[..], &["create_order", "audit_create"]].concat())
    );
    assert_eq!(
        put,
        views(&[&HELPERS[..], &["update_order", "audit_update"]].concat())
    );
    assert_eq!(post.intersection(&put).count(), 14);
    assert_eq!(post.union(&put).count(), 18);
}

#[test]
fn exact_alias() {
    let b = build(false);
    let ans = run(&b, |_| {});
    assert_eq!(ans.groups.len(), 1, "{:#?}", ans.groups);
    let g = &ans.groups[0];
    assert_eq!((g.kind, g.tier), ("exact", "derived"));
    assert_eq!(entries(g), vec!["GET /orders", "GET /v2/orders"]);
    for e in &g.entries {
        assert_eq!(e.file.as_deref(), Some("app/views.py"), "{e:#?}");
        assert_eq!(
            e.line,
            Some(b.list_orders_line),
            "a Flask ROUTE sits at its handler's def: {e:#?}"
        );
        assert_eq!(e.kind, "ROUTE");
    }
    assert_eq!(g.jaccard, 1.0);
    assert_eq!((g.shared, g.union), (3, 3));
    assert!(g.differing.is_empty());
    assert_eq!(g.services, vec!["app"]);
    assert!(ans.absence.is_none());
    assert_eq!(ans.hubs_ignored, 0, "no node reaches the hub threshold");
    // GET /orders, GET /v2/orders, GET /health, POST, PUT: test_list is out.
    assert_eq!(ans.entries, 5);
    // GET /health reaches 1 node (< min_size 3); the rest are flows.
    assert_eq!(ans.flows, 4);
}

#[test]
fn near_pair() {
    let b = build(false);
    let ans = run(&b, |a| a.threshold = 0.7);
    assert_eq!(ans.groups.len(), 2, "{:#?}", ans.groups);
    assert_eq!(ans.groups[0].kind, "exact", "exact groups sort first");
    let g = &ans.groups[1];
    assert_eq!((g.kind, g.tier), ("near", "heuristic"));
    assert_eq!(entries(g), vec!["POST /orders", "PUT /orders/<id>"]);
    assert_eq!(g.jaccard, 14.0 / 18.0);
    assert_eq!((g.shared, g.union), (14, 18));
    let want = views(&[
        "create_order",
        "update_order",
        "audit_create",
        "audit_update",
    ]);
    let got: BTreeSet<String> = differing(g).into_iter().map(String::from).collect();
    assert_eq!(got, want);
    assert!(ans.candidates >= 1);

    // 14/18 = 0.778 is below the default 0.8.
    let strict = run(&b, |a| a.threshold = 0.8);
    assert!(
        strict.groups.iter().all(|g| g.kind == "exact"),
        "{:#?}",
        strict.groups
    );
}

#[test]
fn tests_excluded() {
    let b = build(false);
    let test = "tests::test_orders::test_list";
    let ans = run(&b, |_| {});
    assert!(
        ans.groups
            .iter()
            .all(|g| g.entries.iter().all(|e| e.qname != test)),
        "{:#?}",
        ans.groups
    );
    let with = run(&b, |a| a.include_tests = true);
    let exact: Vec<&DupFlowGroup> = with.groups.iter().filter(|g| g.kind == "exact").collect();
    assert_eq!(exact.len(), 1, "{:#?}", with.groups);
    assert_eq!(
        entries(exact[0]),
        vec!["GET /orders", "GET /v2/orders", test]
    );
    assert_eq!((exact[0].shared, exact[0].union), (3, 3));
    assert_eq!(with.entries, 6);
}

#[test]
fn hubs_ignored() {
    let plain = build(false);
    let logged = build(true);
    for threshold in [0.8, 0.7] {
        let a = run(&plain, |a| a.threshold = threshold);
        let b = run(&logged, |a| a.threshold = threshold);
        assert_eq!(shape(&a), shape(&b), "threshold {threshold}");
        assert!(b.hubs_ignored >= 1, "log is a utility hub: {b:#?}");
    }
    // Kept, the hub joins every flow: the alias group grows to 4 shared
    // nodes, the near pair to 15 of 19.
    let kept = run(&logged, |a| {
        a.threshold = 0.7;
        a.keep_hubs = true;
    });
    assert_eq!(kept.hubs_ignored, 0);
    assert_eq!(kept.groups.len(), 2, "{:#?}", kept.groups);
    assert_eq!((kept.groups[0].shared, kept.groups[0].union), (4, 4));
    assert_eq!(kept.groups[1].jaccard, 15.0 / 19.0);
}

#[test]
fn no_duplicates_is_an_absence() {
    let b = build(false);
    let ans = run(&b, |a| a.scope = Some("tests".into()));
    assert!(ans.groups.is_empty(), "{:#?}", ans.groups);
    let absence = ans.absence.expect("an empty answer carries an absence");
    assert_eq!((absence.tier, absence.reason), ("FACT", "no_match"));
    assert!(absence.note.contains("0.8"), "{}", absence.note);
}

#[test]
fn scope_picks_entries() {
    let b = build(false);
    let ans = run(&b, |a| {
        a.scope = Some("app".into());
        a.threshold = 0.7;
    });
    assert_eq!(ans.groups.len(), 2, "{:#?}", ans.groups);
    let with = run(&b, |a| {
        a.scope = Some("tests".into());
        a.include_tests = true;
    });
    // One entry in scope: nothing to pair it with.
    assert_eq!(with.entries, 1);
    assert!(with.groups.is_empty());
}

#[test]
fn deterministic() {
    let a = build(false);
    let first = json(&run(&a, |a| a.threshold = 0.7));
    assert_eq!(first, json(&run(&a, |a| a.threshold = 0.7)), "two calls");
    // A NodeId hashes the repo's path, so a build in another temp dir has
    // other ids; everything else is byte-identical.
    let b = build(false);
    assert_eq!(
        without_ids(&first),
        without_ids(&json(&run(&b, |a| a.threshold = 0.7))),
        "two builds"
    );
}

/// `json` with every `id` field removed.
fn without_ids(json: &str) -> String {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("id");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v: serde_json::Value = serde_json::from_str(json).expect("json");
    strip(&mut v);
    v.to_string()
}
