//! CC.6b — `glia review`, the PR-report gate over the engine's review
//! (`glia_engine::review::{review_vs_rev, render_markdown}`, CC.6a / CC.6b),
//! driving the real binary over temporary git repos. These tests need a `git`
//! binary: without one they FAIL with a message saying so, never skip.
//!
//! Every git call — the fixture's and the binary's — runs hermetically: a
//! fixed identity, no signing, `main` as the initial branch, no system or
//! global config and `HOME` inside the scratch dir (the isolation of
//! `delta_cli.rs`; the cli crate has no dev-dependencies, so no `tempfile`).
//!
//! The tree is CC.6a's: two manifest projects (web/, services/api/),
//! `services/api/internal.py` `charge`, `web/app.py` `pay`,
//! `tests/test_pay.py` calling `pay`, and a `.glia/overlay.toml` forbidding
//! web -> services/api over IMPORTS + CALLS.
//!
//! The engine's `[review] base=<rev> ... blocking=<bool>` stderr line is the
//! fired_on marker, then this surface's `[review] surface=cli format=..
//! exit=..`; asserting both here makes them a tested contract. Grep them from
//! a run of the binary:
//! `glia review <repo> 2>&1 >/dev/null | grep '^\[review\] '`.
//!
//! DT.1p: over the engine's `model-breaks-shop` fixture (copied into a
//! scratch repo, v1 then the json tag rename as v2) an inferred model break
//! is advisory by default (exit 0) and blocking under `--models-min-tier
//! derived` (exit 1), with the engine's `[model-breaks] base=..` line before
//! `[review] base=`; a rule's coverage caveats print once per rule
//! (`[review] caveats rules=<r> lines=<l> folded=<f>`).

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

const MANIFESTS: [(&str, &str); 2] = [
    ("web/pyproject.toml", "[project]\nname = \"web\"\n"),
    ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
];
const INTERNAL: &str = "def charge(o):\n    return o\n";
/// `INTERNAL` with `charge`'s body edited: unrelated to the rule.
const INTERNAL_EDITED: &str = "def charge(o):\n    return o or 0\n";
const WEB_APP_CLEAN: &str = "def pay(o):\n    return o\n";
/// `pay` imports and calls the api's internals: IMPORTS on line 1, CALLS on
/// line 5.
const WEB_APP: &str =
    "from services.api.internal import charge\n\n\ndef pay(o):\n    return charge(o)\n";
const TEST_PAY: &str = "from web.app import pay\n\n\ndef test_pay():\n    assert pay(1) == 1\n";
/// The forbid_edge rule; `[[constraint]]` is on line 3.
const RULES: &str = "version = 1\n\n[[constraint]]\nid = \"web-no-api-internals\"\nkind = \"forbid_edge\"\nfrom = \"web\"\nto = \"services/api\"\ncategories = [\"IMPORTS\", \"CALLS\"]\n";

/// The Review's JSON keys, in the engine's field order (`unanchored` only
/// when a rule checks nothing).
const KEYS: [&str; 11] = [
    "base",
    "counts",
    "changed",
    "impact",
    "tests",
    "edges",
    "new_violations",
    "resolved_violations",
    "model_breaks",
    "check_errors",
    "blocking",
];

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
        let root = std::env::temp_dir().join(format!("glia-cc6b-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let top = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&top).expect("scratch repo dir");
        std::fs::create_dir_all(&home).expect("scratch HOME dir");
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        Scratch {
            root,
            top,
            home,
            gitconfig,
        }
    }

    /// CC.6a's tree with `web_app` as `web/app.py`, committed on `main`.
    fn committed(name: &str, web_app: &str) -> Self {
        let s = Scratch::plain(name);
        s.git(&["init", "-q"]);
        for (rel, src) in MANIFESTS {
            s.write(rel, src);
        }
        s.write("services/api/internal.py", INTERNAL);
        s.write("web/app.py", web_app);
        s.write("tests/test_pay.py", TEST_PAY);
        s.write(".glia/overlay.toml", RULES);
        s.git(&["add", "-A"]);
        s.git(&["commit", "-q", "-m", "clean"]);
        s
    }

    /// The engine's `model-breaks-shop` fixture committed as `v1`, then its
    /// first `json:"customerName"` tag (Order.CustomerName) renamed to
    /// `customer_name` and committed as `v2` (DT.1p).
    fn model_shop(name: &str) -> Self {
        let s = Scratch::plain(name);
        s.git(&["init", "-q"]);
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/data/model-breaks-shop");
        let mut files = Vec::new();
        files_under(&fixture, &fixture, &mut files);
        for rel in &files {
            let text = std::fs::read_to_string(fixture.join(rel))
                .unwrap_or_else(|e| panic!("read fixture {rel}: {e}"));
            s.write(rel, &text);
        }
        s.commit("v1");
        let order = s.top.join("api/internal/models/order.go");
        let text = std::fs::read_to_string(&order).expect("order.go");
        assert!(text.contains("`json:\"customerName\"`"), "the fixture's tag");
        let renamed = text.replacen("`json:\"customerName\"`", "`json:\"customer_name\"`", 1);
        std::fs::write(&order, renamed).expect("rename");
        s.commit("v2");
        s
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
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
            panic!("CC.6b review tests need a `git` binary on PATH; running it failed: {e}")
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

    /// `glia review <repo> <extra>`.
    fn review(&self, extra: &[&str]) -> Output {
        let mut args = vec!["review", self.path()];
        args.extend_from_slice(extra);
        self.glia(&args)
    }
}

/// Every file under `at`, relative to `dir`, sorted.
fn files_under(dir: &std::path::Path, at: &std::path::Path, out: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(at)
        .unwrap_or_else(|e| panic!("read {}: {e}", at.display()))
        .map(|e| e.expect("fixture dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files_under(dir, &p, out);
        } else {
            let rel = p.strip_prefix(dir).expect("under the fixture");
            out.push(rel.to_str().expect("utf-8 fixture path").to_string());
        }
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn show(o: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        stdout(o),
        stderr(o)
    )
}

/// The `[review] ` lines on stderr: the engine's, then the surface's.
fn markers(o: &Output) -> Vec<String> {
    stderr(o)
        .lines()
        .filter(|l| l.starts_with("[review] "))
        .map(str::to_string)
        .collect()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{}", show(o)))
}

/// The keys of the JSON object `text`, in the order printed (`serde_json`'s
/// `Value` sorts them).
fn top_level_keys(text: &str) -> Vec<String> {
    let (mut keys, mut depth, mut in_str, mut escaped) = (Vec::new(), 0usize, false, false);
    let mut current = String::new();
    let mut prev_structural = ' ';
    for c in text.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
                if depth == 1 && matches!(prev_structural, '{' | ',') {
                    keys.push(std::mem::take(&mut current));
                }
                current.clear();
                continue;
            }
            current.push(c);
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => {
                depth += 1;
                prev_structural = c;
            }
            '}' | ']' => {
                depth -= 1;
                prev_structural = c;
            }
            ',' | ':' => prev_structural = c,
            _ => {}
        }
    }
    keys
}

/// A new violation in the working tree fails the gate: exit 1, the markdown
/// report on stdout, the engine's marker saying `blocking=true`.
#[test]
fn a_new_violation_exits_1_with_the_markdown_report() {
    let s = Scratch::committed("new", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with("## glia review vs `HEAD`\n**1 new violation(s)**"),
        "{}",
        show(&o)
    );
    for want in [
        "### New violations (blocking)",
        "#### web-no-api-internals (forbid_edge, .glia/overlay.toml:3)",
        "| IMPORTS | `web::app` | `services::api::internal` | web/app.py:1 | fact |",
        "### Tests to run",
        "| `tests::test_pay::test_pay` |",
    ] {
        assert!(out.contains(want), "missing {want:?}\n{}", show(&o));
    }
    let m = markers(&o);
    assert_eq!(m.len(), 2, "{}", show(&o));
    assert!(
        m[0].starts_with("[review] base=HEAD changed=")
            && m[0].contains(" edges +2 -0 (fact=2 derived=0 heuristic=0) violations new=1 resolved=0 blocking=true"),
        "{}",
        show(&o)
    );
    assert_eq!(m[1], "[review] surface=cli format=markdown exit=1");
}

/// A violation the base already had does not fail the gate (`glia check`
/// would): exit 0, no blocking section.
#[test]
fn a_pre_existing_violation_exits_0() {
    let s = Scratch::committed("pre-existing", WEB_APP);
    s.write("services/api/internal.py", INTERNAL_EDITED);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let out = stdout(&o);
    assert!(
        out.starts_with("## glia review vs `HEAD`\n**0 new violation(s)** | 0 resolved |"),
        "{}",
        show(&o)
    );
    assert!(!out.contains("### New violations"), "{}", show(&o));
    assert!(
        markers(&o)[0].ends_with("violations new=0 resolved=0 blocking=false"),
        "{}",
        show(&o)
    );
    let check = s.glia(&["check", s.path()]);
    assert_eq!(
        check.status.code(),
        Some(1),
        "check fails on a blocking violation (this one is fact)\n{}",
        show(&check)
    );
}

/// `--json` is the engine's Review with its keys in field order, and keeps
/// the exit code.
#[test]
fn json_is_the_review_and_keeps_the_exit_code() {
    let s = Scratch::committed("json", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&["--json", "--base", "HEAD"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let v = json(&o);
    assert_eq!(
        top_level_keys(&stdout(&o)),
        KEYS,
        "exactly the Review's keys, in field order"
    );
    assert_eq!(
        (v["base"].as_str(), v["blocking"].as_bool()),
        (Some("HEAD"), Some(true))
    );
    assert_eq!(v["new_violations"][0]["rule_id"], "web-no-api-internals");
    assert_eq!(markers(&o)[1], "[review] surface=cli format=json exit=1");

    let capped = s.review(&[
        "--json",
        "--max-tests",
        "0",
        "--max-impact",
        "0",
        "--depth",
        "1",
    ]);
    let c = json(&capped);
    assert_eq!(
        c["tests"]["tests"].as_array().map(Vec::len),
        Some(0),
        "{}",
        show(&capped)
    );
    assert_eq!(
        c["impact"]["results"].as_array().map(Vec::len),
        Some(0),
        "{}",
        show(&capped)
    );
    assert_eq!(c["counts"], v["counts"], "the counts stay uncut");
}

/// `--markdown-rows` cuts each table and says so.
#[test]
fn markdown_rows_cut_the_tables() {
    let s = Scratch::committed("rows", WEB_APP_CLEAN);
    s.write("web/app.py", WEB_APP);
    let o = s.review(&["--markdown-rows", "1"]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let footers: Vec<String> = stdout(&o)
        .lines()
        .filter(|l| l.starts_with("_("))
        .map(str::to_string)
        .collect();
    assert_eq!(
        footers,
        ["_(1 of 2)_", "_(1 of 2)_", "_(1 of 3)_"],
        "{}",
        show(&o)
    );
}

/// A clean tree: exit 0 and the no-change sentence.
#[test]
fn no_change_exits_0() {
    let s = Scratch::committed("clean", WEB_APP_CLEAN);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        stdout(&o).ends_with("\n\nNo graph change vs HEAD.\n"),
        "{}",
        show(&o)
    );
}

/// The rev pair is one repo: `--with` is refused, and nothing is built.
#[test]
fn with_is_refused() {
    let s = Scratch::committed("with", WEB_APP_CLEAN);
    let o = s.review(&["--with", "x"]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("--with"), "{}", show(&o));
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
    assert!(stdout(&o).is_empty(), "{}", show(&o));
}

#[test]
fn not_a_git_work_tree_exits_2() {
    let s = Scratch::plain("not-git");
    s.write("web/app.py", WEB_APP);
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(
        stderr(&o).contains("error: not a git work tree"),
        "the engine's message, not a clap usage error\n{}",
        show(&o)
    );
    assert!(stdout(&o).is_empty(), "{}", show(&o));
}

#[test]
fn unknown_rev_and_no_overlay_exit_2() {
    let s = Scratch::committed("usage", WEB_APP_CLEAN);
    let o = s.review(&["--base", "no-such-rev"]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("no-such-rev"), "{}", show(&o));
    let o = s.glia(&["--no-overlay", "review", s.path()]);
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert!(stderr(&o).contains("--no-overlay"), "{}", show(&o));
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
}

/// The fixture's model-break line in the markdown.
const MODEL_ROW: &str = "| http_model | `GET /api/orders/:id` (+2) | derived | json_name_changed | `customerName` | customerName | customer_name | `web::src::app::core::models::order::Order` web/src/app/core/models/order.ts:11 | `api::internal::models::order::Order` api/internal/models/order.go:22 |";

/// DT.1p: the inferred model break of the v2 rename is advisory by default:
/// exit 0, the headline's model count, the advisory section; `--models-min-tier
/// derived` makes it block (exit 1); `heuristic` is a usage error.
#[test]
fn an_inferred_model_break_is_advisory_and_blocks_under_derived() {
    let s = Scratch::model_shop("model-breaks");
    let o = s.review(&["--base", "HEAD~1"]);
    assert_eq!(o.status.code(), Some(0), "advisory: the gate passes\n{}", show(&o));
    let out = stdout(&o);
    let headline = out.lines().nth(1).unwrap_or_default();
    assert!(
        headline.starts_with("**0 new violation(s)** | 0 resolved |")
            && headline.ends_with(" | 1 model break(s) (1 advisory)"),
        "{}",
        show(&o)
    );
    assert!(
        out.contains("### Model breaks (advisory: an inferred client <-> server pairing below the models min tier)"),
        "{}",
        show(&o)
    );
    assert!(out.contains(MODEL_ROW), "{}", show(&o));
    let m = markers(&o);
    assert_eq!(m.len(), 2, "exactly two [review] lines\n{}", show(&o));
    assert!(
        m[0].starts_with("[review] base=HEAD~1 ") && m[0].ends_with(" blocking=false"),
        "{}",
        show(&o)
    );
    assert_eq!(m[1], "[review] surface=cli format=markdown exit=0");
    let err = stderr(&o);
    let model = err
        .lines()
        .position(|l| l == "[model-breaks] base=HEAD~1 min_tier=fact exchanges=7 http=5 graphql=2 rows=1 breaking=1 blocking=0 advisory=1 compatible=0 new_pairings=0 lost=0 unjudged=0")
        .unwrap_or_else(|| panic!("the model marker\n{}", show(&o)));
    let review = err
        .lines()
        .position(|l| l.starts_with("[review] base="))
        .expect("the review marker");
    assert!(model < review, "[model-breaks] comes before [review] base=\n{}", show(&o));

    let o = s.review(&["--base", "HEAD~1", "--models-min-tier", "derived"]);
    assert_eq!(o.status.code(), Some(1), "derived: the break blocks\n{}", show(&o));
    let out = stdout(&o);
    assert!(out.contains("### Model breaks (blocking)"), "{}", show(&o));
    assert!(out.contains(MODEL_ROW), "{}", show(&o));
    let m = markers(&o);
    assert!(m[0].ends_with(" blocking=true"), "{}", show(&o));
    assert_eq!(m[1], "[review] surface=cli format=markdown exit=1");

    let o = s.review(&["--base", "HEAD~1", "--json"]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(top_level_keys(&stdout(&o)), KEYS, "{}", show(&o));
    let v = json(&o);
    assert_eq!(
        (v["model_breaks"][0]["pairing"].as_str(), v["counts"]["model_breaks"].as_u64(), v["blocking"].as_bool()),
        (Some("http_model"), Some(1), Some(false)),
        "{v}"
    );

    let o = s.review(&["--base", "HEAD~1", "--models-min-tier", "heuristic"]);
    assert_eq!(o.status.code(), Some(2), "clap refuses the tier\n{}", show(&o));
    assert!(stderr(&o).contains("fact, derived"), "{}", show(&o));
    assert!(markers(&o).is_empty(), "nothing is built\n{}", show(&o));
}

/// DT.8a's fold in review: two new violations of one negation-only rule
/// carry the same coverage caveats; the markdown prints one caveat paragraph
/// for the rule, and `[review] caveats ..` says what the fold saved. `--json`
/// keeps each violation's own list.
#[test]
fn a_rules_caveats_print_once() {
    let s = Scratch::plain("caveats");
    s.git(&["init", "-q"]);
    for (rel, text) in [
        ("pyproject.toml", "[project]\nname = \"root\"\n"),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\n"),
        ("store/pyproject.toml", "[project]\nname = \"store\"\n"),
        ("store/db.py", "def load(k):\n    return k\n"),
        ("services/api/handlers.py", HANDLERS),
        (".glia/overlay.toml", API_CALLED),
    ] {
        s.write(rel, text);
    }
    s.commit("rules");
    s.write("services/api/handlers.py", &format!("{HANDLERS}\n\ndef also_unused():\n    return 2\n\n\ndef third_unused():\n    return 3\n"));
    let o = s.review(&[]);
    assert_eq!(o.status.code(), Some(1), "two new fact violations block\n{}", show(&o));
    let out = stdout(&o);
    assert_eq!(
        out.matches("#### api-called (rule, .glia/overlay.toml:3)").count(),
        2,
        "two violations of the rule\n{}",
        show(&o)
    );
    let paragraphs: Vec<&str> = out.split("\n\n").filter(|p| p.starts_with("⚠ ")).collect();
    assert_eq!(paragraphs.len(), 1, "one caveat paragraph for the rule\n{}", show(&o));
    let lines = paragraphs[0].lines().count();
    let m = markers(&o);
    assert_eq!(m.len(), 3, "{}", show(&o));
    assert_eq!(m[1], "[review] surface=cli format=markdown exit=1");
    assert_eq!(
        m[2],
        format!("[review] caveats rules=1 lines={lines} folded={lines}"),
        "{}",
        show(&o)
    );
    let v = json(&s.review(&["--json"]));
    let caveats: Vec<usize> = v["new_violations"]
        .as_array()
        .expect("violations")
        .iter()
        .map(|x| x["caveats"].as_array().map_or(0, Vec::len))
        .collect();
    assert!(
        caveats.len() == 2 && caveats.iter().all(|&n| n > 0 && n.min(5) == lines),
        "every violation keeps its own list: {caveats:?}"
    );
}

/// [`a_rules_caveats_print_once`]'s handlers: `get_order` loads from the
/// store, `unused` is called by nothing.
const HANDLERS: &str = "from store.db import load\n\n\ndef get_order(o):\n    return load(o)\n\n\ndef unused():\n    return 1\n";
/// DB.3a's negation-only `api-called` invariant (`[[constraint]]` on line 3).
const API_CALLED: &str = r#"version = 2

[[constraint]]
id = "api-called"
kind = "invariant"
text = "every api handler is called"
rule = "violation(f) :- node(f, \"FUNCTION\"), in_scope(f, \"services/api/handlers.py\"), !called(f). called(f) :- edge(_, f, \"CALLS\")."
"#;
