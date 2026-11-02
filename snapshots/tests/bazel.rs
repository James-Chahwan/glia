//! HT.3: `targets_sync`, the build-target snapshot step.
//!
//! The two fixtures under `tests/data/bazel/` are `bazel query '//...:*'
//! --output=streamed_jsonproto` output for bazelbuild/examples' cpp-tutorial
//! and go-tutorial stage 3 (see that directory's NOTICE). `/WORKSPACE` in
//! them is the workspace root; each test writes a copy with it replaced by a
//! temporary workspace's absolute path. The spawn path runs a stand-in
//! `bazel` shell script (unix only), never a real bazel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glia_build_domain::snapshot::{
    KIND_GENERATED_FILE, KIND_RULE, KIND_SOURCE_FILE, META_FILE, TARGETS_FILE, TargetRow,
    read_targets, targets_dir,
};
use glia_snapshots::{TargetsOptions, targets_sync};

const CPP: &str = include_str!("data/bazel/cpp-stage3.jsonl");
const GO: &str = include_str!("data/bazel/go-stage3.jsonl");

/// A temporary repo with one bazel workspace per entry of `workspaces`
/// (each gets a MODULE.bazel), and a directory outside it for query files.
struct Repo {
    tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    fn new(workspaces: &[&str]) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        for ws in workspaces {
            let dir = root.join(ws);
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join("MODULE.bazel"), "").expect("MODULE.bazel");
        }
        std::fs::create_dir_all(&root).expect("mkdir");
        Repo { tmp, root }
    }

    /// `fixture` with `/WORKSPACE` replaced by workspace `ws`'s absolute
    /// path, saved outside the repo; returns the saved file.
    fn query_file(&self, name: &str, ws: &str, fixture: &str) -> PathBuf {
        let abs = std::fs::canonicalize(self.root.join(ws)).expect("canonical workspace");
        let text = fixture.replace("/WORKSPACE", &abs.display().to_string());
        let path = self.tmp.path().join(name);
        std::fs::write(&path, text).expect("write query file");
        path
    }

    fn sync_from(&self, ws: &str, file: &Path) -> glia_snapshots::TargetsSummary {
        targets_sync(&self.root, &from(ws, file)).expect("targets_sync")
    }

    fn rows(&self) -> Vec<TargetRow> {
        read_targets(&self.root).expect("a complete snapshot").rows
    }
}

fn from(ws: &str, file: &Path) -> TargetsOptions {
    let mut opts = TargetsOptions::default();
    opts.workspace = PathBuf::from(ws);
    opts.from = Some(file.to_path_buf());
    opts
}

fn row<'a>(rows: &'a [TargetRow], ws: &str, label: &str) -> &'a TargetRow {
    rows.iter()
        .find(|r| r.workspace == ws && r.label == label)
        .unwrap_or_else(|| panic!("no row {ws:?} {label}"))
}

fn labels(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
        .collect()
}

fn count(rows: &[TargetRow], kind: &str) -> usize {
    rows.iter().filter(|r| r.kind == kind).count()
}

#[test]
fn defaults_query_every_target_of_the_repo_root() {
    let opts = TargetsOptions::default();
    assert_eq!(opts.bazel, "bazel");
    assert_eq!(opts.workspace, PathBuf::new());
    assert_eq!(opts.expr, "//...:*");
    assert!(opts.from.is_none());
}

#[test]
fn cpp_fixture_reads_rules_source_files_and_outputs() {
    let repo = Repo::new(&["cpp"]);
    let file = repo.query_file("cpp.jsonl", "cpp", CPP);
    let summary = repo.sync_from("cpp", &file);
    assert_eq!(summary.workspace, "cpp");
    assert_eq!(summary.targets, 12);
    assert_eq!(summary.rules, 3);
    assert_eq!(summary.source_files, 7);
    assert_eq!(summary.external_labels, 0);
    assert_eq!(summary.skipped, 0);
    assert_eq!(summary.bazel_version, "from-file");

    let rows = repo.rows();
    assert_eq!(count(&rows, KIND_RULE), 3);
    assert_eq!(count(&rows, KIND_SOURCE_FILE), 7);
    assert_eq!(count(&rows, KIND_GENERATED_FILE), 2);
    let world = row(&rows, "cpp", "//main:hello-world");
    assert_eq!(world.rule_class.as_deref(), Some("cc_binary"));
    assert_eq!(world.file.as_deref(), Some("cpp/main/BUILD"));
    assert_eq!(world.line, Some(9));
    // Only what the BUILD file set: the defaulted `malloc` and the rule
    // class's own `:cc_toolchain` wiring are not edges of the target.
    assert_eq!(
        world.labels,
        labels(&[
            ("deps", &["//lib:hello-time", "//main:hello-greet"]),
            ("srcs", &["//main:hello-world.cc"]),
        ])
    );
    assert!(!world.testonly);
    let time = row(&rows, "cpp", "//lib:hello-time");
    assert_eq!(
        time.labels,
        labels(&[
            ("hdrs", &["//lib:hello-time.h"]),
            ("srcs", &["//lib:hello-time.cc"]),
        ])
    );
    assert_eq!(time.visibility, ["//main:__pkg__"]);
    let source = row(&rows, "cpp", "//main:hello-world.cc");
    assert_eq!(source.file.as_deref(), Some("cpp/main/hello-world.cc"));
    assert_eq!(source.line, Some(0));
    assert!(source.labels.is_empty() && source.rule_class.is_none());
    let stripped = row(&rows, "cpp", "//main:hello-world.stripped");
    assert_eq!(stripped.kind, KIND_GENERATED_FILE);
    assert_eq!(stripped.file.as_deref(), Some("cpp/main/BUILD"));

    // Sorted by (workspace, label), and no absolute path is written.
    let mut sorted = rows.clone();
    sorted.sort_by(|a, b| (&a.workspace, &a.label).cmp(&(&b.workspace, &b.label)));
    assert_eq!(rows, sorted);
    let jsonl = std::fs::read_to_string(targets_dir(&repo.root).join(TARGETS_FILE)).unwrap();
    assert!(!jsonl.contains(&repo.tmp.path().display().to_string()));
    assert!(!jsonl.contains("/WORKSPACE"));
}

#[test]
fn go_fixture_keeps_the_test_to_library_embed() {
    let repo = Repo::new(&["go"]);
    let file = repo.query_file("go.jsonl", "go", GO);
    let summary = repo.sync_from("go", &file);
    assert_eq!((summary.rules, summary.source_files), (3, 5));
    let rows = repo.rows();
    let test = row(&rows, "go", "//fortune:fortune_test");
    assert_eq!(test.rule_class.as_deref(), Some("go_test"));
    assert_eq!(
        test.labels,
        labels(&[
            ("embed", &["//fortune:fortune"]),
            ("srcs", &["//fortune:fortune_test.go"]),
        ])
    );
    assert!(test.testonly, "a test rule is testonly");
    assert_eq!(test.file.as_deref(), Some("go/fortune/BUILD"));
    assert_eq!(test.line, Some(9));
    let bin = row(&rows, "go", "//:print_fortune");
    assert_eq!(
        bin.labels,
        labels(&[
            ("deps", &["//fortune:fortune"]),
            ("srcs", &["//:print_fortune.go"]),
        ])
    );
    assert_eq!(bin.file.as_deref(), Some("go/BUILD"));
    let lib = row(&rows, "go", "//fortune:fortune");
    assert_eq!(lib.visibility, ["//visibility:public"]);
}

#[test]
fn two_workspaces_share_one_snapshot_and_a_resync_replaces_only_its_own() {
    let repo = Repo::new(&["cpp", "go"]);
    let cpp = repo.query_file("cpp.jsonl", "cpp", CPP);
    let go = repo.query_file("go.jsonl", "go", GO);
    repo.sync_from("go", &go);
    repo.sync_from("cpp", &cpp);
    let snap = read_targets(&repo.root).expect("snapshot");
    let ws: Vec<(&str, usize)> = snap
        .meta
        .workspaces
        .iter()
        .map(|w| (w.workspace.as_str(), w.rows))
        .collect();
    assert_eq!(ws, [("cpp", 12), ("go", 8)]);
    assert_eq!(snap.meta.rows, 20);
    assert_eq!(snap.meta.generator, "glia targets sync");
    let go_rows: Vec<TargetRow> = snap
        .rows
        .iter()
        .filter(|r| r.workspace == "go")
        .cloned()
        .collect();

    // Re-sync cpp from the first four targets only.
    let fewer: String = CPP.lines().take(4).map(|l| format!("{l}\n")).collect();
    let fewer = repo.query_file("cpp-fewer.jsonl", "cpp", &fewer);
    let summary = repo.sync_from("cpp", &fewer);
    assert_eq!(summary.targets, 4);
    let snap = read_targets(&repo.root).expect("snapshot");
    let ws: Vec<(&str, usize)> = snap
        .meta
        .workspaces
        .iter()
        .map(|w| (w.workspace.as_str(), w.rows))
        .collect();
    assert_eq!(ws, [("cpp", 4), ("go", 8)]);
    let go_after: Vec<TargetRow> = snap
        .rows
        .into_iter()
        .filter(|r| r.workspace == "go")
        .collect();
    assert_eq!(go_after, go_rows);
}

#[test]
fn snapshot_round_trips_with_meta_written_last_and_gitignored() {
    let repo = Repo::new(&[""]);
    let file = repo.query_file("go.jsonl", "", GO);
    repo.sync_from("", &file);
    let dir = targets_dir(&repo.root);
    let jsonl = std::fs::read(dir.join(TARGETS_FILE)).unwrap();
    let snap = read_targets(&repo.root).expect("snapshot");
    assert_eq!(
        snap.meta.data_hash,
        glia_code_domain::snapshots::data_hash(&[&jsonl])
    );
    let rows = repo.rows();
    assert_eq!(rows.len(), 8);
    assert_eq!(
        row(&rows, "", "//fortune:fortune").file.as_deref(),
        Some("fortune/BUILD")
    );
    assert_eq!(
        glia_build_domain::snapshot::CONTROL_DIR,
        glia_code_domain::walk_gating::CONTROL_DIR
    );
    let ignore = std::fs::read_to_string(repo.root.join(".glia/.gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "targets-snapshot/"), "{ignore}");

    // meta.json is the completion mark: rows without it are no snapshot.
    std::fs::remove_file(dir.join(META_FILE)).unwrap();
    assert!(read_targets(&repo.root).is_none());
}

#[test]
fn selects_externals_and_relative_locations_normalise() {
    let repo = Repo::new(&["w"]);
    let lines = [
        // A select(): every branch's labels; `@@//` is the main repo.
        r#"{"type":"RULE","rule":{"name":"//a:a","ruleClass":"cc_library","location":"a/BUILD:2:11","attribute":[{"name":"deps","type":"LABEL_LIST","explicitlySpecified":true,"selectorList":{"type":"LABEL_LIST","elements":[{"entries":[{"label":"//conditions:default","stringListValue":["@rules_foo//x:y","@@//b:b"]},{"label":"//:linux","stringListValue":["//c:c","//b:b"]}]}]}},{"name":"tags","type":"STRING_LIST","stringListValue":["manual","manual","b"],"explicitlySpecified":true},{"name":"actual","type":"LABEL","stringValue":"@//d:d","explicitlySpecified":true}]}}"#,
        // Kept: a package group is no target row, and a broken line counts too.
        r#"{"type":"PACKAGE_GROUP","packageGroup":{"name":"//a:friends"}}"#,
        "{not json",
        "",
        r#"{"type":"SOURCE_FILE","sourceFile":{"name":"//a:BUILD","location":"/elsewhere/a/BUILD:1:1"}}"#,
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let file = repo.query_file("w.jsonl", "w", &text);
    let summary = repo.sync_from("w", &file);
    assert_eq!(summary.targets, 2);
    assert_eq!(summary.skipped, 2);
    assert_eq!(summary.external_labels, 1);
    let rows = repo.rows();
    let a = row(&rows, "w", "//a:a");
    assert_eq!(
        a.labels,
        labels(&[
            ("actual", &["//d:d"]),
            ("deps", &["//b:b", "//c:c", "@rules_foo//x:y"]),
        ])
    );
    assert_eq!(a.tags, ["b", "manual"]);
    assert_eq!(a.file.as_deref(), Some("w/a/BUILD"));
    assert_eq!(a.line, Some(1));
    // A location outside the repo is no file.
    assert_eq!(row(&rows, "w", "//a:BUILD").file, None);
}

#[test]
fn a_dir_without_a_workspace_file_is_refused() {
    let repo = Repo::new(&[]);
    std::fs::create_dir_all(repo.root.join("plain")).unwrap();
    let mut opts = TargetsOptions::default();
    opts.workspace = PathBuf::from("plain");
    let err = targets_sync(&repo.root, &opts).expect_err("no workspace");
    assert!(err.starts_with("no bazel workspace at "), "{err}");
    assert!(err.contains("(no MODULE.bazel / WORKSPACE)"), "{err}");
    assert!(!targets_dir(&repo.root).exists());
}

#[test]
fn a_workspace_outside_the_repo_is_refused() {
    let repo = Repo::new(&[""]);
    let mut opts = TargetsOptions::default();
    opts.workspace = PathBuf::from("../elsewhere");
    let err = targets_sync(&repo.root, &opts).expect_err("outside");
    assert!(err.contains("repo-relative"), "{err}");
}

#[test]
fn a_missing_bazel_binary_is_named() {
    let repo = Repo::new(&[""]);
    let mut opts = TargetsOptions::default();
    opts.bazel = "/nonexistent/glia-ht3-no-bazel".into();
    let err = targets_sync(&repo.root, &opts).expect_err("no bazel");
    assert!(err.contains("/nonexistent/glia-ht3-no-bazel"), "{err}");
    assert!(!targets_dir(&repo.root).exists());
}

/// A stand-in `bazel`: `version` prints a build label, `query` checks its
/// arguments, prints `out` and exits `code` with `stderr`.
#[cfg(unix)]
fn fake_bazel(dir: &Path, out: &Path, code: i32, stderr: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-bazel");
    let err = dir.join("fake-bazel.stderr");
    std::fs::write(&err, stderr).unwrap();
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = version ]; then echo 'Bazelisk version: development'; echo 'Build label: 7.4.0'; exit 0; fi\n\
         [ \"$1\" = query ] || exit 9\n\
         [ \"$2\" = '//...:*' ] || exit 9\n\
         [ \"$3\" = --output=streamed_jsonproto ] || exit 9\n\
         [ \"$4\" = --keep_going ] || exit 9\n\
         [ \"$5\" = --noshow_progress ] || exit 9\n\
         [ -f MODULE.bazel ] || exit 9\n\
         cat '{}'\n\
         cat '{}' >&2\n\
         exit {code}\n",
        out.display(),
        err.display()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn spawned_query_reads_the_version_and_accepts_a_partial_result() {
    let repo = Repo::new(&["go"]);
    let out = repo.query_file("go.jsonl", "go", GO);
    let bazel = fake_bazel(
        repo.tmp.path(),
        &out,
        3,
        "ERROR: /x/BUILD:1:1: no such package\nERROR: another\nINFO: done\n",
    );
    let mut opts = TargetsOptions::default();
    opts.bazel = bazel.display().to_string();
    opts.workspace = PathBuf::from("go");
    let summary = targets_sync(&repo.root, &opts).expect("partial result accepted");
    assert_eq!(summary.bazel_version, "7.4.0");
    assert_eq!(summary.targets, 8);
    assert_eq!(summary.skipped, 2, "the ERROR lines of a partial query");
    let snap = read_targets(&repo.root).expect("snapshot");
    assert_eq!(snap.meta.workspaces[0].bazel_version, "7.4.0");
    assert_eq!(snap.meta.workspaces[0].expr, "//...:*");
}

#[cfg(unix)]
#[test]
fn an_old_bazel_without_streamed_jsonproto_says_to_use_from() {
    let repo = Repo::new(&[""]);
    let empty = repo.tmp.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let bazel = fake_bazel(
        repo.tmp.path(),
        &empty,
        2,
        "ERROR: Invalid output format 'streamed_jsonproto'. Valid values are: label, proto\n",
    );
    let mut opts = TargetsOptions::default();
    opts.bazel = bazel.display().to_string();
    let err = targets_sync(&repo.root, &opts).expect_err("old bazel");
    assert_eq!(
        err,
        "bazel 7.4.0 has no --output=streamed_jsonproto; run it with a newer bazel and pass --from <file>"
    );
}

#[cfg(unix)]
#[test]
fn a_failed_query_writes_nothing() {
    let repo = Repo::new(&[""]);
    let empty = repo.tmp.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let bazel = fake_bazel(repo.tmp.path(), &empty, 7, "ERROR: build interrupted\n");
    let mut opts = TargetsOptions::default();
    opts.bazel = bazel.display().to_string();
    let err = targets_sync(&repo.root, &opts).expect_err("failed query");
    assert!(
        err.contains("exit 7") && err.contains("build interrupted"),
        "{err}"
    );
    assert!(!targets_dir(&repo.root).exists());
}
