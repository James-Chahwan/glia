//! CC.8b — `glia contract-breaks`, the CI gate over the engine's contract
//! breaks (`glia_engine::contract_breaks::contract_breaks_vs_rev`, CC.8a),
//! driving the real binary over temporary git repos. These tests need a `git`
//! binary: without one they FAIL with a message saying so, never skip.
//!
//! Every git call — the fixture's and the binary's — runs hermetically: a
//! fixed identity, no signing, `main` as the initial branch, no system or
//! global config and `HOME` inside the scratch dir (the isolation of
//! `delta_cli.rs`; the cli crate has no dev-dependencies, so no `tempfile`).
//!
//! The engine's `[contract-breaks] base=<rev> pairs=<P> breaking=<B> ...`
//! stderr line is the fired_on marker; asserting it here makes it a tested
//! contract. Grep it from a run of the binary:
//! `glia contract-breaks <repo> 2>&1 >/dev/null | grep '^\[contract-breaks\] base='`.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

/// CC.8a's openapi fixture: one GET op whose 200 response declares `id` and
/// `total`; `get:` sits on line 7.
const ORDERS_V1: &str = "openapi: 3.0.0
info:
  title: orders
  version: \"1\"
paths:
  /orders/{id}:
    get:
      operationId: getOrder
      responses:
        \"200\":
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  id:
                    type: string
                  total:
                    type: number
";

const TOTAL: &str = "                  total:\n                    type: number\n";

/// `ORDERS_V1` without the response's `total`: a breaking change.
fn orders_without_total() -> String {
    ORDERS_V1.replace(TOTAL, "")
}

/// `ORDERS_V1` with a new optional response field: a compatible change.
fn orders_with_currency() -> String {
    format!("{ORDERS_V1}                  currency:\n                    type: string\n")
}

const CLIENT_PY: &str =
    "import requests\n\n\ndef list_orders():\n    return requests.get(\"http://api/orders\")\n";
const APP_PY: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/orders\")\ndef orders():\n    return []\n";

const MARKER_BREAKING: &str = "[contract-breaks] base=HEAD pairs=1 breaking=1 compatible=0 unknown=0 removed=0 added=0 orphaned_clients=0";

/// A scratch dir holding a git work tree (`repo/`), an empty global git
/// config and a `HOME`; removed on drop.
struct Scratch {
    root: PathBuf,
    top: PathBuf,
    home: PathBuf,
    gitconfig: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Scratch {
    /// A fresh dir with no git work tree in it (`git_repo` inits one).
    fn plain(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-cc8b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        Scratch { root, top, home, gitconfig }
    }

    /// A fresh `git init` (branch `main`) with no commits.
    fn git_repo(name: &str) -> Self {
        let s = Scratch::plain(name);
        s.git(&["init", "-q"]);
        s
    }

    /// The committed `openapi.yaml` of [`ORDERS_V1`].
    fn orders(name: &str) -> Self {
        let s = Scratch::git_repo(name);
        s.write("openapi.yaml", ORDERS_V1);
        s.commit("v1");
        s
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    /// Write `text` to the repo-relative `rel`. Not staged.
    fn write(&self, rel: &str, text: &str) {
        let p = self.top.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent dir");
        }
        std::fs::write(&p, text).expect("fixture write");
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    /// `cmd` with this scratch dir's hermetic git environment.
    fn hermetic(&self, mut cmd: Command) -> Command {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("HOME", &self.home)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd
    }

    /// One hermetic git command in the work tree; panics on failure.
    fn git(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "user.name=glia",
            "-c",
            "user.email=glia@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(&self.top)
        .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("CC.8b contract-breaks tests need a `git` binary on PATH; running it failed: {e}")
        });
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            self.top.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// `glia <args>` under the same hermetic git environment.
    fn glia(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.args(args);
        self.hermetic(cmd).output().expect("run glia")
    }

    /// `glia contract-breaks <repo> <extra>`.
    fn breaks(&self, extra: &[&str]) -> Output {
        let mut args = vec!["contract-breaks", self.path()];
        args.extend_from_slice(extra);
        self.glia(&args)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn show(o: &Output) -> String {
    format!("exit {:?}\nstdout:\n{}\nstderr:\n{}", o.status.code(), stdout(o), stderr(o))
}

/// The engine's `[contract-breaks] base=..` line, once per run.
fn markers(o: &Output) -> Vec<String> {
    stderr(o)
        .lines()
        .filter(|l| l.starts_with("[contract-breaks] base="))
        .map(str::to_string)
        .collect()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", show(o)))
}

/// The table rows (`| ... |` lines, header and rule lines excluded) under the
/// `## <section>` heading of `out`, up to the next heading.
fn section_rows(out: &str, section: &str) -> Vec<String> {
    let heading = format!("## {section}");
    let mut rows = Vec::new();
    let mut inside = false;
    let mut header_seen = false;
    for line in out.lines() {
        if line.starts_with("## ") || line.starts_with("# ") {
            inside = line == heading;
            header_seen = false;
            continue;
        }
        if inside && line.starts_with('|') {
            if !header_seen {
                header_seen = true;
            } else if !line.starts_with("|---") {
                rows.push(line.to_string());
            }
        }
    }
    rows
}

#[test]
fn removed_response_field_fails_the_gate_and_a_revert_passes_it() {
    let s = Scratch::orders("removed-field");
    s.write("openapi.yaml", &orders_without_total());
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(1), "a breaking change exits 1\n{}", show(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with(&format!("# glia contract-breaks `{}` vs HEAD\n", s.path())),
        "{}",
        show(&o)
    );
    let rows = section_rows(&out, "breaking");
    assert_eq!(rows.len(), 1, "one row per field change\n{}", show(&o));
    let row = &rows[0];
    for want in [
        "operation",
        "`GET /orders/{id}`",
        "modified",
        "fact",
        "openapi.yaml:7",
        "response_field_removed",
        "total",
        "number",
    ] {
        assert!(row.contains(want), "row lacks `{want}`: {row}\n{}", show(&o));
    }
    assert!(!out.contains("## compatible") && !out.contains("## unknown"), "{}", show(&o));
    assert!(!out.contains("## orphaned clients"), "no client lost its provider\n{}", show(&o));
    assert_eq!(markers(&o), [MARKER_BREAKING], "{}", show(&o));

    s.write("openapi.yaml", ORDERS_V1);
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(0), "a reverted tree passes\n{}", show(&o));
    let out = stdout(&o);
    assert!(out.contains("no contract change vs HEAD"), "{}", show(&o));
    assert!(out.contains("> FACT: "), "the engine's absence is printed\n{}", show(&o));
    assert!(!out.contains("## breaking"), "{}", show(&o));
    assert_eq!(
        markers(&o),
        ["[contract-breaks] base=HEAD pairs=1 breaking=0 compatible=0 unknown=0 removed=0 added=0 orphaned_clients=0"],
        "{}",
        show(&o)
    );
}

#[test]
fn json_is_the_engine_answer_and_keeps_the_exit_code() {
    let s = Scratch::orders("json");
    s.write("openapi.yaml", &orders_without_total());
    let o = s.breaks(&["--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let v = json(&o);
    let mut keys: Vec<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["absence", "base", "breaking", "orphaned_clients", "schemas"], "{v}");
    let text = stdout(&o);
    let order: Vec<usize> = ["\"base\":", "\"schemas\":", "\"orphaned_clients\":", "\"breaking\":", "\"absence\":"]
        .iter()
        .map(|k| text.rfind(k).unwrap_or(usize::MAX))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "the engine's field order (a top-level key is its last occurrence): {text}");
    assert_eq!(v["base"], "HEAD");
    assert_eq!(v["breaking"], 1);
    assert!(v["absence"].is_null(), "{v}");
    let row = &v["schemas"][0];
    assert_eq!((row["status"].as_str(), row["key"].as_str()), (Some("breaking"), Some("GET /orders/{id}")));
    let c = &row["changes"][0];
    assert_eq!(
        (c["section"].as_str(), c["field"].as_str(), c["rule"].as_str(), c["breaking"].as_bool()),
        (Some("response:200"), Some("total"), Some("response_field_removed"), Some(true)),
        "{v}"
    );
    assert_eq!(markers(&o), [MARKER_BREAKING], "{}", show(&o));
}

#[test]
fn compatible_change_passes_and_breaking_only_hides_it() {
    let s = Scratch::orders("compatible");
    s.write("openapi.yaml", &orders_with_currency());
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(0), "a compatible change never fails the gate\n{}", show(&o));
    let out = stdout(&o);
    assert!(section_rows(&out, "breaking").is_empty(), "{}", show(&o));
    let rows = section_rows(&out, "compatible");
    assert_eq!(rows.len(), 1, "{}", show(&o));
    assert!(
        rows[0].contains("response_field_added") && rows[0].contains("currency"),
        "{}",
        show(&o)
    );

    let o = s.breaks(&["--breaking-only"]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let out = stdout(&o);
    assert!(!out.contains("## compatible"), "--breaking-only leaves compatible rows out\n{}", show(&o));
    assert!(out.contains("no breaking contract change vs HEAD"), "{}", show(&o));
    assert!(out.contains("left out by breaking_only"), "the absence says why\n{}", show(&o));
}

#[test]
fn removed_op_is_one_row_located_at_the_base() {
    let s = Scratch::orders("removed-op");
    s.write("openapi.yaml", "openapi: 3.0.0\ninfo:\n  title: orders\n  version: \"1\"\npaths: {}\n");
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let rows = section_rows(&stdout(&o), "breaking");
    assert_eq!(rows.len(), 1, "a removed op is one row\n{}", show(&o));
    assert!(
        rows[0].contains("removed") && rows[0].contains("openapi.yaml:7 (at HEAD)"),
        "{}",
        show(&o)
    );
}

#[test]
fn orphaned_client_fails_the_gate() {
    let s = Scratch::git_repo("orphan");
    s.write("web/pyproject.toml", "[project]\nname = \"web\"\n");
    s.write("web/client.py", CLIENT_PY);
    s.write("services/api/pyproject.toml", "[project]\nname = \"api\"\n");
    s.write("services/api/app.py", APP_PY);
    s.commit("client + service");
    std::fs::remove_file(s.top.join("services/api/app.py")).expect("remove the route");
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(1), "an orphaned client is breaking\n{}", show(&o));
    let out = stdout(&o);
    let rows = section_rows(&out, "orphaned clients");
    assert_eq!(rows.len(), 1, "{}", show(&o));
    for want in [
        "`endpoint:GET:/orders @web`",
        "web/client.py:5",
        "HTTP_CALLS",
        "`GET /orders @services/api`",
        "target_removed",
        "fact",
    ] {
        assert!(rows[0].contains(want), "orphan row lacks `{want}`\n{}", show(&o));
    }
    assert!(markers(&o)[0].ends_with("orphaned_clients=1"), "{}", show(&o));
}

#[test]
fn unknown_avro_mode_is_a_usage_error() {
    let s = Scratch::orders("avro");
    let o = s.breaks(&["--avro", "sideways"]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    let err = stderr(&o);
    assert!(err.contains("sideways"), "{}", show(&o));
    for mode in ["backward", "forward", "full"] {
        assert!(err.contains(mode), "the error lists `{mode}`\n{}", show(&o));
    }
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
    for mode in ["forward", "full"] {
        let o = s.breaks(&["--avro", mode]);
        assert_eq!(o.status.code(), Some(0), "--avro {mode}\n{}", show(&o));
    }
}

#[test]
fn not_a_git_work_tree_exits_2() {
    let s = Scratch::plain("not-git");
    s.write("openapi.yaml", ORDERS_V1);
    let o = s.breaks(&[]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(
        stderr(&o).contains("error: not a git work tree"),
        "the engine's message, not a clap usage error\n{}",
        show(&o)
    );
    assert!(stdout(&o).is_empty(), "{}", show(&o));
}

#[test]
fn no_overlay_is_refused() {
    let s = Scratch::orders("no-overlay");
    let o = s.glia(&["--no-overlay", "contract-breaks", s.path()]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("--no-overlay"), "{}", show(&o));
}
