//! CC.4c — `glia pack`, driving the real binary over CC.4b's acceptance tree
//! (engine/tests/pack.rs): `shop/a.py` holds a documented `price(o)` and
//! `place(o)`, which calls it; `shop/b.py` imports `place` and `checkout(o)`
//! calls it; `shop/util.py` is an unrelated `format_report()`.
//!
//! stdout carries the pack text only; the engine's `[pack] query=...` line
//! (the fired_on marker) and the `packed ...` summary go to stderr, which
//! asserting them here makes a tested contract.

use std::path::PathBuf;
use std::process::{Command, Output};

const A_PY: &str = "def price(o):
    \"\"\"Price an order.

    Sums the line totals, then takes the discount off.
    \"\"\"
    total = 0
    for line in o.lines:
        total = total + line.qty * line.unit
    if o.discount:
        total = total - o.discount
    total = round(total, 2)
    return total


def place(o):
    return price(o)
";

const B_PY: &str = "from shop.a import place


def checkout(o):
    return place(o)
";

const UTIL_PY: &str = "def format_report():
    rows = []
    rows.append(\"report\")
    return \"\\n\".join(rows)
";

/// The `--json` keys, in the engine's field order.
const PACK_KEYS: [&str; 11] = [
    "query",
    "text",
    "budget_tokens",
    "used_tokens",
    "bytes",
    "bytes_per_token",
    "candidates",
    "nodes",
    "dropped",
    "rerenders",
    "absence",
];

/// A fresh temp root holding the fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cc4c-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        for (rel, src) in [
            ("shop/a.py", A_PY),
            ("shop/b.py", B_PY),
            ("shop/util.py", UTIL_PY),
            (
                "pyproject.toml",
                "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
            ),
        ] {
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

/// Run `glia pack <args>`, relay the fired_on marker, and check the exit code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .arg("pack")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    let err = stderr(&out);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia pack {args:?}\nstdout:\n{}\nstderr:\n{err}",
        stdout(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[pack\] '` sees it.
    for line in err.lines().filter(|l| l.starts_with("[pack] ")) {
        eprintln!("{line}");
    }
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The stderr lines starting with `prefix`.
fn lines_with(out: &Output, prefix: &str) -> Vec<String> {
    stderr(out)
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

#[test]
fn text_to_stdout_summary_to_stderr() {
    let root = Root::new("text");
    let out = glia(&[root.path(), "price", "--budget", "100000"], 0);
    let text = stdout(&out);
    assert!(text.starts_with("# context for price\n"), "{text}");
    assert!(
        text.contains("### shop::a::price (FUNCTION shop/a.py:1-"),
        "{text}"
    );
    assert!(text.contains("    return total"), "the Full block: {text}");
    assert!(
        !text.contains("[pack]") && !text.contains("packed "),
        "stdout is the pack only: {text}"
    );

    let marker = lines_with(&out, "[pack] query=price ");
    assert_eq!(marker.len(), 1, "{}", stderr(&out));
    assert!(
        marker[0].starts_with("[pack] query=price seeds=1 candidates=")
            && marker[0].contains(" (full=")
            && marker[0].contains("/100000 bytes=")
            && marker[0].ends_with(" bpt=3.7 rerenders=0"),
        "{}",
        marker[0]
    );

    let summary = lines_with(&out, "packed ");
    assert_eq!(summary.len(), 1, "{}", stderr(&out));
    let s = &summary[0];
    assert!(
        s.contains(" full, ")
            && s.contains(" preview, ")
            && s.contains(" outline, ")
            && s.contains(" qname) in ")
            && s.contains("/100000 tokens (est. 3.7 bytes/token); ")
            && s.ends_with(" dropped"),
        "{s}"
    );
    // The summary and --json agree on the counts.
    let json: serde_json::Value = serde_json::from_slice(
        &glia(&[root.path(), "price", "--budget", "100000", "--json"], 0).stdout,
    )
    .expect("--json parses");
    let nodes = json["nodes"].as_array().expect("nodes").len();
    assert!(
        s.starts_with(&format!("packed {nodes} nodes (")),
        "{s} vs {nodes} nodes"
    );
    assert_eq!(
        json["text"].as_str(),
        Some(text.as_str()),
        "stdout is exactly `text`"
    );
}

#[test]
fn json_is_the_whole_pack() {
    let root = Root::new("json");
    let out = glia(&[root.path(), "price", "--budget", "100000", "--json"], 0);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("--json parses");
    let obj = json.as_object().expect("an object");
    let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    let mut want = PACK_KEYS.to_vec();
    want.sort_unstable();
    let mut got = keys.clone();
    got.sort_unstable();
    assert_eq!(got, want, "{json}");
    assert_eq!(json["bytes_per_token"], "3.7");
    assert_eq!(json["budget_tokens"], 100_000);
    assert_eq!(json["query"], "price");
    assert!(json["absence"].is_null(), "{json}");
    let first = &json["nodes"][0];
    assert_eq!(first["qname"], "shop::a::price", "{json}");
    assert_eq!(first["fidelity"], "full");
    assert_eq!(first["file"], "shop/a.py");
    assert_eq!(first["line"], 1, "1-based (LD.1)");
    assert_eq!(first["reason"], "seed");
    assert_eq!(
        json["bytes"].as_u64(),
        json["text"].as_str().map(|t| t.len() as u64)
    );
    assert_eq!(lines_with(&out, "[pack] query=price ").len(), 1);
    assert_eq!(lines_with(&out, "packed ").len(), 1);
}

#[test]
fn flags_reach_the_engine() {
    let root = Root::new("flags");
    let out = glia(
        &[
            root.path(),
            "price",
            "--budget",
            "5000",
            "--bytes-per-token",
            "4",
            "--seeds",
            "1",
            "--candidates",
            "2",
            "--preset",
            "repair",
            "--scope",
            "shop",
            "--json",
        ],
        0,
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("--json parses");
    assert_eq!(json["bytes_per_token"], "4.0");
    assert_eq!(json["budget_tokens"], 5000);
    assert_eq!(json["candidates"], 2, "{json}");
    let marker = lines_with(&out, "[pack] query=price ");
    assert!(
        marker.len() == 1
            && marker[0].contains(" candidates=2 ")
            && marker[0].contains(" bpt=4.0 "),
        "{marker:?}"
    );
    assert_eq!(
        lines_with(&out, "[scope] pack scope=").len(),
        1,
        "{}",
        stderr(&out)
    );
}

#[test]
fn bytes_per_token_out_of_range_is_a_usage_error() {
    let root = Root::new("bpt");
    for bad in ["0.5", "20.1", "3.75", "abc"] {
        let out = glia(&[root.path(), "price", "--bytes-per-token", bad], 2);
        assert!(stdout(&out).is_empty(), "{bad}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("between 1.0 and 20.0"),
            "{bad}: {}",
            stderr(&out)
        );
    }
    let out = glia(&[root.path(), "price", "--preset", "no_such_preset"], 2);
    assert!(
        stderr(&out).contains("repair"),
        "clap lists the table's presets: {}",
        stderr(&out)
    );
}

#[test]
fn no_match_exits_1_with_the_absence() {
    let root = Root::new("none");
    let out = glia(&[root.path(), "zzz_nothing"], 1);
    assert!(stdout(&out).is_empty(), "stdout: {}", stdout(&out));
    assert_eq!(lines_with(&out, "> FACT: ").len(), 1, "{}", stderr(&out));
    assert_eq!(
        lines_with(
            &out,
            "packed 0 nodes (0 full, 0 preview, 0 outline, 0 qname) in 0/8000 tokens"
        )
        .len(),
        1,
        "{}",
        stderr(&out)
    );

    let out = glia(&[root.path(), "zzz_nothing", "--json"], 1);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("--json parses");
    assert_eq!(json["absence"]["reason"], "no_match", "{json}");
    assert_eq!(json["text"], "");
    assert_eq!(json["nodes"].as_array().map(Vec::len), Some(0));
}
