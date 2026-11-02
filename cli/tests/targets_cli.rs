//! HT.7 — `glia targets sync|owners|deps|rdeps|affected`, driving the real
//! binary over a temp copy of engine/tests/data/targets (bazelbuild/examples'
//! cpp-tutorial/stage3 and go-tutorial/stage3, Apache-2.0, see that
//! directory's README) as workspaces `cpp` and `go`, synced `--from` the raw
//! `bazel query --output=streamed_jsonproto` outputs of
//! snapshots/tests/data/bazel/ (their `/WORKSPACE` prefix rewritten to the
//! copy's absolute workspace dir, as snapshots/tests/bazel.rs does). Nothing
//! runs bazel. The `affected` work-tree test needs a `git` binary.
//!
//! fired_on, on stderr: `[targets] sync ..`, `[targets] sidecar ..`,
//! `[targets] answer kind=..` and the surface's `[targets] surface=cli cmd=..`
//! (`cargo test -p glia-cli --test cli_tests targets_cli:: -- --nocapture
//! 2>&1 | grep '^\[targets\] '`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// A scratch dir holding `repo/` (the two workspaces' sources) and the
/// rewritten query outputs; removed on drop (`cli` has no `tempfile`).
struct Scratch {
    root: PathBuf,
    repo: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .expect("read_dir")
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let dst = to.join(e.file_name());
        if e.file_type().expect("file type").is_dir() {
            copy_tree(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).expect("copy");
        }
    }
}

impl Scratch {
    /// The sources copied, no snapshot yet.
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("glia-ht7-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let data = workspace_root().join("engine/tests/data/targets");
        for ws in ["cpp", "go"] {
            copy_tree(&data.join(ws), &repo.join(ws));
        }
        Scratch { root, repo }
    }

    /// The sources plus a snapshot synced from both fixtures.
    fn synced(name: &str) -> Self {
        let s = Self::new(name);
        for ws in ["cpp", "go"] {
            let (code, _, err) = s.sync(ws);
            assert_eq!(code, 0, "sync {ws}: {err}");
        }
        s
    }

    fn repo(&self) -> &str {
        self.repo.to_str().expect("utf-8 scratch path")
    }

    /// `snapshots/tests/data/bazel/<ws>-stage3.jsonl` with `/WORKSPACE` the
    /// copy's absolute workspace dir.
    fn query_output(&self, ws: &str) -> PathBuf {
        let raw = std::fs::read_to_string(
            workspace_root().join(format!("snapshots/tests/data/bazel/{ws}-stage3.jsonl")),
        )
        .expect("read fixture");
        let abs = std::fs::canonicalize(self.repo.join(ws)).expect("canonical workspace");
        let path = self.root.join(format!("{ws}.jsonl"));
        std::fs::write(&path, raw.replace("/WORKSPACE", &abs.display().to_string()))
            .expect("write query output");
        path
    }

    fn sync(&self, ws: &str) -> (i32, String, String) {
        let from = self.query_output(ws);
        glia(&[
            "targets",
            "sync",
            self.repo(),
            "--workspace",
            ws,
            "--from",
            from.to_str().expect("utf-8"),
        ])
    }

    fn git(&self, args: &[&str]) {
        let gitconfig = self.root.join("gitconfig");
        std::fs::write(&gitconfig, "").expect("empty gitconfig");
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture Author")
            .env("GIT_AUTHOR_EMAIL", "author@fixture.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture Committer")
            .env("GIT_COMMITTER_EMAIL", "committer@fixture.invalid")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap_or_else(|e| panic!("HT.7's affected test needs a `git` binary on PATH: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn glia(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_glia"))
        .args(args)
        .env("GLIA_NO_PERSIST", "1")
        .output()
        .expect("run glia");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    for line in stderr.lines().filter(|l| l.starts_with("[targets] ")) {
        eprintln!("{line}");
    }
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr,
    )
}

fn json(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"))
}

fn surface_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[targets] surface=cli "))
        .collect();
    assert_eq!(lines.len(), 1, "one surface line: {stderr}");
    lines[0]
}

fn labels(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .expect("rows")
        .iter()
        .filter_map(|r| r["label"].as_str())
        .collect()
}

#[test]
fn sync_writes_each_workspace_from_its_query_output() {
    let s = Scratch::new("sync");
    let (code, stdout, stderr) = s.sync("cpp");
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr.lines().any(|l| l.starts_with(
            "[targets] sync workspace=cpp rows=12 rules=3 source_files=7 external=1 skipped=0 \
             bazel=from-file -> "
        )),
        "{stderr}"
    );
    assert_eq!(
        surface_line(&stderr),
        "[targets] surface=cli cmd=sync workspace=cpp rows=12"
    );
    assert!(
        stdout.starts_with(
            "synced 12 rows (3 rules, 7 source files, 1 external labels, 0 skipped) \
             of workspace cpp (bazel from-file) -> "
        ),
        "{stdout}"
    );
    assert!(s.repo.join(".glia/targets-snapshot/meta.json").is_file());

    let from = s.query_output("go");
    let (code, stdout, stderr) = glia(&[
        "targets",
        "sync",
        s.repo(),
        "--workspace",
        "go",
        "--from",
        from.to_str().expect("utf-8"),
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let j = json(&stdout);
    assert_eq!(j["workspace"], "go");
    assert_eq!(j["targets"], 8);
    assert_eq!(j["rules"], 3);
    assert_eq!(j["source_files"], 5);
    assert_eq!(j["bazel_version"], "from-file");
}

#[test]
fn sync_errors_exit_2_and_write_nothing() {
    let s = Scratch::new("sync-err");
    let missing = s.root.join("missing.jsonl");
    let (code, stdout, stderr) = glia(&[
        "targets",
        "sync",
        s.repo(),
        "--workspace",
        "cpp",
        "--from",
        missing.to_str().expect("utf-8"),
    ]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("error: read "), "{stderr}");
    assert!(!stderr.contains("[targets] surface=cli"), "{stderr}");

    // No workspace marker under the repo root and no --from: refused before
    // bazel is looked for.
    let (code, _, stderr) = glia(&["targets", "sync", s.repo()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no bazel workspace at"), "{stderr}");
    let (code, _, stderr) = glia(&["targets", "sync", s.repo(), "--workspace", "../out"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(!s.repo.join(".glia/targets-snapshot").exists());
}

#[test]
fn no_snapshot_exits_2_naming_the_sync() {
    let s = Scratch::new("nosnap");
    for args in [
        vec!["owners", s.repo(), "cpp/lib/hello-time.h"],
        vec!["deps", s.repo(), "//main:hello-world"],
        vec!["rdeps", s.repo(), "//fortune"],
        vec!["affected", s.repo(), "go/fortune/fortune.go"],
    ] {
        let mut full = vec!["targets"];
        full.extend(args.iter().copied());
        let (code, stdout, stderr) = glia(&full);
        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(
            stderr.contains("no targets snapshot at") && stderr.contains("glia targets sync"),
            "{stderr}"
        );
    }
    assert!(!s.repo.join(".glia/graph/targets.gmap").exists());
}

#[test]
fn owners_text_and_json() {
    let s = Scratch::synced("owners");
    let (code, stdout, stderr) = glia(&[
        "targets",
        "owners",
        s.repo(),
        "cpp/lib/hello-time.h",
        "go/fortune/fortune.go",
        "nope.cc",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "cpp/lib/hello-time.h -> [cpp] //lib:hello-time\n\
         go/fortune/fortune.go -> [go] //fortune:fortune\n\
         nope.cc -> (no target names it)\n"
    );
    assert_eq!(
        surface_line(&stderr),
        "[targets] surface=cli cmd=owners files=3 rows=2"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[targets] sidecar built nodes=25 edges=33 ")),
        "{stderr}"
    );

    let (code, stdout, stderr) = glia(&[
        "targets",
        "owners",
        s.repo(),
        "cpp/lib/hello-time.h",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[targets] sidecar reused ")),
        "{stderr}"
    );
    let j = json(&stdout);
    assert_eq!(j[0]["file"], "cpp/lib/hello-time.h");
    let o = &j[0]["owners"][0];
    assert_eq!(o["label"], "//lib:hello-time");
    assert_eq!(o["workspace"], "cpp");
    assert_eq!(o["rule_class"], "cc_library");
    assert_eq!(o["role"], "library");
    assert_eq!(o["via"], "hdrs");
    assert_eq!(o["file"], "cpp/lib/BUILD");
    assert_eq!(o["line"], 3);
    assert_eq!(o["tier"], "fact");

    let (code, stdout, _) = glia(&["targets", "owners", s.repo(), "cpp/main/BUILD"]);
    assert_eq!(code, 1, "no owner is the absence");
    assert_eq!(stdout, "cpp/main/BUILD -> (no target names it)\n");
}

#[test]
fn deps_and_rdeps_text_and_json() {
    let s = Scratch::synced("walk");
    let (code, stdout, stderr) = glia(&[
        "targets",
        "deps",
        s.repo(),
        "//main:hello-world",
        "--depth",
        "1",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "[cpp] //lib:hello-time  cc_library  library  cpp/lib/BUILD:3  depth=1  via=deps\n\
         [cpp] //main:hello-greet  cc_library  library  cpp/main/BUILD:4  depth=1  via=deps\n\
         [cpp] //main:hello-world.cc  -  -  cpp/main/hello-world.cc:1  depth=1  via=srcs\n\
         [cpp] @rules_cc//:link_extra_lib  -  -  -  depth=1  via=deps\n"
    );
    assert_eq!(
        surface_line(&stderr),
        "[targets] surface=cli cmd=deps rows=4 absence=-"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "[targets] answer kind=deps rows=4 tests=0"),
        "{stderr}"
    );

    let (code, stdout, stderr) = glia(&[
        "targets",
        "rdeps",
        s.repo(),
        "//fortune",
        "--depth",
        "1",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let j = json(&stdout);
    assert!(j["absence"].is_null(), "{j}");
    assert_eq!(
        labels(&j["results"]),
        ["//:print_fortune", "//fortune:fortune_test"]
    );
    assert_eq!(j["results"][1]["role"], "test");
    assert_eq!(j["results"][1]["via"], "embed");

    // --workspace picks one workspace's label: cpp has no //fortune.
    let (code, stdout, stderr) = glia(&[
        "targets",
        "rdeps",
        s.repo(),
        "//fortune:fortune",
        "--workspace",
        "cpp",
        "--json",
    ]);
    assert_eq!(code, 1, "{stderr}");
    let j = json(&stdout);
    assert_eq!(j["results"], serde_json::json!([]));
    assert_eq!(j["absence"]["reason"], "unknown_symbol");
    let (code, stdout, _) = glia(&[
        "targets",
        "deps",
        s.repo(),
        "//main:hello-world",
        "--workspace",
        "cpp",
        "--depth",
        "2",
        "--json",
    ]);
    assert_eq!(code, 0);
    let j = json(&stdout);
    let rows = j["results"].as_array().expect("rows");
    assert_eq!(rows.iter().filter(|r| r["depth"] == 2).count(), 4);
    assert!(rows.iter().all(|r| r["workspace"] == "cpp"));
}

#[test]
fn an_unknown_label_or_no_edge_is_the_absence() {
    let s = Scratch::synced("absence");
    let (code, stdout, stderr) = glia(&["targets", "deps", s.repo(), "//nope:fortune"]);
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(
        stdout,
        "_(no rows)_\n\
         > FACT: no target //nope:fortune in the targets snapshot (20 rows, workspaces cpp,go)\n\
         > did you mean: //:print_fortune, //fortune:fortune, //fortune:fortune_test\n"
    );
    assert_eq!(
        surface_line(&stderr),
        "[targets] surface=cli cmd=deps rows=0 absence=unknown_symbol"
    );

    let (code, stdout, _) = glia(&["targets", "rdeps", s.repo(), "//main:hello-world", "--json"]);
    assert_eq!(code, 1);
    let j = json(&stdout);
    assert_eq!(j["absence"]["reason"], "no_edges");
    assert_eq!(j["absence"]["tier"], "fact");
}

#[test]
fn affected_by_files_text_and_json() {
    let s = Scratch::synced("affected");
    let (code, stdout, stderr) = glia(&["targets", "affected", s.repo(), "go/fortune/fortune.go"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "## targets\n\
         [go] //fortune:fortune  go_library  library  go/fortune/BUILD:3  depth=1  via=srcs\n\
         [go] //:print_fortune  go_binary  binary  go/BUILD:3  depth=2  via=deps\n\
         [go] //fortune:fortune_test  go_test  test  go/fortune/BUILD:10  depth=2  via=embed\n\
         \n\
         ## tests\n\
         [go] //fortune:fortune_test  go_test  test  go/fortune/BUILD:10  depth=2  via=embed\n"
    );
    assert_eq!(
        surface_line(&stderr),
        "[targets] surface=cli cmd=affected changed=1 rows=3 tests=1 unowned=0 caveats=0"
    );

    let (code, stdout, stderr) = glia(&[
        "targets",
        "affected",
        s.repo(),
        "cpp/lib/hello-time.cc",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let j = json(&stdout);
    assert_eq!(j["changed"], serde_json::json!(["cpp/lib/hello-time.cc"]));
    assert_eq!(labels(&j["owners"]), ["//lib:hello-time"]);
    assert_eq!(
        labels(&j["targets"]),
        ["//lib:hello-time", "//main:hello-world"]
    );
    assert_eq!(j["tests"], serde_json::json!([]));

    // A BUILD file and a new file: nothing affected (exit 1), the caveat.
    let (code, stdout, _) = glia(&[
        "targets",
        "affected",
        s.repo(),
        "go/fortune/BUILD",
        "go/new.go",
    ]);
    assert_eq!(code, 1);
    assert_eq!(
        stdout,
        "## targets\n_(none)_\n\n## tests\n_(none)_\n\n\
         unowned: go/new.go\n\
         > caveat: BUILD files changed; re-run glia targets sync for exact owners\n"
    );
}

#[test]
fn affected_with_no_files_reads_the_work_tree_against_base() {
    let s = Scratch::synced("worktree");
    s.git(&["init", "-q"]);
    s.git(&["add", "-A"]);
    s.git(&["commit", "-q", "-m", "fixture"]);
    let f = s.repo.join("go/fortune/fortune.go");
    let mut text = std::fs::read_to_string(&f).expect("read");
    text.push_str("\n// changed\n");
    std::fs::write(&f, text).expect("write");

    let (code, stdout, stderr) = glia(&["targets", "affected", s.repo(), "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let j = json(&stdout);
    assert_eq!(j["changed"], serde_json::json!(["go/fortune/fortune.go"]));
    assert_eq!(labels(&j["tests"]), ["//fortune:fortune_test"]);

    let (code, _, stderr) = glia(&["targets", "affected", s.repo(), "--base", "no-such-rev"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("error: "), "{stderr}");
}
