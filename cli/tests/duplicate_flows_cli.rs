//! CD.4f — `glia duplicate-flows`, driving the real binary over CD.4e's
//! acceptance tree (engine/tests/duplicate_flows.rs, `views_py(false)`,
//! `repo_py(false)`, `TESTS_PY`): one Flask repo whose `app/views.py` stacks
//! `@app.route('/orders')` and `@app.route('/v2/orders')` on one handler
//! `list_orders` (calling `repo.find()` / `repo.count()`), and whose
//! `create_order` (POST /orders) and `update_order` (PUT /orders/<id>) call
//! the same 14 helpers plus one private helper each, so their flows share 14
//! of 18 nodes (Jaccard 0.778). GET /health is unrelated;
//! `tests/test_orders.py` `test_list()` calls `list_orders()`.
//!
//! The engine's `[dupflows] entries=<E> ... surface=cli` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract.
//!
//! `thread_count_invariant`: the rayon pool is sized once per process, so it
//! runs the binary twice as child processes (`GLIA_THREADS=1`, then unset)
//! and compares the `--json` bytes, as `parallel_cli.rs` does.
//!
//! Before CD.4f `glia duplicate-flows` was an unrecognised subcommand
//! (exit 2).

use std::path::PathBuf;
use std::process::{Command, Output};

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

/// `app/views.py`: CD.4e's `views_py(false)`.
fn views_py() -> String {
    let mut s = String::from("from flask import Flask\n\nfrom app import repo\n");
    s.push_str("\napp = Flask(__name__)\n\n\n");
    s.push_str("@app.route('/orders')\n@app.route('/v2/orders')\ndef list_orders():\n");
    s.push_str("    rows = repo.find()\n    return {'rows': rows, 'n': repo.count()}\n\n\n");
    for h in HELPERS.iter().chain(&["audit_create", "audit_update"]) {
        s.push_str(&format!("def {h}(x):\n    return x\n\n\n"));
    }
    for (route, method, name, audit) in [
        ("/orders", "POST", "create_order", "audit_create"),
        ("/orders/<id>", "PUT", "update_order", "audit_update"),
    ] {
        s.push_str(&format!(
            "@app.route('{route}', methods=['{method}'])\ndef {name}():\n    x = {{}}\n"
        ));
        for h in HELPERS {
            s.push_str(&format!("    x = {h}(x)\n"));
        }
        s.push_str(&format!("    return {audit}(x)\n\n\n"));
    }
    s.push_str("@app.route('/health')\ndef health():\n    return 'ok'\n");
    s
}

const REPO_PY: &str = "def find():\n    return []\n\n\ndef count():\n    return 0\n";
const TESTS_PY: &str =
    "from app.views import list_orders\n\n\ndef test_list():\n    list_orders()\n";

/// 1-based line of the first line of `views_py()` starting with `prefix`.
fn views_line(prefix: &str) -> usize {
    views_py()
        .lines()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line {prefix:?} in views.py"))
        + 1
}

/// A fresh temp root holding the fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cd4f-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let files = [
            ("app/__init__.py", String::new()),
            ("app/views.py", views_py()),
            ("app/repo.py", REPO_PY.to_string()),
            ("tests/test_orders.py", TESTS_PY.to_string()),
        ];
        for (rel, src) in &files {
            let path = p.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, src).expect("write source");
        }
        Root(p)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 temp path")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `glia duplicate-flows <args>` with `GLIA_THREADS` set to `threads`, or
/// unset.
fn run(args: &[&str], threads: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("duplicate-flows")
        .args(args)
        .env("GLIA_NO_PERSIST", "1");
    match threads {
        Some(t) => cmd.env("GLIA_THREADS", t),
        None => cmd.env_remove("GLIA_THREADS"),
    };
    cmd.output().expect("glia runs")
}

/// Run `glia duplicate-flows <args>`, relaying the fired_on marker, and
/// check the exit code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = run(args, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia duplicate-flows {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[dupflows\] '` sees it.
    for line in stderr.lines().filter(|l| l.starts_with("[dupflows] ")) {
        eprintln!("{line}");
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn markers(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[dupflows] "))
        .map(str::to_string)
        .collect()
}

/// The lines under the heading starting `head`, up to the next heading.
fn block<'a>(text: &'a str, head: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|l| !l.starts_with(head))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| !l.is_empty())
        .collect()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("--json parses")
}

fn qnames(rows: &serde_json::Value) -> Vec<&str> {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|r| r["qname"].as_str().expect("qname"))
        .collect()
}

#[test]
fn alias_and_near() {
    let root = Root::new("alias");
    let out = glia(&[root.path(), "--threshold", "0.7"], 0);
    let text = stdout(&out);
    let list = views_line("def list_orders");
    let create = views_line("def create_order");
    let update = views_line("def update_order");

    let exact = "## exact (derived): GET /orders = GET /v2/orders  (3 shared nodes)";
    assert!(text.contains(exact), "{text}");
    assert_eq!(
        block(&text, exact),
        vec![
            format!("- `GET /orders`  ROUTE  app/views.py:{list}").as_str(),
            format!("- `GET /v2/orders`  ROUTE  app/views.py:{list}").as_str(),
            "- services: app",
        ],
        "{text}"
    );

    let near = "## near (heuristic, jaccard 0.78): POST /orders ~ PUT /orders/<id>  (14 shared nodes of 18)";
    assert!(text.contains(near), "{text}");
    let rows = block(&text, near);
    assert_eq!(
        rows[..3],
        [
            format!("- `POST /orders`  ROUTE  app/views.py:{create}").as_str(),
            format!("- `PUT /orders/<id>`  ROUTE  app/views.py:{update}").as_str(),
            "- services: app",
        ],
        "{text}"
    );
    let differs: Vec<&str> = rows
        .iter()
        .filter(|l| l.starts_with("- differs by "))
        .copied()
        .collect();
    assert_eq!(
        differs,
        vec![
            format!(
                "- differs by `app::views::audit_create`  FUNCTION  app/views.py:{}",
                views_line("def audit_create")
            )
            .as_str(),
            format!(
                "- differs by `app::views::audit_update`  FUNCTION  app/views.py:{}",
                views_line("def audit_update")
            )
            .as_str(),
            format!("- differs by `app::views::create_order`  FUNCTION  app/views.py:{create}")
                .as_str(),
            format!("- differs by `app::views::update_order`  FUNCTION  app/views.py:{update}")
                .as_str(),
        ],
        "{text}"
    );
    assert!(
        text.find(exact) < text.find(near),
        "exact groups first:\n{text}"
    );
    assert!(
        text.contains(
            "- 5 entries compared (test entries left out), 4 flows of >= 3 nodes within 6 hops; 0 utility hub(s) left out"
        ),
        "{text}"
    );
    assert!(
        text.contains("- 1 exact group(s), 1 near group(s) at jaccard >= 0.7 from "),
        "{text}"
    );

    let m = markers(&out);
    assert_eq!(m.len(), 1, "one marker per answer: {m:?}");
    assert!(
        m[0].starts_with("[dupflows] entries=5 flows=4 exact_groups=1 near_groups=1 ")
            && m[0].ends_with(" threshold=0.7 surface=cli"),
        "{}",
        m[0]
    );

    // 14/18 = 0.778 is below the default 0.8: only the alias group.
    let text = stdout(&glia(&[root.path()], 0));
    assert!(text.contains(exact), "{text}");
    assert!(!text.contains("## near "), "{text}");
}

#[test]
fn threshold_outside_zero_one_is_refused() {
    let root = Root::new("threshold");
    for bad in ["1.5", "0", "-0.2", "NaN"] {
        let out = glia(&[root.path(), &format!("--threshold={bad}")], 2);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("error: --threshold must be in (0, 1], got "),
            "{bad}: {err}"
        );
        assert!(markers(&out).is_empty(), "refused before the engine runs");
        assert!(out.stdout.is_empty(), "{bad}");
    }
    let out = glia(&[root.path(), "--threshold", "1"], 0);
    assert!(stdout(&out).contains("## exact (derived): "));
}

#[test]
fn json_is_the_engine_answer() {
    let root = Root::new("json");
    let out = glia(&[root.path(), "--json", "--threshold", "0.7"], 0);
    // The answer's keys in engine field order (serde_json::Value sorts them,
    // so read the text).
    let raw = stdout(&out);
    assert!(
        raw.starts_with("{\"entries\":5,\"flows\":4,\"hubs_ignored\":0,\"candidates\":"),
        "{raw}"
    );
    let at: Vec<usize> = [
        ",\"oversized_buckets\":",
        ",\"groups\":[{\"kind\":\"exact\",\"tier\":\"derived\",\"entries\":[",
        "],\"jaccard\":1.0,\"shared\":3,\"union\":3,\"differing\":[],\"services\":[\"app\"]}",
        ",{\"kind\":\"near\",\"tier\":\"heuristic\",\"entries\":[",
        ",\"absence\":null}",
    ]
    .iter()
    .map(|k| raw.find(k).unwrap_or_else(|| panic!("{k} in {raw}")))
    .collect();
    assert!(
        at.windows(2).all(|w| w[0] < w[1]),
        "key order {at:?}: {raw}"
    );
    let v = json(&out);
    let groups = v["groups"].as_array().expect("groups");
    assert_eq!(groups.len(), 2, "{v}");
    assert_eq!(
        qnames(&groups[0]["entries"]),
        vec!["GET /orders", "GET /v2/orders"]
    );
    let e = &groups[0]["entries"][0];
    assert_eq!(
        (&e["file"], &e["line"], &e["kind"]),
        (
            &serde_json::json!("app/views.py"),
            &serde_json::json!(views_line("def list_orders")),
            &serde_json::json!("ROUTE")
        ),
        "{e}"
    );
    assert_eq!(
        qnames(&groups[1]["entries"]),
        vec!["POST /orders", "PUT /orders/<id>"]
    );
    assert_eq!(groups[1]["jaccard"], serde_json::json!(14.0 / 18.0));
    assert_eq!(
        (&groups[1]["shared"], &groups[1]["union"]),
        (&14.into(), &18.into())
    );
    assert_eq!(groups[1]["differing"].as_array().expect("rows").len(), 4);
    assert!(markers(&out)[0].ends_with(" surface=cli"));
}

#[test]
fn flags_reach_the_engine() {
    let root = Root::new("flags");
    let test = "tests::test_orders::test_list";

    let with = json(&glia(&[root.path(), "--json", "--include-tests"], 0));
    assert_eq!(with["entries"], 6, "{with}");
    assert_eq!(
        qnames(&with["groups"][0]["entries"]),
        vec!["GET /orders", "GET /v2/orders", test]
    );
    let text = stdout(&glia(&[root.path(), "--include-tests"], 0));
    assert!(
        text.contains(&format!(
            "## exact (derived): GET /orders = GET /v2/orders = {test}  (3 shared nodes)"
        )) && text.contains("(test entries included)"),
        "{text}"
    );

    // One hop from a route reaches its handler only: every flow is 1 node,
    // under --min-size 3, unless --min-size 1 keeps them.
    let shallow = json(&glia(&[root.path(), "--json", "--depth", "1"], 0));
    assert_eq!(shallow["flows"], 0, "{shallow}");
    let kept = json(&glia(
        &[root.path(), "--json", "--depth", "1", "--min-size", "1"],
        0,
    ));
    assert_eq!(kept["flows"], 5, "{kept}");

    let big = json(&glia(&[root.path(), "--json", "--min-size", "4"], 0));
    assert_eq!(big["flows"], 2, "only POST and PUT reach 4+ nodes: {big}");

    let hubs = stdout(&glia(&[root.path(), "--keep-hubs"], 0));
    assert!(hubs.contains("; utility hubs kept"), "{hubs}");

    let scoped = json(&glia(
        &[
            root.path(),
            "--json",
            "--scope",
            "app",
            "--threshold",
            "0.7",
        ],
        0,
    ));
    assert_eq!(scoped["groups"].as_array().expect("groups").len(), 2);
}

#[test]
fn no_groups_is_an_absence() {
    let root = Root::new("none");
    // One test entry in scope: nothing to pair it with. A report: exit 0.
    let out = glia(&[root.path(), "--scope", "tests", "--include-tests"], 0);
    let text = stdout(&out);
    assert!(text.contains("_(no duplicate flows)_"), "{text}");
    assert!(
        text.contains("> FACT: no two of the 1 entry flows (1 entries compared"),
        "{text}"
    );
    assert!(!text.contains("## "), "{text}");
    let v = json(&glia(&[root.path(), "--json", "--scope", "tests"], 0));
    assert_eq!(v["groups"], serde_json::json!([]));
    assert_eq!(v["absence"]["reason"], "no_match", "{v}");
    assert_eq!(v["absence"]["tier"], "FACT", "{v}");
}

#[test]
fn thread_count_invariant() {
    let root = Root::new("threads");
    let args = [
        root.path(),
        "--json",
        "--threshold",
        "0.7",
        "--include-tests",
    ];
    let one = run(&args, Some("1"));
    let all = run(&args, None);
    for (name, out) in [("GLIA_THREADS=1", &one), ("GLIA_THREADS unset", &all)] {
        assert_eq!(
            out.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!one.stdout.is_empty());
    assert!(
        one.stdout == all.stdout,
        "--json differs by pool size:\n1 thread:\n{}\nevery core:\n{}",
        stdout(&one),
        stdout(&all)
    );
}
