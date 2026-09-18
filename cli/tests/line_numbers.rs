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
    let main_row = v
        .as_array()
        .expect("a JSON array")
        .iter()
        .find(|r| r["qname"] == "app::main")
        .unwrap_or_else(|| panic!("app::main is in helper's blast radius: {v}"));
    assert_eq!(main_row["file"], "app.py", "{v}");
    assert_eq!(main_row["line"], 8, "1-based `def main` line: {v}");

    // blast-radius table: the printed location is the same 1-based line.
    let out = glia(&repo.0, &["blast-radius", d, "app::helper"]);
    let table = String::from_utf8_lossy(&out.stdout);
    assert!(table.contains("app.py:8"), "table must print app.py:8:\n{table}");
    assert!(!table.contains("app.py:7"), "no 0-based row may leak:\n{table}");

    // trace --json: the hop into `app::helper` lands on line 4.
    let v = json(&glia(&repo.0, &["trace", d, "app::main", "--json"]));
    let hop = v
        .as_array()
        .expect("a JSON array")
        .iter()
        .find(|h| h["to_qname"] == "app::helper")
        .unwrap_or_else(|| panic!("main's trace reaches helper: {v}"));
    assert_eq!(hop["to_file"], "app.py", "{v}");
    assert_eq!(hop["to_line"], 4, "1-based `def helper` line: {v}");

    // resolve --kind diff: every node of the changed file, 1-based; the
    // MODULE row is line 1, never 0.
    let v = json(&glia(&repo.0, &["resolve", d, "app.py", "--kind", "diff", "--json"]));
    let mut got: Vec<(String, i64)> = v
        .as_array()
        .expect("a JSON array")
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
