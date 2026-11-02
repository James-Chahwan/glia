//! HT.6: `glia_engine::targets`, the build-target sidecar and its answers.
//!
//! The repo is bazelbuild/examples' cpp-tutorial/stage3 and go-tutorial/stage3
//! (engine/tests/data/targets/, Apache-2.0, see its README) as workspaces
//! `cpp` and `go`, and the targets snapshot the snapshot step writes for them
//! (the rows of snapshots/tests/data/bazel/, normalised as HT.3's
//! `targets_sync` does, written inline as HT.5's parser test writes them).

use std::path::{Path, PathBuf};

use glia_build_domain::snapshot::{
    KIND_GENERATED_FILE, KIND_RULE, KIND_SOURCE_FILE, TargetRow, TargetsMeta, WorkspaceMeta,
    write_targets,
};
use glia_engine::persist::{default_layout_dir, persist_result};
use glia_engine::targets::{
    Affected, BUILD_FILES_CAVEAT, TARGETS_GMAP, TargetHit, affected_vs_rev, load_or_build,
    sidecar_path,
};

use crate::git_fixture;
use git_fixture::GitRepo;

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/targets")
}

/// Copy `from` into `to`, recursively.
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

/// The two workspaces' sources under `root`.
fn write_sources(root: &Path) {
    copy_tree(&data().join("cpp"), &root.join("cpp"));
    copy_tree(&data().join("go"), &root.join("go"));
}

fn source(ws: &str, label: &str, file: &str) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_SOURCE_FILE);
    r.file = Some(file.to_string());
    r.line = Some(0);
    r
}

fn rule(
    ws: &str,
    label: &str,
    class: &str,
    at: (&str, u32),
    labels: &[(&str, &[&str])],
) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_RULE);
    r.rule_class = Some(class.to_string());
    r.file = Some(at.0.to_string());
    r.line = Some(at.1);
    r.testonly = class.ends_with("_test");
    for (attr, ls) in labels {
        r.labels
            .insert(attr.to_string(), ls.iter().map(|s| s.to_string()).collect());
    }
    r
}

fn generated(ws: &str, label: &str, at: (&str, u32)) -> TargetRow {
    let mut r = TargetRow::new(ws, label, KIND_GENERATED_FILE);
    r.file = Some(at.0.to_string());
    r.line = Some(at.1);
    r
}

/// cpp-tutorial/stage3 as workspace `cpp`.
fn cpp() -> Vec<TargetRow> {
    let mut time = rule(
        "cpp",
        "//lib:hello-time",
        "cc_library",
        ("cpp/lib/BUILD", 2),
        &[
            ("hdrs", &["//lib:hello-time.h"]),
            ("srcs", &["//lib:hello-time.cc"]),
        ],
    );
    time.visibility = vec!["//main:__pkg__".to_string()];
    vec![
        source("cpp", "//lib:BUILD", "cpp/lib/BUILD"),
        time,
        source("cpp", "//lib:hello-time.cc", "cpp/lib/hello-time.cc"),
        source("cpp", "//lib:hello-time.h", "cpp/lib/hello-time.h"),
        source("cpp", "//main:BUILD", "cpp/main/BUILD"),
        rule(
            "cpp",
            "//main:hello-greet",
            "cc_library",
            ("cpp/main/BUILD", 3),
            &[
                ("hdrs", &["//main:hello-greet.h"]),
                ("srcs", &["//main:hello-greet.cc"]),
            ],
        ),
        source("cpp", "//main:hello-greet.cc", "cpp/main/hello-greet.cc"),
        source("cpp", "//main:hello-greet.h", "cpp/main/hello-greet.h"),
        rule(
            "cpp",
            "//main:hello-world",
            "cc_binary",
            ("cpp/main/BUILD", 9),
            &[
                (
                    "deps",
                    &[
                        "//lib:hello-time",
                        "//main:hello-greet",
                        "@rules_cc//:link_extra_lib",
                    ],
                ),
                ("srcs", &["//main:hello-world.cc"]),
            ],
        ),
        source("cpp", "//main:hello-world.cc", "cpp/main/hello-world.cc"),
        generated("cpp", "//main:hello-world.dwp", ("cpp/main/BUILD", 9)),
        generated("cpp", "//main:hello-world.stripped", ("cpp/main/BUILD", 9)),
    ]
}

/// go-tutorial/stage3 as workspace `go`.
fn go() -> Vec<TargetRow> {
    let mut lib = rule(
        "go",
        "//fortune:fortune",
        "go_library",
        ("go/fortune/BUILD", 2),
        &[("srcs", &["//fortune:fortune.go"])],
    );
    lib.visibility = vec!["//visibility:public".to_string()];
    vec![
        source("go", "//:BUILD", "go/BUILD"),
        rule(
            "go",
            "//:print_fortune",
            "go_binary",
            ("go/BUILD", 2),
            &[
                ("deps", &["//fortune:fortune"]),
                ("srcs", &["//:print_fortune.go"]),
            ],
        ),
        source("go", "//:print_fortune.go", "go/print_fortune.go"),
        source("go", "//fortune:BUILD", "go/fortune/BUILD"),
        lib,
        source("go", "//fortune:fortune.go", "go/fortune/fortune.go"),
        rule(
            "go",
            "//fortune:fortune_test",
            "go_test",
            ("go/fortune/BUILD", 9),
            &[
                ("embed", &["//fortune:fortune"]),
                ("srcs", &["//fortune:fortune_test.go"]),
            ],
        ),
        source(
            "go",
            "//fortune:fortune_test.go",
            "go/fortune/fortune_test.go",
        ),
    ]
}

fn meta() -> TargetsMeta {
    TargetsMeta::new(
        "glia targets sync",
        vec![
            WorkspaceMeta::new("cpp", "9.3.1", "//...:*"),
            WorkspaceMeta::new("go", "7.4.0", "//...:*"),
        ],
    )
}

/// Write the snapshot of `rows` under `root`.
fn write_snapshot(root: &Path, rows: &[TargetRow]) -> String {
    write_targets(root, meta(), rows)
        .expect("write snapshot")
        .data_hash
}

/// The sources and the snapshot of both workspaces, in a temp dir.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write_sources(dir.path());
    let mut rows = cpp();
    rows.extend(go());
    write_snapshot(dir.path(), &rows);
    dir
}

fn labels(rows: &[TargetHit]) -> Vec<&str> {
    rows.iter().map(|r| r.label.as_str()).collect()
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_snapshot_is_none_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_sources(dir.path());
    assert!(load_or_build(dir.path()).expect("load").is_none());
    assert!(!sidecar_path(dir.path()).exists());
    assert!(
        affected_vs_rev(dir.path(), "HEAD")
            .expect("no snapshot needs no git")
            .is_none()
    );
}

#[test]
fn owners_of_a_header_and_a_source() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let owners = view.owners(&strings(&["cpp/lib/hello-time.h"]));
    assert_eq!(labels(&owners), ["//lib:hello-time"]);
    let o = &owners[0];
    assert_eq!(o.workspace, "cpp");
    assert_eq!(o.kind, "TARGET");
    assert_eq!(o.rule_class.as_deref(), Some("cc_library"));
    assert_eq!(o.role.as_deref(), Some("library"));
    assert_eq!(o.via, "hdrs");
    assert_eq!(o.depth, 1);
    assert_eq!(o.file.as_deref(), Some("cpp/lib/BUILD"));
    assert_eq!(o.line, Some(3), "LOCATION line 2 is 0-based");
    assert_eq!(o.tier, "fact");
    // A BUILD file and a generated output name no owner; an unknown path none.
    assert!(view.owners(&strings(&["cpp/main/BUILD"])).is_empty());
    assert!(
        view.owners(&strings(&["cpp/main/hello-world.dwp"]))
            .is_empty()
    );
    assert!(view.owners(&strings(&["nope.cc"])).is_empty());
    // Generated outputs are not repo files.
    assert!(!view.files().contains(&"cpp/main/hello-world.dwp"));
    assert!(view.files().contains(&"go/print_fortune.go"));
}

#[test]
fn affected_cpp_library_reaches_the_binary() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let a = view.affected(&strings(&["cpp/lib/hello-time.cc"]));
    assert_eq!(labels(&a.owners), ["//lib:hello-time"]);
    assert_eq!(
        labels(&a.targets),
        ["//lib:hello-time", "//main:hello-world"]
    );
    assert_eq!(a.targets[0].depth, 1);
    assert_eq!(a.targets[1].depth, 2);
    assert_eq!(a.targets[1].via, "deps");
    assert!(a.tests.is_empty());
    assert!(a.caveats.is_empty());
    assert!(a.unowned.is_empty());
}

#[test]
fn affected_go_library_reaches_its_test_through_embed() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let a = view.affected(&strings(&["go/fortune/fortune.go"]));
    assert_eq!(labels(&a.owners), ["//fortune:fortune"]);
    assert_eq!(
        labels(&a.targets),
        [
            "//fortune:fortune",
            "//:print_fortune",
            "//fortune:fortune_test"
        ],
        "by (depth, workspace, label)"
    );
    assert_eq!(labels(&a.tests), ["//fortune:fortune_test"]);
    assert_eq!(a.tests[0].via, "embed");
    assert_eq!(a.tests[0].role.as_deref(), Some("test"));
    assert!(a.targets.iter().all(|t| t.workspace == "go"));
}

#[test]
fn affected_by_a_build_file_change_carries_the_caveat() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let a: Affected = view.affected(&strings(&["go/fortune/BUILD", "go/new.go"]));
    assert_eq!(a.caveats, [BUILD_FILES_CAVEAT]);
    assert_eq!(
        BUILD_FILES_CAVEAT,
        "BUILD files changed; re-run glia targets sync for exact owners"
    );
    assert!(a.targets.is_empty(), "a BUILD file is no target's source");
    assert_eq!(a.unowned, ["go/new.go"]);
    for f in [
        "x/BUILD.bazel",
        "defs.bzl",
        "cpp/MODULE.bazel",
        "WORKSPACE.bzlmod",
    ] {
        assert_eq!(
            view.affected(&strings(&[f])).caveats,
            [BUILD_FILES_CAVEAT],
            "{f}"
        );
    }
}

#[test]
fn deps_and_rdeps_walk_the_carry_edges() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");

    let r = view.rdeps("//fortune:fortune", 1);
    assert!(r.absence.is_none());
    assert_eq!(
        labels(&r.results),
        ["//:print_fortune", "//fortune:fortune_test"]
    );
    assert_eq!(r.results[0].via, "deps");
    assert_eq!(r.results[1].via, "embed");
    assert_eq!(r.results[1].role.as_deref(), Some("test"));
    assert_eq!(r.results[1].line, Some(10));

    let d = view.deps("//main:hello-world", 1);
    assert_eq!(
        d.results
            .iter()
            .map(|h| (h.kind, h.label.as_str(), h.via.as_str()))
            .collect::<Vec<_>>(),
        [
            ("TARGET", "//lib:hello-time", "deps"),
            ("TARGET", "//main:hello-greet", "deps"),
            ("SOURCE", "//main:hello-world.cc", "srcs"),
            ("EXTERNAL", "@rules_cc//:link_extra_lib", "deps"),
        ],
        "by label bytes: `@` sorts after `/`"
    );
    let d2 = view.deps("cpp|//main:hello-world", 2);
    assert_eq!(d2.results.iter().filter(|h| h.depth == 2).count(), 4);
    assert!(d2.results.iter().all(|h| h.workspace == "cpp"));

    // `//pkg` is `//pkg:pkg`.
    assert_eq!(
        labels(&view.rdeps("//fortune", 1).results),
        ["//:print_fortune", "//fortune:fortune_test"]
    );
    // Another workspace's label of the same text is not this one.
    assert!(view.rdeps("cpp|//fortune:fortune", 1).absence.is_some());
}

#[test]
fn an_unknown_label_is_a_fact_absence() {
    let dir = repo();
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let a = view.deps("//nope:fortune", 3);
    assert!(a.results.is_empty());
    let abs = a.absence.expect("absence");
    assert_eq!(abs.tier, "fact");
    assert_eq!(abs.reason, "unknown_symbol");
    assert_eq!(
        abs.note,
        "no target //nope:fortune in the targets snapshot (20 rows, workspaces cpp,go)"
    );
    assert_eq!(abs.mechanisms, ["DEPENDS_ON", "RUNTIME_DEP", "HAS_SRC"]);
    assert_eq!(
        abs.suggestions,
        [
            "//:print_fortune",
            "//fortune:fortune",
            "//fortune:fortune_test"
        ]
    );
    // A known target nothing reaches is a no_edges absence.
    let none = view.rdeps("//main:hello-world", 3);
    assert_eq!(none.absence.expect("absence").reason, "no_edges");
}

#[test]
fn code_modules_join_a_target_to_its_sources_modules() {
    let dir = repo();
    let root = dir.path().to_str().expect("utf-8");
    let built = glia_engine::generate_one(root).expect("build");
    let view = load_or_build(dir.path()).expect("load").expect("snapshot");
    let mods = view.code_modules(&built.merged, "//main:hello-world");
    assert_eq!(
        mods.iter()
            .map(|m| (m.kind, m.file.as_deref()))
            .collect::<Vec<_>>(),
        [("MODULE", Some("cpp/main/hello-world.cc"))]
    );
    let lib = view.code_modules(&built.merged, "//lib:hello-time");
    assert_eq!(
        lib.iter().map(|m| m.file.as_deref()).collect::<Vec<_>>(),
        [Some("cpp/lib/hello-time.cc"), Some("cpp/lib/hello-time.h")]
    );
    let go = view.code_modules(&built.merged, "//fortune:fortune");
    assert_eq!(
        go.iter().map(|m| m.file.as_deref()).collect::<Vec<_>>(),
        [Some("go/fortune/fortune.go")]
    );
    assert!(view.code_modules(&built.merged, "//nope:x").is_empty());
}

#[test]
fn the_sidecar_is_reused_then_rebuilt_and_glia_build_writes_none() {
    let dir = repo();
    let root = dir.path();
    // A build + persist (what `glia build` does) leaves no sidecar.
    let built = glia_engine::generate_one(root.to_str().expect("utf-8")).expect("build");
    persist_result(&built, &default_layout_dir(root), "test").expect("persist");
    assert!(default_layout_dir(root).join("manifest.json").exists());
    assert!(
        !sidecar_path(root).exists(),
        "a build writes no {TARGETS_GMAP}"
    );

    let first = load_or_build(root).expect("load").expect("snapshot");
    assert!(!first.reused);
    assert_eq!(first.path, default_layout_dir(root).join(TARGETS_GMAP));
    assert!(first.path.exists());
    let first_bytes = std::fs::read(&first.path).expect("read sidecar");

    // A persist's orphan sweep leaves the sidecar alone.
    persist_result(&built, &default_layout_dir(root), "test").expect("persist");
    let second = load_or_build(root).expect("load").expect("snapshot");
    assert!(second.reused);
    assert_eq!(second.graph, first.graph);
    assert_eq!(second.data_hash, first.data_hash);

    // A new snapshot rebuilds it.
    let mut rows = cpp();
    rows.extend(
        go().into_iter()
            .filter(|r| r.label != "//fortune:fortune_test"),
    );
    let hash = write_snapshot(root, &rows);
    assert_ne!(hash, first.data_hash);
    let third = load_or_build(root).expect("load").expect("snapshot");
    assert!(!third.reused);
    assert_eq!(third.data_hash, hash);
    assert!(
        third
            .affected(&strings(&["go/fortune/fortune.go"]))
            .tests
            .is_empty()
    );

    // Two checkouts of one snapshot write the same bytes.
    let other = repo();
    let again = load_or_build(other.path())
        .expect("load")
        .expect("snapshot");
    assert!(!again.reused);
    assert_eq!(std::fs::read(&again.path).expect("read"), first_bytes);
}

#[test]
fn affected_vs_rev_reads_the_work_tree_against_a_rev() {
    let repo = GitRepo::init();
    write_sources(repo.root());
    let mut rows = cpp();
    rows.extend(go());
    write_snapshot(repo.root(), &rows);
    repo.commit("base");
    repo.write(
        "go/fortune/fortune.go",
        "package fortune\n\nfunc Get() string { return \"changed\" }\n",
    );
    repo.write("cpp/lib/BUILD", "# edited\n");
    let a = affected_vs_rev(repo.root(), "HEAD")
        .expect("affected")
        .expect("snapshot");
    assert_eq!(a.changed, ["cpp/lib/BUILD", "go/fortune/fortune.go"]);
    assert_eq!(labels(&a.tests), ["//fortune:fortune_test"]);
    assert_eq!(a.caveats, [BUILD_FILES_CAVEAT]);
    assert!(affected_vs_rev(repo.root(), "no-such-rev").is_err());
}
