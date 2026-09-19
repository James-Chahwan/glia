//! LF.1c — `glia cell set / rm / ls`, the CLI surface of the cell write API
//! (`glia_store::write_cell` / `remove_cell_entry`), and `ls --check`,
//! which binds every sidecar row through the build's one resolver
//! (`glia_graph::cells::QnameIndex`) against a fresh build.
//!
//! Each test drives the real binary over a scratch repo. The fired_on markers
//! are grep-able:
//! `cargo test -p glia-cli --test cell_cli -- --nocapture 2>&1 | grep -o '\[cells\] .*surface=cli.*'`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CHARGE: &str = "def charge(order_id):\n    total = order_id * 2\n    return total + len(str(order_id))\n";

/// A scratch repo, created fresh and removed on drop (`cli` has no
/// dev-dependencies, so no `tempfile`).
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glia-lf1c-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("utf-8 scratch path")
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        std::fs::write(p, text).expect("write fixture file");
    }

    /// The `.glia/cells.jsonl` rows.
    fn cell_rows(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.0.join(".glia/cells.jsonl")).unwrap_or_default();
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("a JSON row"))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", self.stdout))
    }
}

fn glia(args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .output()
        .expect("glia runs");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // Relay the markers so `-- --nocapture | grep '\[cells\]'` sees them.
    for line in stderr.lines().filter(|l| l.starts_with("[cells] ") && l.contains("surface=cli")) {
        eprintln!("{line}");
    }
    Run { code: out.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr }
}

fn ok(args: &[&str]) -> Run {
    let r = glia(args);
    assert_eq!(r.code, 0, "glia {args:?} exited {}\nstdout:\n{}\nstderr:\n{}", r.code, r.stdout, r.stderr);
    r
}

fn texts(rows: &[Value]) -> Vec<(String, String, String)> {
    rows.iter()
        .map(|r| {
            let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
            (s(&r["qname"]), s(&r["entry"]["id"]), s(&r["entry"]["text"]))
        })
        .collect()
}

/// The acceptance flow: set, list, append, remove, then a rename orphans the
/// row and `ls --check` says so with exit 1.
#[test]
fn set_ls_rm_then_a_rename_orphans_the_row() {
    let repo = Scratch::new("flow");
    repo.write("a.py", CHARGE);
    let root = repo.path();

    let first = ok(&["cell", "set", root, "a::charge", "CONV", "--text", "retries are safe"]);
    assert_eq!(first.stdout.trim(), "wrote CONV 000001 for a::charge (write-through: no gmap)");
    assert!(
        first.stderr.contains(
            "[cells] write qname=a::charge cell=CONV id=000001 rows=1 target=pending write_through=no_gmap surface=cli"
        ),
        "{}",
        first.stderr
    );
    assert_eq!(
        texts(&repo.cell_rows()),
        vec![("a::charge".into(), "000001".into(), "retries are safe".into())]
    );

    let listed = ok(&["cell", "ls", root, "--json"]).json();
    let rows = listed["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!((rows[0]["qname"].as_str(), rows[0]["cell"].as_str()), (Some("a::charge"), Some("CONV")));
    assert_eq!(rows[0]["entry"]["text"], "retries are safe");
    assert!(rows[0].get("status").is_none(), "plain ls classifies nothing: {listed}");

    let second = ok(&["cell", "set", root, "a::charge", "CONV", "--text", "idempotent by order id"]);
    assert!(second.stdout.starts_with("wrote CONV 000002 for a::charge"), "{}", second.stdout);
    assert_eq!(repo.cell_rows().len(), 2);

    let rm = ok(&["cell", "rm", root, "a::charge", "CONV", "--id", "000001"]);
    assert!(rm.stdout.starts_with("removed CONV 000001 from a::charge"), "{}", rm.stdout);
    assert!(rm.stderr.contains("[cells] remove qname=a::charge cell=CONV id=000001 rows=1 "), "{}", rm.stderr);
    assert_eq!(
        texts(&repo.cell_rows()),
        vec![("a::charge".into(), "000002".into(), "idempotent by order id".into())]
    );

    // Before the rename the row binds and the check passes.
    let bound = ok(&["cell", "ls", root, "--check"]);
    assert!(
        bound.stderr.contains("[cells] ls rows=1 bound=1 rekeyed=0 ambiguous=0 orphaned=0 rejected=0 surface=cli"),
        "{}",
        bound.stderr
    );

    repo.write("a.py", &CHARGE.replace("def charge(", "def charge2("));
    let orphaned = glia(&["cell", "ls", root, "--check"]);
    assert_eq!(orphaned.code, 1, "an orphaned row fails the check:\n{}\n{}", orphaned.stdout, orphaned.stderr);
    assert!(orphaned.stdout.contains("orphaned a::charge CONV api/000002"), "{}", orphaned.stdout);
    assert!(
        orphaned.stderr.contains("[cells] ls rows=1 bound=0 rekeyed=0 ambiguous=0 orphaned=1 rejected=0 surface=cli"),
        "{}",
        orphaned.stderr
    );
    let json = glia(&["cell", "ls", root, "--check", "--json"]);
    assert_eq!(json.code, 1);
    let v = json.json();
    assert_eq!(v["rows"][0]["status"], "orphaned", "{v}");
    assert_eq!(v["check"]["orphaned"], 1, "{v}");
}

/// A row written while the default layout is fresh carries the node's
/// move-stable hint; after the file moves, `ls --check` reports the row
/// rekeyed with its tier (exit 0: the build still applies it), and
/// `--rekey` rewrites it to the node's current qname.
#[test]
fn check_reports_a_moved_node_rekeyed_and_rekey_rewrites_it() {
    let repo = Scratch::new("rekey");
    repo.write("a.py", CHARGE);
    let root = repo.path();
    ok(&["build", root]);
    let set = ok(&["cell", "set", root, "a::charge", "DECISION", "--json", r#"{"id":"d1","title":"charge is idempotent"}"#]);
    assert!(
        set.stderr.contains("[cells] write qname=a::charge cell=DECISION id=d1 rows=1 target=bound write_through=applied surface=cli"),
        "{}",
        set.stderr
    );
    let hint = repo.cell_rows()[0]["hint"].as_str().unwrap_or_default().to_string();
    assert!(hint.starts_with("v1|"), "the row carries the node's hint: {:?}", repo.cell_rows());

    std::fs::create_dir_all(repo.0.join("pkg")).expect("mkdir pkg");
    std::fs::rename(repo.0.join("a.py"), repo.0.join("pkg/a.py")).expect("move a.py");

    let check = ok(&["cell", "ls", root, "--check"]);
    assert!(
        check.stdout.contains("rekeyed a::charge DECISION api/d1 -> pkg::a::charge tier=same-name"),
        "{}",
        check.stdout
    );
    assert!(check.stderr.contains("[cells] ls rows=1 bound=0 rekeyed=1 "), "{}", check.stderr);
    assert_eq!(repo.cell_rows()[0]["qname"], "a::charge", "--check alone never rewrites");

    let rekey = ok(&["cell", "ls", root, "--check", "--rekey"]);
    assert!(
        rekey.stderr.contains("[cells] rekey qname=a::charge -> pkg::a::charge cell=DECISION id=d1 tier=same-name surface=cli"),
        "{}",
        rekey.stderr
    );
    let rows = repo.cell_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["qname"], "pkg::a::charge");
    assert_eq!(rows[0]["entry"]["title"], "charge is idempotent");
    // The hint is re-captured from the node where it now sits; it keys on the
    // file's basename, which the move kept.
    assert_eq!(rows[0]["hint"].as_str(), Some(hint.as_str()), "{rows:?}");

    let after = ok(&["cell", "ls", root, "--check", "--json"]).json();
    assert_eq!(after["rows"][0]["status"], "bound", "{after}");
    assert_eq!(after["check"]["rekeyed"], 0, "{after}");
}

/// `--rekey` never overwrites a different entry already at the target key: a
/// CONV takes the next free id there, a DECISION is left in place and counted
/// as a conflict.
#[test]
fn rekey_never_overwrites_an_entry_at_the_target() {
    let repo = Scratch::new("conflict");
    repo.write("a.py", CHARGE);
    let root = repo.path();
    ok(&["build", root]);
    ok(&["cell", "set", root, "a::charge", "CONV", "--text", "old note"]);
    ok(&["cell", "set", root, "a::charge", "DECISION", "--json", r#"{"id":"d1","title":"old"}"#]);
    ok(&["cell", "set", root, "pkg::a::charge", "CONV", "--text", "new note"]);
    ok(&["cell", "set", root, "pkg::a::charge", "DECISION", "--json", r#"{"id":"d1","title":"new"}"#]);
    std::fs::create_dir_all(repo.0.join("pkg")).expect("mkdir pkg");
    std::fs::rename(repo.0.join("a.py"), repo.0.join("pkg/a.py")).expect("move a.py");

    let r = ok(&["cell", "ls", root, "--check", "--rekey", "--json"]);
    assert!(
        r.stderr.contains("[cells] rekey qname=a::charge -> pkg::a::charge cell=CONV id=000001 tier=same-name surface=cli new_id=000002"),
        "{}",
        r.stderr
    );
    assert!(r.stderr.contains("[cells] rekey conflict qname=a::charge -> pkg::a::charge cell=DECISION id=d1"), "{}", r.stderr);
    assert_eq!(r.json()["rekey"], serde_json::json!({"rewritten": 1, "conflicts": 1}));

    let rows: Vec<(String, String, String)> = repo
        .cell_rows()
        .iter()
        .map(|r| {
            let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
            (s(&r["qname"]), s(&r["entry"]["id"]), format!("{}{}", s(&r["entry"]["text"]), s(&r["entry"]["title"])))
        })
        .collect();
    let want = [
        ("a::charge", "d1", "old"),
        ("pkg::a::charge", "000001", "new note"),
        ("pkg::a::charge", "000002", "old note"),
        ("pkg::a::charge", "d1", "new"),
    ];
    assert_eq!(rows, want.map(|(q, i, t)| (q.to_string(), i.to_string(), t.to_string())));
}

/// A qname two nodes share (a CLASS and a FUNCTION) is ambiguous: `--check`
/// lists every candidate and picks none; `--kind` binds the one meant.
#[test]
fn check_lists_ambiguous_candidates() {
    let repo = Scratch::new("ambiguous");
    repo.write("a.py", "class Foo:\n    pass\n\n\ndef Foo():\n    return 1\n");
    let root = repo.path();
    ok(&["cell", "set", root, "a::Foo", "CONV", "--text", "which one?"]);
    ok(&["cell", "set", root, "a::Foo", "CONV", "--text", "the function", "--kind", "FUNCTION"]);
    let r = glia(&["cell", "ls", root, "--check", "--json"]);
    assert_eq!(r.code, 1, "an ambiguous row is not applied:\n{}", r.stderr);
    let v = r.json();
    let rows = v["rows"].as_array().expect("rows");
    let ambiguous: Vec<&Value> = rows.iter().filter(|r| r["status"] == "ambiguous").collect();
    assert_eq!(ambiguous.len(), 1, "{v}");
    let mut kinds: Vec<&str> =
        ambiguous[0]["candidates"].as_array().expect("candidates").iter().filter_map(|c| c["kind"].as_str()).collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["CLASS", "FUNCTION"], "{v}");
    assert!(rows.iter().any(|r| r["status"] == "bound" && r["kind"] == "FUNCTION"), "{v}");
    assert!(r.stderr.contains("[cells] ls rows=2 bound=1 rekeyed=0 ambiguous=1 orphaned=0 rejected=0 surface=cli"), "{}", r.stderr);
}

/// A VECTOR goes through `--file` (raw bytes), lists as `<n> bytes`, and is
/// removed without an id.
#[test]
fn vector_set_ls_rm() {
    let repo = Scratch::new("vector");
    repo.write("a.py", CHARGE);
    let bytes: Vec<u8> = [1.0f32, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let vec_path = repo.0.join("emb.bin");
    std::fs::write(&vec_path, &bytes).expect("write vector");
    let root = repo.path();
    let file = vec_path.to_str().expect("utf-8");

    let set = ok(&["cell", "set", root, "a::charge", "VECTOR", "--file", file, "--model", "m1", "--dims", "2"]);
    assert_eq!(set.stdout.trim(), "wrote VECTOR - for a::charge (write-through: no gmap)");
    let ls = ok(&["cell", "ls", root]);
    assert!(ls.stdout.contains("a::charge VECTOR 8 bytes model=m1 dims=2"), "{}", ls.stdout);
    let json = ok(&["cell", "ls", root, "--json", "--qname", "a::charge"]).json();
    assert_eq!(json["rows"][0]["bytes"], 8, "{json}");
    assert!(ok(&["cell", "ls", root, "--json", "--qname", "nope"]).json()["rows"].as_array().is_some_and(Vec::is_empty));

    ok(&["cell", "rm", root, "a::charge", "VECTOR"]);
    assert!(ok(&["cell", "ls", root, "--json"]).json()["rows"].as_array().is_some_and(Vec::is_empty));
}

/// Usage errors exit 2 and write nothing; removing a row that is not there
/// exits 1.
#[test]
fn usage_errors_exit_2_and_a_missing_row_exits_1() {
    let repo = Scratch::new("usage");
    repo.write("a.py", CHARGE);
    let root = repo.path();
    let two = |args: &[&str]| {
        let r = glia(args);
        assert_eq!(r.code, 2, "glia {args:?}:\n{}\n{}", r.stdout, r.stderr);
        r
    };
    two(&["cell", "set", root, "a::charge", "CONV"]);
    two(&["cell", "set", root, "a::charge", "CONV", "--text", "x", "--json", "{}"]);
    two(&["cell", "ls", root, "--rekey"]);
    assert!(two(&["cell", "set", root, "a::charge", "CODE", "--text", "x"]).stderr.contains("not writable"));
    assert!(two(&["cell", "set", root, "a::charge", "CONV", "--json", "not json"]).stderr.contains("not JSON"));
    assert!(two(&["cell", "set", root, "a::charge", "CONV", "--text", "x", "--dims", "2"]).stderr.contains("VECTOR"));
    assert!(two(&["cell", "rm", root, "a::charge", "CONV"]).stderr.contains("--id"));
    let missing = repo.0.join("no-such-dir");
    two(&["cell", "ls", missing.to_str().expect("utf-8")]);
    assert!(!missing.exists(), "a missing repo is never created");
    assert!(repo.cell_rows().is_empty(), "no usage error writes a row");

    let gone = glia(&["cell", "rm", root, "a::charge", "CONV", "--id", "000009"]);
    assert_eq!(gone.code, 1, "{}", gone.stderr);
    assert!(gone.stdout.contains("no CONV api/000009 on a::charge"), "{}", gone.stdout);
    assert!(!Path::new(root).join(".glia/cells.jsonl").exists());
}
