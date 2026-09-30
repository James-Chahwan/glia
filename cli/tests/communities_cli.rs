//! CD.1e — `glia communities`, driving the real binary over CD.1d's fixture
//! (engine/tests/communities.rs): one Python repo, two packages.
//! `pkg_a/core.py` defines `f0..f5`, each `fi` calling `f(i+1 mod 6)` and
//! `f(i+2 mod 6)`, `f3` also calling `pkg_b`'s `g1` (imported with
//! `from pkg_b.core import g1`), and `main()` calling `f0`; `pkg_b/core.py`
//! defines `g0..g5` wired the same way; both `__init__.py` files are empty.
//! As CD.1d measured it: two communities labelled `pkg_a::core` (8 nodes) and
//! `pkg_b::core` (7), 17 nodes, the two `__init__` modules isolated, and one
//! link each way of 2 edges (`CALLS 1, IMPORTS 1`).
//!
//! The engine's `[communities] method=<m> ... surface=cli` stderr line is the
//! fired_on marker; asserting it here makes it a tested contract.
//!
//! `thread_count_invariant`: the rayon pool is sized once per process, so it
//! runs the binary twice as child processes (`GLIA_THREADS=1`, then unset) and
//! compares the `--json` bytes (the `parallel_cli.rs` way).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `core.py` of one package: `<p>0..<p>5` in a ring with chords, `f3` also
/// calling the imported `g1`, and `main` in `pkg_a` (CD.1d's `core_py`).
fn core_py(p: char, import: Option<&str>) -> String {
    let mut s = import
        .map(|m| format!("from {m}.core import g1\n\n"))
        .unwrap_or_default();
    for i in 0..6 {
        let extra = if p == 'f' && i == 3 { "\n    g1()" } else { "" };
        s.push_str(&format!(
            "\ndef {p}{i}():\n    {p}{}()\n    {p}{}(){extra}\n\n",
            (i + 1) % 6,
            (i + 2) % 6
        ));
    }
    if p == 'f' {
        s.push_str("\ndef main():\n    f0()\n");
    }
    s
}

/// The 1-based line of `def main():` in `pkg_a/core.py`.
fn main_line() -> usize {
    core_py('f', Some("pkg_b"))
        .lines()
        .position(|l| l == "def main():")
        .expect("main is defined")
        + 1
}

/// A fresh temp root holding the fixture, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-cd1e-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let files = [
            ("pkg_a/__init__.py", String::new()),
            ("pkg_a/core.py", core_py('f', Some("pkg_b"))),
            ("pkg_b/__init__.py", String::new()),
            ("pkg_b/core.py", core_py('g', None)),
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

/// `glia communities <args>` with `GLIA_THREADS` set to `threads`, or unset.
fn run(args: &[&str], threads: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("communities")
        .args(args)
        .env("GLIA_NO_PERSIST", "1");
    match threads {
        Some(t) => cmd.env("GLIA_THREADS", t),
        None => cmd.env_remove("GLIA_THREADS"),
    };
    cmd.output().expect("glia runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run `glia communities <args>`, relaying the fired_on marker, and check the
/// exit code.
fn glia(args: &[&str], want: i32) -> Output {
    let out = run(args, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia communities {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    // Relay the marker so `-- --nocapture | grep '^\[communities\] '` sees it.
    for line in stderr.lines().filter(|l| l.starts_with("[communities] ")) {
        eprintln!("{line}");
    }
    out
}

fn markers(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[communities] "))
        .map(str::to_string)
        .collect()
}

/// The `## ` community headings of a table run.
fn blocks(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with("## ")).collect()
}

/// The lines of the `## <heading>` block, up to the next one.
fn block<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    text.lines()
        .skip_while(|l| *l != heading)
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .filter(|l| !l.is_empty())
        .collect()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("--json parses")
}

fn labels(v: &serde_json::Value) -> Vec<String> {
    v["communities"]
        .as_array()
        .expect("communities is a list")
        .iter()
        .map(|c| c["label"].as_str().expect("a label").to_string())
        .collect()
}

#[test]
fn two_packages() {
    let root = Root::new("tables");
    let out = glia(&[root.path()], 0);
    let text = stdout(&out);

    let m = markers(&out);
    assert_eq!(m.len(), 1, "one marker per answer: {m:?}");
    assert!(
        m[0].starts_with("[communities] method=leiden nodes=17 "),
        "{}",
        m[0]
    );
    for part in [" communities=2 ", " listed=2 ", " isolated=2 ", " seed=42 "] {
        assert!(m[0].contains(part), "{part} in {}", m[0]);
    }
    assert!(m[0].ends_with(" surface=cli"), "{}", m[0]);

    assert_eq!(
        blocks(&text),
        [
            "## 0 pkg_a::core (8 nodes, cohesion 0.92)",
            "## 1 pkg_b::core (7 nodes, cohesion 0.92)",
        ],
        "{text}"
    );
    let header = text
        .lines()
        .find(|l| l.starts_with("- method "))
        .unwrap_or_else(|| panic!("a header line:\n{text}"));
    assert!(
        header.starts_with("- method leiden, modularity 0.")
            && header.ends_with(
                ", communities 2/2 (listed/total), isolated 2 of 17 nodes (seed 42, resolution 1)"
            ),
        "{header}"
    );

    let a = block(&text, "## 0 pkg_a::core (8 nodes, cohesion 0.92)");
    let main_at = format!("  - `pkg_a::core::main`  pkg_a/core.py:{}", main_line());
    assert_eq!(
        a[..7],
        [
            "- kinds: FUNCTION 7, MODULE 1",
            "- services: pkg_a 8",
            "- sinks: —",
            "- files: 1",
            "- entries:",
            main_at.as_str(),
            "- top members:",
        ],
        "{text}"
    );
    assert_eq!(
        a[7], "  - `pkg_a::core::f0`  FUNCTION  pkg_a/core.py:4",
        "the heaviest member first, located 1-based:\n{text}"
    );
    assert_eq!(a.iter().filter(|l| l.starts_with("  - `")).count(), 9);
    let links: Vec<&&str> = a
        .iter()
        .filter(|l| l.starts_with("  - -> #1 weight "))
        .collect();
    assert_eq!(links.len(), 1, "{text}");
    assert!(
        links[0].ends_with(" (CALLS 1, IMPORTS 1)"),
        "the call and the import cross: {}",
        links[0]
    );

    let b = block(&text, "## 1 pkg_b::core (7 nodes, cohesion 0.92)");
    assert!(b.contains(&"- entries: —"), "{text}");
    assert!(
        b.iter()
            .any(|l| l.starts_with("  - -> #0 weight ") && l.ends_with(" (CALLS 1, IMPORTS 1)")),
        "a link is listed from both ends:\n{text}"
    );

    let out = glia(&[root.path(), "--json"], 0);
    let raw = stdout(&out);
    assert!(
        raw.starts_with("{\"method\":\"leiden\",\"seed\":42,\"resolution\":1.0,\"modularity\":"),
        "engine field order: {raw}"
    );
    let v = json(&out);
    assert_eq!(v["communities"].as_array().map(Vec::len), Some(2), "{v}");
    assert_eq!(labels(&v), ["pkg_a::core", "pkg_b::core"]);
    assert_eq!(
        (
            v["total"].as_u64(),
            v["nodes"].as_u64(),
            v["isolated"].as_u64()
        ),
        (Some(2), Some(17), Some(2))
    );
    assert!(v["absence"].is_null(), "{v}");
    let l = &v["communities"][0]["links"][0];
    assert_eq!(
        (l["to"].as_u64(), l["edges"].as_u64()),
        (Some(1), Some(2)),
        "{l}"
    );
    assert_eq!(
        l["categories"],
        serde_json::json!([["CALLS", 1], ["IMPORTS", 1]])
    );
    let m = markers(&out);
    assert!(m.len() == 1 && m[0].ends_with(" surface=cli"), "{m:?}");
}

#[test]
fn options_reach_the_engine() {
    let root = Root::new("options");

    let text = stdout(&glia(&[root.path(), "--top", "1", "--members", "3"], 0));
    assert_eq!(
        blocks(&text),
        ["## 0 pkg_a::core (8 nodes, cohesion 0.92)"],
        "--top cuts the list:\n{text}"
    );
    assert!(
        text.contains(", communities 1/2 (listed/total), "),
        "{text}"
    );
    let a = block(&text, "## 0 pkg_a::core (8 nodes, cohesion 0.92)");
    let members: Vec<&&str> = a
        .iter()
        .skip_while(|l| **l != "- top members:")
        .skip(1)
        .take_while(|l| l.starts_with("  - "))
        .collect();
    assert_eq!(members.len(), 3, "--members 3:\n{text}");

    let out = glia(
        &[
            root.path(),
            "--method",
            "lpa",
            "--seed",
            "7",
            "--resolution",
            "0.5",
            "--json",
        ],
        0,
    );
    let v = json(&out);
    assert_eq!(v["method"], "label_propagation");
    assert_eq!(
        (v["seed"].as_u64(), v["resolution"].as_f64()),
        (Some(7), Some(0.5))
    );
    let m = markers(&out);
    assert!(
        m.len() == 1
            && m[0].starts_with("[communities] method=label_propagation ")
            && m[0].contains(" seed=7 "),
        "{m:?}"
    );

    let v = json(&glia(&[root.path(), "--scope", "pkg_b", "--json"], 0));
    assert_eq!(labels(&v), ["pkg_b::core"], "{v}");
    assert_eq!(
        (v["nodes"].as_u64(), v["isolated"].as_u64()),
        (Some(8), Some(1))
    );

    // clap refuses a method the engine does not run.
    let out = run(&[root.path(), "--method", "louvain"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(markers(&out).is_empty(), "refused before any build");
}

#[test]
fn empty_answer_is_a_report() {
    let root = Root::new("empty");
    let out = glia(&[root.path(), "--scope", "no_such_dir"], 0);
    let text = stdout(&out);
    assert!(blocks(&text).is_empty(), "{text}");
    assert!(text.contains("_(no communities)_"), "{text}");
    assert!(
        text.lines()
            .any(|l| l.starts_with("> FACT: ") && l.contains("no_such_dir")),
        "the absence says why:\n{text}"
    );
    let v = json(&glia(&[root.path(), "--scope", "no_such_dir", "--json"], 0));
    assert_eq!(v["absence"]["reason"], "no_edges", "{v}");
    assert_eq!(v["communities"], serde_json::json!([]));
}

#[test]
fn resolution_must_be_above_zero() {
    let root = Root::new("resolution");
    for bad in ["0", "-1", "NaN"] {
        let out = glia(&[root.path(), &format!("--resolution={bad}")], 2);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("error: --resolution must be > 0"),
            "{bad}: {stderr}"
        );
        assert!(markers(&out).is_empty(), "refused before any build");
        assert!(stdout(&out).is_empty());
    }
    let out = glia(&[root.path(), "--resolution", "inf"], 2);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("error: --resolution must be a finite number"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn build_error_exits_2() {
    let missing = Path::new(&std::env::temp_dir())
        .join(format!("glia-cd1e-cli-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let out = glia(&[missing.to_str().expect("utf-8 temp path")], 2);
    assert!(markers(&out).is_empty());
}

#[test]
fn thread_count_invariant() {
    let root = Root::new("threads");
    let one = run(&[root.path(), "--json"], Some("1"));
    let pool = run(&[root.path(), "--json"], None);
    for (name, out) in [("GLIA_THREADS=1", &one), ("GLIA_THREADS unset", &pool)] {
        assert_eq!(
            out.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!one.stdout.is_empty());
    assert_eq!(
        stdout(&one),
        stdout(&pool),
        "the pool size never reaches the answer"
    );
}
