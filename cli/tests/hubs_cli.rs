//! CD.4c — `glia hubs`, driving the real binary over CD.4b's acceptance tree
//! (engine/tests/hubs.rs): `util/log.py` `log()` is called from 12 functions
//! across `svc_a/` and `svc_b/`; `svc_a/main.py` `main()` calls 10 steps;
//! `tests/` holds 30 `test_*` functions that each call `log` once (left out by
//! default, so `log` has 12 callers, 42 with `--include-tests`).
//!
//! The engine's `[hubs] nodes=<N> edges=<M> ... surface=cli` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract.
//!
//! `thread_count_invariant` is CD.4b's handed-off determinism check: the rayon
//! pool is sized once per process, so it runs the binary twice as child
//! processes (`GLIA_THREADS=1`, then unset) and compares the `--json` bytes.

use std::path::PathBuf;
use std::process::{Command, Output};

const LOG_PY: &str = "def log(msg):\n    return msg\n";

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

/// A fresh temp root holding the fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cd4c-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let files = [
            ("util/log.py", LOG_PY.to_string()),
            ("svc_a/handlers.py", handlers("a")),
            ("svc_b/handlers.py", handlers("b")),
            ("svc_a/steps.py", steps()),
            ("svc_a/main.py", main_py()),
            ("tests/test_log.py", tests_py()),
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

/// `glia hubs <args>` with `GLIA_THREADS` set to `threads`, or unset.
fn run(args: &[&str], threads: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("hubs").args(args).env("GLIA_NO_PERSIST", "1");
    match threads {
        Some(t) => cmd.env("GLIA_THREADS", t),
        None => cmd.env_remove("GLIA_THREADS"),
    };
    cmd.output().expect("glia runs")
}

/// Run `glia hubs <args>`, relaying the fired_on marker, and check the exit
/// code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = run(args, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia hubs {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[hubs\] '` sees it.
    for line in stderr.lines().filter(|l| l.starts_with("[hubs] ")) {
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
        .filter(|l| l.starts_with("[hubs] "))
        .map(str::to_string)
        .collect()
}

/// The table rows (`| 1 | ...`) under `## <title>`, up to the next section.
fn rows<'a>(text: &'a str, title: &str) -> Vec<&'a str> {
    let head = format!("## {title}");
    text.lines()
        .skip_while(|l| *l != head)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("| ") && !l.starts_with("| # "))
        .collect()
}

/// A table row's cells, trimmed, the outer pipes dropped.
fn cells(row: &str) -> Vec<&str> {
    let inner = row.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(str::trim).collect()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("--json parses")
}

#[test]
fn utility_and_orchestrator() {
    let root = Root::new("tables");
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);

    let fan_in = rows(&text, "Fan-in");
    let first = cells(
        fan_in
            .first()
            .unwrap_or_else(|| panic!("a fan-in row:\n{text}")),
    );
    assert_eq!(
        first,
        vec![
            "1",
            "`util::log::log`",
            "FUNCTION",
            "utility",
            "12",
            "0",
            "in svc_a, svc_b",
            "util/log.py:1",
            first[8],
            "0.000",
        ],
        "{text}"
    );
    assert!(
        first[8].parse::<f64>().is_ok_and(|a| a > 0.0) && first[8].len() == 5,
        "authority to 3 decimals: {}",
        first[8]
    );
    assert!(
        fan_in.iter().all(|r| !r.contains("`tests::")),
        "test nodes are left out:\n{text}"
    );

    let fan_out = rows(&text, "Fan-out");
    let first = cells(
        fan_out
            .first()
            .unwrap_or_else(|| panic!("a fan-out row:\n{text}")),
    );
    assert_eq!(
        (first[1], first[3], first[4], first[5], first[6], first[7]),
        (
            "`svc_a::main::main`",
            "orchestrator",
            "0",
            "10",
            "out svc_a",
            "svc_a/main.py:4"
        ),
        "{text}"
    );

    let cross = rows(&text, "Cross-service");
    assert!(
        cross.iter().any(|r| r.contains("`util::log::log`")),
        "log joins two services:\n{text}"
    );
    let v = json(&glia(&[root.path(), "--json"], 0));
    let (p99_in, p99_out) = (
        v["p99_in"].as_u64().expect("p99_in"),
        v["p99_out"].as_u64().expect("p99_out"),
    );
    let footer = format!(
        "- thresholds: fan-in >= {} (p99 {p99_in}, --min-degree 5); fan-out >= {} (p99 {p99_out}, --min-degree 5); cross-service: callers or callees in 2+ services",
        p99_in.max(5),
        p99_out.max(5)
    );
    assert!(text.contains(&footer), "the p99 footer {footer:?}:\n{text}");

    let m = markers(&out);
    assert_eq!(m.len(), 1, "one marker per answer: {m:?}");
    assert!(
        m[0].contains(" hits_iters=20 surface=cli") && m[0].contains(" fan_in=1 "),
        "{}",
        m[0]
    );

    let out = glia(&[root.path(), "--json"], 0);
    let v = json(&out);
    // The answer's keys in engine field order (serde_json::Value sorts them,
    // so read the text): a row's `fan_out` follows a number, never `]`.
    let raw = stdout(&out);
    assert!(
        raw.starts_with("{\"fan_in\":[{\"qname\":\"util::log::log\","),
        "{raw}"
    );
    let at: Vec<usize> = [
        "],\"fan_out\":[",
        "],\"cross_service\":[",
        "],\"nodes\":",
        ",\"edges\":",
        ",\"p99_in\":",
        ",\"p99_out\":",
        ",\"absence\":null}",
    ]
    .iter()
    .map(|k| raw.find(k).unwrap_or_else(|| panic!("{k} in {raw}")))
    .collect();
    assert!(
        at.windows(2).all(|w| w[0] < w[1]),
        "key order {at:?}: {raw}"
    );
    assert_eq!(v["fan_in"][0]["qname"], "util::log::log");
    assert_eq!(v["fan_in"][0]["fan_in"], 12);
    assert_eq!(
        v["fan_in"][0]["by_category"],
        serde_json::json!([["CALLS", 12, 0]])
    );
    assert_eq!(v["fan_out"][0]["qname"], "svc_a::main::main");
    assert!(v["absence"].is_null());
    assert!(markers(&out)[0].ends_with(" surface=cli"));
}

#[test]
fn flags_reach_the_engine() {
    let root = Root::new("flags");
    let v = json(&glia(&[root.path(), "--json", "--include-tests"], 0));
    let log = v["fan_in"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|r| r["qname"] == "util::log::log")
        .expect("log");
    assert_eq!(log["fan_in"], 42, "the 30 test calls count: {log}");

    let calls = json(&glia(&[root.path(), "--json", "--category", "CALLS"], 0));
    let base = json(&glia(&[root.path(), "--json"], 0));
    assert_eq!(
        calls["fan_in"], base["fan_in"],
        "CALLS is every counted edge here"
    );

    let one = json(&glia(
        &[root.path(), "--json", "--include-tests", "--top", "1"],
        0,
    ));
    for list in ["fan_in", "fan_out", "cross_service"] {
        assert!(one[list].as_array().expect(list).len() <= 1, "{one}");
    }

    let scoped = json(&glia(&[root.path(), "--json", "--scope", "svc_a"], 0));
    assert_eq!(scoped["fan_out"][0]["qname"], "svc_a::main::main");
    assert!(
        !scoped["fan_in"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|r| r["qname"] == "util::log::log"),
        "{scoped}"
    );
}

#[test]
fn no_rows_is_an_absence() {
    let root = Root::new("none");
    // svc_b's handlers call only util: no row joins two services either.
    let out = glia(&[root.path(), "--min-degree", "50", "--scope", "svc_b"], 1);
    let text = stdout(&out);
    assert!(text.contains("_(no hubs)_"), "{text}");
    assert!(
        text.contains("> FACT: no node under scope `svc_b`"),
        "{text}"
    );
    let v = json(&glia(
        &[
            root.path(),
            "--json",
            "--min-degree",
            "50",
            "--scope",
            "svc_b",
        ],
        1,
    ));
    assert_eq!(v["absence"]["reason"], "no_match", "{v}");
}

#[test]
fn unknown_category_is_a_usage_error() {
    let root = Root::new("category");
    let out = glia(&[root.path(), "--category", "NO_SUCH"], 2);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("invalid value 'NO_SUCH'") && err.contains("CALLS"),
        "{err}"
    );
}

#[test]
fn thread_count_invariant() {
    let root = Root::new("threads");
    let one = run(
        &[root.path(), "--json", "--include-tests", "--top", "0"],
        Some("1"),
    );
    let all = run(
        &[root.path(), "--json", "--include-tests", "--top", "0"],
        None,
    );
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
