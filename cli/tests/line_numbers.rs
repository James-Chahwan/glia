//! LD.1 — one line convention: every answer record's `line` is 1-based.
//!
//! POSITION cells store 0-based tree-sitter rows (storage is unchanged); the
//! answer records (`blast-radius`, `trace`, `resolve`, `docs-for`,
//! `contracts`, in table and `--json` form) convert once, in
//! `Locator::locate`, so a reported `file:line` is the line an editor shows.
//! Before LD.1 every record was one line early and a MODULE row said `:0`.
//!
//! Drives the real binary. The `[locate] locator built:` stderr line is the
//! LD.1 fired_on marker; asserting it here makes it a tested contract.
//!
//! LD.8a rides on the same fixture: `resolve`, `docs-for` and `find` answer
//! `{results, absence}` in `--json`, an empty table is followed by the
//! `> FACT:` absence block, and `docs-for` on an unknown qname is an absence
//! (exit 0) instead of exit 3. The `[absence] primitive=` stderr line is the
//! LD.8a fired_on marker.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `def helper` is on 1-based line 4, `def main` on line 8, the module on 1.
const APP_PY: &str =
    "import os\n\n\ndef helper(x):\n    return x + 1\n\n\ndef main():\n    return helper(2)\n";

/// A fresh repo dir holding only `app.py`, removed on drop.
struct TempRepo(PathBuf);

impl TempRepo {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-ld1-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("app.py"), APP_PY).expect("write app.py");
        TempRepo(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("temp path is UTF-8")
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `glia <args>` with persistence off; assert success and exactly one
/// `[locate] locator built:` line (the marker is once per process).
fn glia(repo: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .env("GLIA_NO_PERSIST", "1")
        .args(args)
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "glia {args:?} on {} exited {:?}\nstderr:\n{stderr}",
        repo.display(),
        out.status
    );
    let markers: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[locate] locator built:"))
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "glia {args:?}: the locator marker must fire exactly once per process:\n{stderr}"
    );
    // Relay it so `-- --nocapture 2>&1 | grep '\[locate\] locator built:'` sees it.
    eprintln!("{}", markers[0]);
    out
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("stdout is JSON")
}

#[test]
fn answers_report_1_based_lines() {
    let repo = TempRepo::new("answers");
    let d = repo.path();

    // blast-radius --json: the caller `app::main` sits on line 8.
    let v = json(&glia(&repo.0, &["blast-radius", d, "app::helper", "--json"]));
    let main_row = v["results"]
        .as_array()
        .expect("a `results` array (LD.5: {seeds, unresolved, results, absence})")
        .iter()
        .find(|r| r["qname"] == "app::main")
        .unwrap_or_else(|| panic!("app::main is in helper's blast radius: {v}"));
    assert_eq!(main_row["file"], "app.py", "{v}");
    assert_eq!(main_row["line"], 8, "1-based `def main` line: {v}");
    assert_eq!(v["seeds"][0]["line"], 4, "the seed is located too, 1-based: {v}");

    // blast-radius table: the printed location is the same 1-based line.
    let out = glia(&repo.0, &["blast-radius", d, "app::helper"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("app.py:8"), "table must print app.py:8:\n{table}");
    assert!(!table.contains("app.py:7"), "no 0-based row may leak:\n{table}");

    // trace --json: the hop into `app::helper` lands on line 4.
    let v = json(&glia(&repo.0, &["trace", d, "app::main", "--json"]));
    let hop = v["hops"]
        .as_array()
        .expect("a `hops` array")
        .iter()
        .find(|h| h["to_qname"] == "app::helper")
        .unwrap_or_else(|| panic!("main's trace reaches helper: {v}"));
    assert_eq!(hop["to_file"], "app.py", "{v}");
    assert_eq!(hop["to_line"], 4, "1-based `def helper` line: {v}");

    // resolve --kind diff: every node of the changed file, 1-based; the
    // MODULE row is line 1, never 0.
    let v = json(&glia(&repo.0, &["resolve", d, "app.py", "--kind", "diff", "--json"]));
    assert!(v["absence"].is_null(), "a found answer carries no absence: {v}");
    let mut got: Vec<(String, i64)> = v["results"]
        .as_array()
        .expect("a `results` array")
        .iter()
        .map(|r| {
            (
                r["qname"].as_str().unwrap_or_default().to_string(),
                r["line"].as_i64().unwrap_or(-1),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("app".to_string(), 1),
            ("app::helper".to_string(), 4),
            ("app::main".to_string(), 8),
        ],
        "{v}"
    );
}

/// The LD.8a marker lines in `out`'s stderr, relayed so
/// `-- --nocapture 2>&1 | grep '\[absence\] primitive='` sees them.
fn absence_markers(out: &Output) -> Vec<String> {
    let lines: Vec<String> = String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[absence] primitive="))
        .map(String::from)
        .collect();
    for l in &lines {
        eprintln!("{l}");
    }
    lines
}

#[test]
fn empty_answers_carry_their_absence() {
    let repo = TempRepo::new("absence");
    let d = repo.path();

    // docs-for on an unknown qname: exit 0 and an absence (before LD.8a:
    // exit 3, `error: no node with qname/name `nope``).
    let out = glia(&repo.0, &["docs-for", d, "nope", "--json"]);
    let v = json(&out);
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
    assert_eq!(v["absence"]["tier"], "FACT", "{v}");
    assert_eq!(v["absence"]["reason"], "unknown_symbol", "{v}");
    assert_eq!(v["absence"]["mechanisms"], serde_json::json!(["DOCUMENTS"]), "{v}");
    assert_eq!(v["absence"]["unparsed_files"], 0, "{v}");
    assert_eq!(
        absence_markers(&out),
        [
            "[absence] primitive=governing_docs reason=unknown_symbol mechanisms=DOCUMENTS caveats=2 suggestions=0"
        ]
    );

    // A known symbol nothing documents: the table's empty line, then the FACT
    // and the DOCUMENTS caveat row.
    let out = glia(&repo.0, &["docs-for", d, "app::helper"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "_(no governing docs)_\n> FACT: no DOCUMENTS edge reaches `app::helper` in this graph\n> caveat (*, DOCUMENTS): "
        ),
        "{text}"
    );
    assert_eq!(
        absence_markers(&out),
        [
            "[absence] primitive=governing_docs reason=no_edges mechanisms=DOCUMENTS caveats=2 suggestions=0"
        ]
    );

    // A near miss is suggested.
    let out = glia(&repo.0, &["docs-for", d, "helpr"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("> did you mean: app::helper"), "{text}");

    // resolve and find: the same envelope, their own reasons.
    let v = json(&glia(&repo.0, &["resolve", d, "zzz.py", "--kind", "diff", "--json"]));
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
    assert_eq!(v["absence"]["reason"], "no_signal_match", "{v}");
    let out = glia(&repo.0, &["resolve", d, "app.py", "--kind", "diff", "--scope", "nowhere"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("_(nothing resolved)_\n> FACT: 3 results outside scope `nowhere`"),
        "{text}"
    );
    let v = json(&glia(&repo.0, &["find", d, "qqqq", "--json"]));
    assert_eq!(v["results"], serde_json::json!([]), "{v}");
    assert_eq!(v["absence"]["reason"], "no_match", "{v}");
}

/// LD.6: `resolve` rows carry a bool `live` and `trace` hops a bool
/// `to_live`, read off the same walk as `blast-radius`; the resolve table
/// renders it as a `live` column (● / ⊘). `main` is an entrypoint by name, so
/// it and the `helper` it calls are live; no carry edge reaches the MODULE.
/// The `[live] annotate` stderr line is the LD.6 fired_on marker.
#[test]
fn resolve_and_trace_rows_carry_live() {
    let repo = TempRepo::new("live");
    let d = repo.path();

    let out = glia(&repo.0, &["resolve", d, "app.py", "--kind", "diff", "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let marker = stderr
        .lines()
        .find(|l| l.starts_with("[live] annotate surface=resolve"))
        .unwrap_or_else(|| panic!("the resolve marker fires:\n{stderr}"));
    eprintln!("{marker}");
    assert_eq!(marker, "[live] annotate surface=resolve rows=3 live=2 entry_kinds=11");
    let v = json(&out);
    let mut got: Vec<(String, bool)> = v["results"]
        .as_array()
        .expect("a `results` array")
        .iter()
        .map(|r| {
            let live = r["live"].as_bool().unwrap_or_else(|| panic!("row carries a bool `live`: {r}"));
            (r["qname"].as_str().unwrap_or_default().to_string(), live)
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("app".to_string(), false),
            ("app::helper".to_string(), true),
            ("app::main".to_string(), true),
        ],
        "{v}"
    );

    let v = json(&glia(&repo.0, &["trace", d, "app::main", "--json"]));
    let hops = v["hops"].as_array().expect("a `hops` array");
    assert!(!hops.is_empty(), "{v}");
    for h in hops {
        assert!(h["to_live"].is_boolean(), "hop carries a bool `to_live`: {h}");
    }
    let helper = hops.iter().find(|h| h["to_qname"] == "app::helper").expect("main -> helper hop");
    assert_eq!(helper["to_live"], true, "{v}");

    let out = glia(&repo.0, &["resolve", d, "app.py", "--kind", "diff"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("| score | live | kind | qname | location |"), "{table}");
    assert!(table.contains("| ● | FUNCTION | `app::main` | app.py:8 |"), "{table}");
    assert!(table.contains("| ⊘ | MODULE | `app` | app.py:1 |"), "{table}");

    let out = glia(&repo.0, &["trace", d, "app::main"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("| depth | mechanism | xsvc | from | → to | live | location |"), "{table}");
    assert!(table.contains("| `app::main` | `app::helper` (FUNCTION) | ● | app.py:4 |"), "{table}");
}
