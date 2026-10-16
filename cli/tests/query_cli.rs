//! DB.5 — `glia query`, driving the real binary (`GLIA_NO_PERSIST=1`) over
//! DB.2a's fx1, inlined without its overlay `[[edge]]`: three manifest projects
//! (web/, shared/, services/api/); web/app.py `page()` calls shared.util.helper
//! and `other()` calls services.api.public.facade.get; shared/util.py
//! `helper()` calls services.api.internal.db.query; facade `get()` calls
//! db.write. So web's FUNCTION -> FUNCTION calls are exactly two rows, both
//! FACT (the engine tests' fx1 adds a heuristic overlay CALLS, a third).
//!
//! The engine's `[query] lang=<..> outputs=<o> rows=<r> ... surface=cli` line
//! and the CLI's `[query] surface=cli exit=<0|1|2>` are the fired_on markers;
//! asserting them here makes them a tested contract.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const WEB_APP: &str = "from shared.util import helper\n\
from services.api.public.facade import get\n\
\n\
\n\
def page():\n    return helper()\n\
\n\
\n\
def other():\n    return get()\n";
const SHARED_UTIL: &str =
    "from services.api.internal.db import query\n\n\ndef helper():\n    return query()\n";
const FACADE: &str =
    "from services.api.internal.db import write\n\n\ndef get():\n    return write()\n";
const DB: &str = "def query():\n    return 1\n\n\ndef write():\n    return 2\n";

const FX1: [(&str, &str); 7] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("shared/pyproject.toml", "[project]\nname = \"shared\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
    ("web/app.py", WEB_APP),
    ("shared/util.py", SHARED_UTIL),
    ("services/api/public/facade.py", FACADE),
    ("services/api/internal/db.py", DB),
];

/// Web's calls, both ends rendered: the acceptance program.
const WEB_CALLS: &str = "x(a, b) :- calls(a, b), in_scope(a, \"web\").";

/// The same question in GQL.
const WEB_CALLS_GQL: &str = "MATCH (a:FUNCTION)-[:CALLS]->(b) WHERE a.qname STARTS WITH 'web::' \
                             RETURN a.qname, b.qname ORDER BY a.qname";

/// A fresh temp root, removed on drop; repos go in subdirectories, programs
/// beside them (never inside a repo the build walks).
struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("glia-db5-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir root");
        Root(p)
    }

    /// Write `files` under `<root>/<dir>`; the repo's path.
    fn repo(&self, dir: &str, files: &[(&str, &str)]) -> String {
        let base = self.0.join(dir);
        for (rel, src) in files {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, src).expect("write source");
        }
        utf8(&base)
    }

    /// Write `text` at `<root>/<name>`; its path.
    fn file(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("write program");
        utf8(&path)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn utf8(p: &Path) -> String {
    p.to_str().expect("utf-8 temp path").to_string()
}

/// `glia query <args>` with `stdin` piped in (`None`: stdin is null).
fn run(args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    cmd.arg("query")
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = cmd.spawn().expect("glia runs");
    if let Some(text) = stdin {
        let mut pipe = child.stdin.take().expect("a stdin pipe");
        pipe.write_all(text.as_bytes()).expect("write stdin");
    }
    child.wait_with_output().expect("glia exits")
}

/// Run `glia query <args>`, relay its markers and check the exit code.
fn glia(args: &[&str], stdin: Option<&str>, want: i32) -> Output {
    let out = run(args, stdin);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(want),
        "glia query {args:?}\nstdout:\n{}\nstderr:\n{stderr}",
        stdout(&out)
    );
    // Relay the markers so `-- --nocapture | grep '^\[query\] '` sees them.
    for line in markers(&out) {
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

fn markers(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with("[query] "))
        .map(str::to_string)
        .collect()
}

/// The CLI's exit marker is the last `[query]` line.
fn exit_marker(out: &Output) -> String {
    markers(out).last().cloned().unwrap_or_default()
}

#[test]
fn datalog_expr_rows_exit_0() {
    let root = Root::new("expr");
    let fx = root.repo("fx1", &FX1);
    let out = glia(&[&fx, "-e", WEB_CALLS], None, 0);
    let s = stdout(&out);
    assert!(
        s.starts_with(&format!("# glia query `{fx}` (datalog)\n")),
        "{s}"
    );
    assert!(s.contains("## x/2 - 2 row(s)"), "{s}");
    assert!(s.contains("| # | a | b | tier | at |"), "{s}");
    assert!(
        s.contains(
            "| 1 | `web::app::other` | `services::api::public::facade::get` | fact | web/app.py:9 |"
        ),
        "{s}"
    );
    assert!(
        s.contains("| 2 | `web::app::page` | `shared::util::helper` | fact | web/app.py:5 |"),
        "{s}"
    );
    assert!(!s.contains("showing"), "nothing is cut: {s}");
    let m = markers(&out);
    assert!(
        m.iter()
            .any(|l| l.starts_with("[query] lang=datalog outputs=1 rows=2 ")
                && l.ends_with(" surface=cli")),
        "{m:?}"
    );
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=0");
}

#[test]
fn file_and_stdin_agree() {
    let root = Root::new("stdin");
    let fx = root.repo("fx1", &FX1);
    let prog = root.file("web_calls.dl", WEB_CALLS);
    let from_file = glia(&[&fx, &prog], None, 0);
    let piped = glia(&[&fx], Some(WEB_CALLS), 0);
    let dash = glia(&[&fx, "-"], Some(WEB_CALLS), 0);
    let expr = glia(&[&fx, "-e", WEB_CALLS], None, 0);
    assert_eq!(stdout(&from_file), stdout(&piped));
    assert_eq!(stdout(&from_file), stdout(&dash));
    assert_eq!(stdout(&from_file), stdout(&expr));
    assert!(stdout(&piped).contains("## x/2 - 2 row(s)"));

    // `--limit` cuts the rows shown after the sort; the total stays.
    let cut = glia(&[&fx, &prog, "--limit", "1"], None, 0);
    let s = stdout(&cut);
    assert!(s.contains("## x/2 - 2 row(s)"), "{s}");
    assert!(s.contains("| 1 | `web::app::other` |"), "{s}");
    assert!(!s.contains("| 2 |"), "{s}");
    assert!(
        s.contains("_(showing 1 of 2; --limit <n> shows more)_"),
        "{s}"
    );

    // --explain follows each row with its witness, check's evidence layout.
    let explained = glia(&[&fx, &prog, "--explain"], None, 0);
    let s = stdout(&explained);
    assert_eq!(s.matches("_witness: 1 edge(s)_").count(), 2, "{s}");
    assert!(
        s.contains("| # | category | from | to | at | emitter | tier |"),
        "{s}"
    );
    assert!(
        s.contains(
            "| 1 | CALLS | `web::app::other` | `services::api::public::facade::get` | web/app.py:10 |"
        ),
        "{s}"
    );
    assert!(
        s.contains("| 1 | CALLS | `web::app::page` | `shared::util::helper` | web/app.py:6 |"),
        "{s}"
    );
}

#[test]
fn gql_mode() {
    let root = Root::new("gql");
    let fx = root.repo("fx1", &FX1);
    let out = glia(&[&fx, "--gql", "-e", WEB_CALLS_GQL], None, 0);
    let s = stdout(&out);
    assert!(
        s.starts_with(&format!("# glia query `{fx}` (gql)\n")),
        "{s}"
    );
    assert!(s.contains("## result - 2 row(s)"), "{s}");
    assert!(s.contains("| # | a.qname | b.qname | tier | at |"), "{s}");
    // Attribute values are strings, printed as is; the row is located at `a`, the
    // first property's node, as the Datalog `x(a, b)` rows are (DT.8b).
    assert!(
        s.contains(
            "| 1 | web::app::other | services::api::public::facade::get | fact | web/app.py:9 |"
        ),
        "{s}"
    );
    assert!(
        s.contains("| 2 | web::app::page | shared::util::helper | fact | web/app.py:5 |"),
        "{s}"
    );
    assert!(
        markers(&out)
            .iter()
            .any(|l| l.starts_with("[query] lang=gql outputs=1 rows=2 ")),
        "{:?}",
        markers(&out)
    );
    // The anchor's fired_on marker: two evaluated rows, none folded.
    assert!(
        stderr(&out)
            .lines()
            .any(|l| l == "[gql] anchor column=a rows=2 merged=0"),
        "{}",
        stderr(&out)
    );
    // An aggregate row is a group: still unlocated, and no anchor runs.
    let agg = glia(
        &[
            &fx,
            "--gql",
            "-e",
            "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname, count(b) AS n",
        ],
        None,
        0,
    );
    let s = stdout(&agg);
    assert!(
        s.contains("| 1 | services::api::public::facade::get | 1 | fact | — |"),
        "{s}"
    );
    assert!(!stderr(&agg).contains("[gql] anchor"), "{}", stderr(&agg));
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=0");

    // A GQL query from a file names the file in its diagnostics.
    let bad = root.file("bad.gql", "MATCH (a:FUNCTIONN) RETURN a");
    let e = glia(&[&fx, "--gql", &bad], None, 2);
    assert!(stderr(&e).contains(&format!("{bad}:1:")), "{}", stderr(&e));
}

#[test]
fn empty_is_exit_1_with_the_absence() {
    let root = Root::new("empty");
    let fx = root.repo("fx1", &FX1);
    let out = glia(&[&fx, "-e", "x(a) :- node(a, \"ROUTE\")."], None, 1);
    let s = stdout(&out);
    assert!(s.contains("## x/1 - 0 row(s)"), "{s}");
    assert!(s.contains("> FACT: no row of x/1 in this graph"), "{s}");
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=1");

    // --json: the same exit code, the absence in the answer.
    let json = glia(
        &[&fx, "-e", "x(a) :- node(a, \"ROUTE\").", "--json"],
        None,
        1,
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("JSON");
    assert_eq!(v["absence"]["reason"], "no_match");
    assert_eq!(v["absence"]["note"], "no row of x/1 in this graph");
    assert_eq!(v["results"], serde_json::json!([]));
}

#[test]
fn errors_exit_2() {
    let root = Root::new("errors");
    let fx = root.repo("fx1", &FX1);

    // An analysis error, rendered `<source>:<line>:<col>: error: <message>`.
    let out = glia(&[&fx, "-e", "x(a) :- node(a, \"ROUTEE\")."], None, 2);
    let e = stderr(&out);
    assert!(e.contains("<-e>:1:"), "{e}");
    assert!(e.contains("unknown node kind `ROUTEE`"), "{e}");
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=2");

    // From stdin the source is `<stdin>`; from a file, its path.
    let out = glia(&[&fx], Some("x(a) :- node(a, \"ROUTEE\")."), 2);
    assert!(stderr(&out).contains("<stdin>:1:"), "{}", stderr(&out));
    let prog = root.file("bad.dl", "x(a) :- node(a, \"ROUTE\").\ny(a) :- nope(a).");
    let out = glia(&[&fx, &prog], None, 2);
    assert!(
        stderr(&out).contains(&format!("{prog}:2:")),
        "{}",
        stderr(&out)
    );

    // A parse error.
    let out = glia(&[&fx, "-e", "x(a) :- node(a, \"ROUTE\""], None, 2);
    assert!(stderr(&out).contains("<-e>:1:"), "{}", stderr(&out));

    // The row budget stops a closure and names the CLI's way to raise it.
    let closure = "r(a, b) :- edge(a, b, \"CALLS\").\nr(a, c) :- r(a, b), edge(b, c, \"CALLS\").";
    let out = glia(&[&fx, "-e", closure, "--max-rows", "1"], None, 2);
    assert!(stderr(&out).contains("--max-rows"), "{}", stderr(&out));
    // Under the default budget the same closure is answered.
    glia(&[&fx, "-e", closure], None, 0);

    // --output is Datalog's: with --gql it is a usage error.
    let out = glia(
        &[&fx, "--gql", "-e", WEB_CALLS_GQL, "--output", "x"],
        None,
        2,
    );
    assert!(stderr(&out).contains("--output"), "{}", stderr(&out));
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=2");

    // No program: stdin is null (empty), so there is nothing to read.
    let out = glia(&[&fx], None, 2);
    assert!(stderr(&out).contains("give a program"), "{}", stderr(&out));
    // A blank -e or file is the same usage error.
    let out = glia(&[&fx, "-e", "  \n"], None, 2);
    assert!(stderr(&out).contains("give a program"), "{}", stderr(&out));
    let blank = root.file("blank.dl", "\n\n");
    let out = glia(&[&fx, &blank], None, 2);
    assert!(stderr(&out).contains("give a program"), "{}", stderr(&out));
    // A file that is not there.
    let out = glia(&[&fx, &utf8(&root.0.join("missing.dl"))], None, 2);
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
    // -e and a file conflict (clap's usage error).
    let out = run(&[&fx, &prog, "-e", WEB_CALLS], None);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    // A repo that is not there is a build error.
    let out = glia(
        &[&utf8(&root.0.join("no-such-repo")), "-e", WEB_CALLS],
        None,
        2,
    );
    assert_eq!(exit_marker(&out), "[query] surface=cli exit=2");
}

#[test]
fn json_is_the_answer() {
    let root = Root::new("json");
    let fx = root.repo("fx1", &FX1);
    let out = glia(&[&fx, "-e", WEB_CALLS, "--json"], None, 0);
    let s = stdout(&out);
    // The key order is the answer's: language, outputs, results, absence.
    let at = |k: &str| {
        s.find(&format!("\"{k}\":"))
            .unwrap_or_else(|| panic!("no {k}: {s}"))
    };
    assert!(s.starts_with("{\"language\":"), "{s}");
    assert!(at("language") < at("outputs"), "{s}");
    assert!(at("outputs") < at("results"), "{s}");
    assert!(at("results") < at("absence"), "{s}");
    let v: serde_json::Value = serde_json::from_str(&s).expect("JSON");
    let keys: Vec<&String> = v.as_object().expect("an object").keys().collect();
    let mut want = ["absence", "language", "outputs", "results"];
    want.sort_unstable();
    assert_eq!(keys, want, "{s}");
    assert_eq!(v["language"], "datalog");
    assert_eq!(v["outputs"][0]["predicate"], "x");
    assert_eq!(v["outputs"][0]["total"], 2);
    assert!(v["absence"].is_null(), "{s}");
    let first = &v["results"][0]["values"][0];
    assert!(first.get("node").is_some(), "{s}");
    assert_eq!(first["node"]["qname"], "web::app::other");
    assert_eq!(v["results"][0]["tier"], "fact");
}

#[test]
fn with_merges() {
    let root = Root::new("with");
    let front = root.repo(
        "front",
        &[
            ("pyproject.toml", "[project]\nname = \"front\"\n"),
            (
                "client.py",
                "import requests\n\n\ndef load():\n    return requests.get(\"http://api/users\")\n",
            ),
        ],
    );
    let back = root.repo(
        "back",
        &[
            ("pyproject.toml", "[project]\nname = \"back\"\n"),
            (
                "server.py",
                "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get(\"/users\")\ndef users():\n    return []\n",
            ),
        ],
    );
    let program = "x(a, b, c) :- cross_repo(a, b, c).";
    let out = glia(&[&front, "--with", &back, "-e", program, "--json"], None, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("JSON");
    let rows = v["results"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "{}", stdout(&out));
    let r = &rows[0];
    assert_eq!(r["values"][0]["node"]["qname"], "endpoint:GET:/users");
    assert_eq!(r["values"][1]["node"]["qname"], "GET /users");
    assert_eq!(r["values"][2]["str"], "HTTP_CALLS");
    assert_eq!(r["tier"], "derived");

    // Without --with there is no second repo, so no cross-repo edge.
    let alone = glia(&[&front, "-e", program], None, 1);
    assert!(
        stdout(&alone).contains("no row of x/3 in this graph"),
        "{}",
        stdout(&alone)
    );
}
