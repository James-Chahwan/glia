//! LF.5c — the co-change audit in `glia coverage` (and the `cochange_no_edge`
//! category of `glia gaps`), driving the real binary over probe g1: the
//! committed substrate-gap fixture `history-cochange` (svc/a.py and svc/b.py
//! co-changed 4 times, no edge of any kind between them), copied with its
//! hand-written `.glia/history-snapshot/` into a temp root.
//!
//! The `[cochange] pairs=P linked=L gaps=G (direct=D bridged=B) surface=...`
//! stderr line is the fired_on marker; asserting it here makes it a tested
//! contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use glia_code_domain::snapshots::{HistoryCommit, HistoryFile, HistoryMeta, write_history};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/history-cochange";
const SNAPSHOT: &str = ".glia/history-snapshot";

const CLIENT_TS: &str = "export async function loadUsers() {\n  return fetch('/users');\n}\n";
const API_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\", methods=[\"GET\"])\ndef list_users():\n    return []\n";

fn glia(args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("glia runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "glia {args:?}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Relay the markers so `-- --nocapture | grep '^\[cochange\] '` sees them.
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        if line.starts_with("[cochange] ") {
            eprintln!("{line}");
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

/// A fresh, empty temp root for one test, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-lf5c-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        Root(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Probe g1 under a temp root: the fixture's sources, and its snapshot when
/// `with_snapshot`.
fn g1(tag: &str, with_snapshot: bool) -> Root {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let d = Root::new(tag);
    let mut dirs = vec!["svc"];
    if with_snapshot {
        dirs.push(SNAPSHOT);
    }
    for dir in dirs {
        std::fs::create_dir_all(d.path().join(dir)).expect("mkdir");
        for entry in std::fs::read_dir(src.join(dir)).expect("fixture dir") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), d.path().join(dir).join(entry.file_name())).expect("copy");
        }
    }
    d
}

#[test]
fn coverage_table_lists_the_unlinked_pair() {
    let d = g1("table", true);
    let out = glia(&["coverage", s(d.path())]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(
        stdout.contains(
            "## co-change without a static edge (heuristic: co-change is history, not proof of coupling)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("| svc/a.py | svc/b.py | 4 | 1000 | python |"),
        "{stdout}"
    );
    assert!(
        stderr.contains("[cochange] pairs=1 linked=0 gaps=1 (direct=0 bridged=0) surface=coverage"),
        "{stderr}"
    );
}

#[test]
fn coverage_json_is_the_unchanged_caveat_array() {
    let d = g1("json", true);
    let out = glia(&["coverage", s(d.path()), "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let rows = v.as_array().expect("a JSON array");
    assert!(!rows.is_empty());
    for r in rows {
        let mut keys: Vec<&str> = r
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["edge_category", "edges_found", "language", "note", "verify"]
        );
    }
    assert!(
        !text(&out.stderr).contains("[cochange]"),
        "no audit in --json"
    );
}

#[test]
fn no_snapshot_no_section() {
    let d = g1("bare", false);
    let out = glia(&["coverage", s(d.path())]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(stdout.contains("| language | edge | found |"), "{stdout}");
    assert!(!stdout.contains("## co-change"), "{stdout}");
    assert!(!stderr.contains("[cochange]"), "{stderr}");
}

#[test]
fn gaps_lists_the_pair_under_its_category() {
    let d = g1("gaps", true);
    let out = glia(&["gaps", s(d.path()), "--category", "cochange_no_edge"]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(stdout.contains("## cochange_no_edge — 1"), "{stdout}");
    assert!(
        stdout.contains(
            "| `svc::a` | MODULE | svc/a.py:1 | heuristic | edge | with=svc/b.py (svc::b); cochanges=4; ratio_permille=1000; languages=python,python |"
        ),
        "{stdout}"
    );
    assert!(
        stderr.contains("[cochange] pairs=1 linked=0 gaps=1 (direct=0 bridged=0) surface=gaps"),
        "{stderr}"
    );
}

/// A client and the API it calls co-change: linked through ENDPOINT -> ROUTE
/// (two file-less nodes), so the section says so and lists no row.
#[test]
fn http_pair_is_bridged() {
    let d = Root::new("http");
    for (p, body) in [("web/client.ts", CLIENT_TS), ("api/app.py", API_PY)] {
        let path = d.path().join(p);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }
    let commits: Vec<HistoryCommit> = (1..=3)
        .rev()
        .map(|i: i64| HistoryCommit {
            c: format!("{i:02}{}", "0".repeat(38)),
            t: 1_767_225_600 + i * 86_400,
            files: ["api/app.py", "web/client.ts"]
                .iter()
                .map(|p| HistoryFile {
                    p: (*p).to_string(),
                    a: Some(1),
                    d: Some(0),
                    from: None,
                })
                .collect(),
        })
        .collect();
    let head = commits[0].c.clone();
    write_history(
        d.path(),
        HistoryMeta::new(head, 2000, None, String::new()),
        &commits,
        &[],
    )
    .expect("write snapshot");
    let out = glia(&["coverage", s(d.path())]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(
        stdout.contains("## co-change without a static edge"),
        "{stdout}"
    );
    assert!(
        stdout.contains("none: every co-changing pair shares a static link."),
        "{stdout}"
    );
    assert!(
        stderr.contains("[cochange] pairs=1 linked=1 gaps=0 (direct=0 bridged=1) surface=coverage"),
        "{stderr}"
    );
}
