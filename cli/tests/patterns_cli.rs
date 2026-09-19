//! LE.7b — `glia patterns --experimental`, the CLI surface of the engine's
//! pattern conformance (LE.7a), driving the real binary over the Go shop of
//! `engine/tests/patterns.rs` in a scratch dir: gin routes in
//! `handlers/handlers.go`, service functions in `service/service.go`, raw SQL
//! in `repository/repository.go`. Five handlers go handler > service >
//! repository > db; `RawOrderHandler` goes straight to the repository.
//!
//! This surface's `[patterns] experimental surface=cli mode=<graph|delta>`
//! stderr line is the fired_on marker; asserting it here makes it a tested
//! contract. Grep it from a run:
//! `glia patterns <repo> --experimental --base HEAD 2>&1 >/dev/null | grep '^\[patterns\] experimental surface='`.
//!
//! The delta test needs a `git` binary: without one it FAILS with a message
//! saying so, never skips. Its git calls run hermetically (fixed identity, no
//! signing, no system or global config, `HOME` in the scratch dir), as
//! `engine/tests/git_fixture` does.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const REFUSAL: &str = "patterns is experimental: pass --experimental (output format may change)";
const CONVENTION: &str = "handler>service>repository>db";
const DIRECT_SIGNATURE: &str = "handler>repository>db";
const DIRECT: &str = "handlers::handlers::RawOrderHandler";
const GRAPH_MARKER: &str = "[patterns] experimental surface=cli mode=graph";
const DELTA_MARKER: &str = "[patterns] experimental surface=cli mode=delta";

/// `(name, gin registration, service fn or None for a direct call,
/// repository fn, SQL)` per handler; the last one skips the service layer.
const HANDLERS: [(&str, &str, Option<&str>, &str, &str); 6] = [
    ("GetUserHandler", "GET(\"/users/:id\"", Some("GetUser"), "FindUser", "SELECT name FROM users WHERE id = $1"),
    ("CreateUserHandler", "POST(\"/users\"", Some("CreateUser"), "InsertUser", "INSERT INTO users (name) VALUES ($1)"),
    ("GetOrderHandler", "GET(\"/orders/:id\"", Some("GetOrder"), "FindOrder", "SELECT item FROM orders WHERE id = $1"),
    ("ListProductsHandler", "GET(\"/products\"", Some("ListProducts"), "AllProducts", "SELECT name FROM products WHERE id > $1"),
    ("CreatePaymentHandler", "POST(\"/payments\"", Some("CreatePayment"), "InsertPayment", "INSERT INTO payments (amount) VALUES ($1)"),
    ("RawOrderHandler", "POST(\"/orders\"", None, "SaveOrder", "INSERT INTO orders (item) VALUES ($1)"),
];

/// `handlers/handlers.go` for the first `n` handlers.
fn handlers_go(n: usize) -> String {
    let hs = &HANDLERS[..n];
    let mut s = String::from("package handlers\n\nimport (\n\t\"net/http\"\n\n");
    if hs.iter().any(|h| h.2.is_none()) {
        s.push_str("\t\"example.com/shop/repository\"\n");
    }
    s.push_str("\t\"example.com/shop/service\"\n\t\"github.com/gin-gonic/gin\"\n)\n\nfunc Register(r *gin.Engine) {\n");
    for (name, route, ..) in hs {
        s.push_str(&format!("\tr.{route}, {name})\n"));
    }
    s.push_str("}\n");
    for (name, _, svc, repo, _) in hs {
        let callee = match svc {
            Some(f) => format!("service.{f}"),
            None => format!("repository.{repo}"),
        };
        s.push_str(&format!(
            "\nfunc {name}(c *gin.Context) {{\n\tv := {callee}(c.Param(\"id\"))\n\tc.JSON(http.StatusOK, v)\n}}\n"
        ));
    }
    s
}

/// A scratch dir holding the tree (`repo/`), an empty global git config and a
/// `HOME`; removed on drop (`cli` has no dev-dependencies, so no `tempfile`).
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
    /// The shop with its first `n` handlers.
    fn shop(name: &str, n: usize) -> Self {
        let root = std::env::temp_dir().join(format!("glia-le7b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        let s = Scratch { root, top, home, gitconfig };
        std::fs::create_dir_all(&s.top).expect("scratch repo dir");
        std::fs::write(&s.gitconfig, "").expect("empty gitconfig");
        s.write_shop(n);
        s
    }

    fn write_shop(&self, n: usize) {
        let hs = &HANDLERS[..n];
        let mut service = String::from("package service\n\nimport \"example.com/shop/repository\"\n");
        let mut repository = String::from("package repository\n\nimport \"database/sql\"\n\nvar db *sql.DB\n");
        for (_, _, svc, repo, sql) in hs {
            if let Some(f) = svc {
                service.push_str(&format!("\nfunc {f}(id string) string {{\n\treturn repository.{repo}(id)\n}}\n"));
            }
            repository.push_str(&format!(
                "\nfunc {repo}(id string) string {{\n\tdb.Exec(\"{sql}\", id)\n\treturn id\n}}\n"
            ));
        }
        self.write("go.mod", "module example.com/shop\n\ngo 1.21\n\nrequire github.com/gin-gonic/gin v1.9.1\n");
        self.write("handlers/handlers.go", &handlers_go(n));
        self.write("service/service.go", &service);
        self.write("repository/repository.go", &repository);
    }

    fn path(&self) -> &str {
        self.top.to_str().expect("utf-8 scratch path")
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.top.join(rel);
        std::fs::create_dir_all(p.parent().expect("a parent dir")).expect("fixture dir");
        std::fs::write(&p, text).expect("fixture write");
    }

    /// `cmd` with this scratch dir's hermetic git environment and no layout
    /// persisted.
    fn hermetic(&self, mut cmd: Command) -> Command {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("HOME", &self.home)
            .env("GLIA_NO_PERSIST", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd
    }

    fn git(&self, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "user.name=glia", "-c", "user.email=glia@example.invalid"])
            .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(&self.top)
            .args(args);
        let out = self.hermetic(cmd).output().unwrap_or_else(|e| {
            panic!("LE.7b patterns --base needs a `git` binary on PATH; running it failed: {e}")
        });
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// `glia patterns <repo> <args>`.
    fn patterns(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
        cmd.arg("patterns").arg(self.path()).args(args);
        self.hermetic(cmd).output().expect("run glia")
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", stdout(o)))
}

fn divergent(v: &Value) -> Vec<&str> {
    v["divergences"]
        .as_array()
        .expect("divergences")
        .iter()
        .map(|d| d["handler"].as_str().unwrap_or(""))
        .collect()
}

/// 1-based line of the first line of `text` containing `needle`.
fn line_of(text: &str, needle: &str) -> usize {
    text.lines().position(|l| l.contains(needle)).expect("needle in fixture") + 1
}

/// Without `--experimental` the command refuses, before building anything:
/// exit 2, the message on stderr, nothing on stdout, no marker.
#[test]
fn refuses_without_flag() {
    let s = Scratch::shop("refuse", 6);
    for args in [&[][..], &["--json"][..], &["--base", "HEAD"][..]] {
        let out = s.patterns(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out).trim_end(), REFUSAL, "{args:?}");
        assert!(stdout(&out).is_empty(), "{args:?}: {}", stdout(&out));
    }
    // Even a path that does not exist is refused on the flag, not the build.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_glia"));
    let out = cmd.args(["patterns", "/no/such/glia-le7b-repo"]).output().expect("run glia");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stderr(&out).trim_end(), REFUSAL);
}

/// `--experimental --json` on the whole graph: exit 0, the engine's report
/// with `experimental` true, one population judged 5/6 and the one located
/// divergence; the surface marker beside the engine's.
#[test]
fn json_has_experimental_true() {
    let s = Scratch::shop("json", 6);
    let out = s.patterns(&["--experimental", "--json"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let v = json(&out);
    assert_eq!(v["experimental"], true, "{v}");
    assert_eq!(v["delta_mode"], false);
    assert_eq!((v["handlers"].as_u64(), v["judged"].as_u64()), (Some(6), Some(1)), "{v}");
    assert_eq!(divergent(&v), [DIRECT]);
    let d = &v["divergences"][0];
    assert_eq!((d["verdict"].as_str(), d["tier"].as_str()), (Some("DIVERGENCE"), Some("heuristic")));
    assert_eq!(d["signature"], DIRECT_SIGNATURE);
    assert_eq!(d["convention"], CONVENTION);
    assert_eq!(d["file"], "handlers/handlers.go");
    let line = line_of(&handlers_go(6), "func RawOrderHandler(");
    assert_eq!(d["line"].as_u64(), Some(line as u64));
    let p = &v["populations"][0];
    assert_eq!((p["status"].as_str(), p["verdict"].as_str()), (Some("judged"), Some("5/6")), "{p}");
    assert!(err.lines().any(|l| l == GRAPH_MARKER), "{err}");
    assert!(err.lines().any(|l| l.starts_with("[patterns] experimental populations=1 judged=1 handlers=6 divergences=1")), "{err}");
}

/// Table mode: the counts, then the population's heading with its verdict and
/// the divergence row, located; `--min-support` above the population prints
/// the below-support line instead; `--min-share` above 100 is a usage error.
#[test]
fn table_names_the_population_and_its_status() {
    let s = Scratch::shop("table", 6);
    let out = s.patterns(&["--experimental"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with(&format!("# glia patterns `{}` (experimental, whole graph)\n", s.path())), "{text}");
    assert!(text.contains("- handlers: 6 in 1 populations; judged: 1; below min support (5): 0; divergences: 1\n"), "{text}");
    assert!(text.contains("- excluded handlers: none\n"), "{text}");
    assert!(text.contains(&format!("## handlers - 5/6 follow `{CONVENTION}`\n")), "{text}");
    let line = line_of(&handlers_go(6), "func RawOrderHandler(");
    let row = format!("| `{DIRECT}` | POST /orders | `{DIRECT_SIGNATURE}` | handlers/handlers.go:{line} |");
    assert!(text.lines().any(|l| l == row), "no row {row} in:\n{text}");

    let small = s.patterns(&["--experimental", "--min-support", "7"]);
    assert_eq!(small.status.code(), Some(0), "{}", stderr(&small));
    let text = stdout(&small);
    assert!(text.contains("_(population below min support: `handlers` has 6 handlers, fewer than 7)_"), "{text}");
    assert!(!text.contains("| handler |"), "{text}");

    let split = s.patterns(&["--experimental", "--min-share", "90"]);
    assert_eq!(split.status.code(), Some(0), "{}", stderr(&split));
    assert!(stdout(&split).contains("## handlers - no convention: no signature holds 90% of 6 handlers"), "{}", stdout(&split));

    let bad = s.patterns(&["--experimental", "--min-share", "101"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("--min-share is a percentage (0-100), got 101"), "{}", stderr(&bad));
}

/// `--base` builds the one repo against its rev, so `--with` beside it is a
/// usage error (exit 2) and nothing is printed on stdout.
#[test]
fn base_with_with_is_rejected() {
    let s = Scratch::shop("with", 6);
    let other = Scratch::shop("with-other", 5);
    let out = s.patterns(&["--experimental", "--base", "HEAD", "--with", other.path()]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--with needs whole-graph mode"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
    assert!(!stderr(&out).contains(DELTA_MARKER));
}

/// Delta mode: five layered handlers committed, the direct one added in the
/// working tree. The conventions come from the whole working tree (5/6) and
/// the added handler is the one divergence; the delta marker fires. With all
/// six committed and an unrelated edit, the divergence is no longer touched:
/// the table says so and counts it in the whole graph.
#[test]
fn base_lists_only_touched_divergences() {
    let s = Scratch::shop("delta", 5);
    s.git(&["init", "-q"]);
    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "five layered handlers"]);
    s.write_shop(6);
    let out = s.patterns(&["--experimental", "--base", "HEAD", "--json"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let v = json(&out);
    assert_eq!((v["experimental"].as_bool(), v["delta_mode"].as_bool()), (Some(true), Some(true)), "{v}");
    assert_eq!(divergent(&v), [DIRECT]);
    assert_eq!(v["populations"][0]["verdict"], "5/6");
    assert!(err.lines().any(|l| l == DELTA_MARKER), "{err}");
    assert!(err.lines().any(|l| l.starts_with("[patterns] delta touched_nodes=")), "{err}");

    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "six handlers"]);
    s.write("util/util.go", "package util\n\nfunc Clamp(v int) int {\n\treturn v\n}\n");
    let out = s.patterns(&["--experimental", "--base", "HEAD"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with(&format!("# glia patterns `{}` (experimental, delta vs `HEAD`)\n", s.path())), "{text}");
    assert!(text.contains("divergences touched by the change: 0\n"), "{text}");
    assert!(text.contains("_(no divergence touched by the change; 1 in the whole graph)_"), "{text}");

    let bad = s.patterns(&["--experimental", "--base", "no-such-rev"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("no-such-rev"), "{}", stderr(&bad));
}

/// A tree with no route handler reads as such, not as an empty "all good".
#[test]
fn no_handlers_says_nothing_to_judge() {
    let s = Scratch::shop("empty", 6);
    for rel in ["handlers/handlers.go", "service/service.go", "repository/repository.go"] {
        std::fs::remove_file(Path::new(s.path()).join(rel)).expect("remove");
    }
    s.write("util/util.go", "package util\n\nfunc Clamp(v int) int {\n\treturn v\n}\n");
    let out = s.patterns(&["--experimental"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("_(no route handler placed in any population: nothing to judge)_"), "{}", stdout(&out));
}
