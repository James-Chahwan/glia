//! DC.10a: the scripted watch parity gate (R2-watch section 8). One
//! long-lived [`WatchSession`] per input class is stepped through an edit
//! sequence, and after EVERY step:
//!
//! 1. the session's `parse_errors` and `merged.pass_undo` equal a clean
//!    `generate_one` of the tree as it now is (both also ride the manifest,
//!    asserted on their own first so a failure reads as what moved);
//! 2. the session's persisted layout equals the clean build's
//!    `persist_result` byte for byte, file by file (both written at the same
//!    depth, so their recorded relative roots are equal);
//! 3. the update's node / edge rows serialise equal to the rows of the public
//!    `delta::located_delta(previous clean, clean)`, and its `nodes_*` /
//!    `edges_*` / `moves_ignored` counts equal that `GraphDelta`'s list
//!    lengths and `ignored_moves` (scoped and full deltas alike;
//!    `regions_excluded` has no public accessor and is not compared);
//! 4. the step's [`Expect`]: the groups rebuilt, the full-rebuild reason, the
//!    table entries added / modified / removed, the row kinds it must show,
//!    whether `seq` moves.
//!
//! The fixture is written into a tempdir: the source files of five
//! substrate-gap probes copied by relative path (never their `key.json`; the
//! copy asserts each source exists, so a probe edit that drops one fails
//! here loudly) plus inline files, see [`write_fixture`]. Every sequence runs
//! on a fresh fixture and a fresh session, so a failure names one class.
//! The first clean build's `pass_undo` is non-empty on purpose: the Flask
//! `GET /users` route has no client until the `client` sequence adds one
//! (LC.10a demotes it with an undo record).
//!
//! Falsifiability, checked on a scratch build when this landed: making
//! `go.mod` Low in `watch::durability` fails `go_mod` at its totals (it keeps
//! the IMPORTS / CALLS of `example.com/parity/tools` that a clean build of
//! the renamed module drops), and keeping the loaded
//! overlay across a `.glia` update fails `overlay` at its layout bytes
//! (`manifest.json` first). Dropping `overlay_fp` from the group fingerprint fails `overlay`
//! only at its [`EVERY_GROUP`] expectation: the pin reaches the one group it
//! moves through that group's post-graft parse. Dropping only the `go_mod`
//! arm of `durability` changes nothing: `go.mod` is a `project_roots::MANIFESTS`
//! name, so it stays High (`manifest`) and still re-plans.
//!
//! Stderr markers are never compared between the two sides (the session
//! prints its own stage lines). fired_on: every rebuilding step prints the
//! session's `[watch] update seq=<s> ...` line, a noop its `[watch] noop`.
//!
//! DC.10b, the random half ([`random_sequences`]): seeded [`SplitMix64`]
//! sequences of [`Op`]s on the same fixture, one session per seed, each step
//! reaching the session through `apply` of the paths it touched, the stat
//! poll, or neither yet (deferred: the next flush batches it), and checks 1-3
//! after every flush, plus the files, `full_rebuild` and groups a byte
//! snapshot of the tree implies ([`Want`]). Seeds `1..=4` x 15 steps by
//! default; `GLIA_WATCH_PARITY_SEEDS=<n>` (seeds `1..=n`) and
//! `GLIA_WATCH_PARITY_STEPS=<m>` widen it for a soak run, and
//! `GLIA_WATCH_PARITY_SEED=<s>` replays the one seed a failure names
//! (`seed=<s> step=<i> op=<name> path=<p>`; with the failing run's
//! `GLIA_WATCH_PARITY_STEPS`, since the last step never defers).
//! Falsifiability, checked on a scratch tree when this landed (default
//! seeds): `go.mod` Low in `watch::durability` fails at a `go_mod` step, a
//! stat poll that skips its directory pass at the first polled file add, and
//! a scoped delta taken with a SCIP snapshot present at the first TS edit
//! after one ([`write_snapshot`]). Each step also prints
//! `[watch-parity] seed=<s> step=<i> op=<name> path=<p> via=<route>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipRefRow, ScipSymbolRecord, scip_dir, source_hash,
    write_scip,
};
use glia_engine::delta::located_delta;
use glia_engine::persist::persist_result;
use glia_engine::watch::{WatchSession, WatchUpdate};
use glia_engine::{BuildOptions, GenerateResult, generate_one};

/// The probes the fixture copies from, relative to this crate.
const BENCH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap");

/// `(source under BENCH, destination in the fixture)`.
const COPIED: &[(&str, &str)] = &[
    ("fixtures/ts-tsconfig-paths/tsconfig.json", "web/tsconfig.json"),
    ("fixtures/ts-tsconfig-paths/src/app/core/auth.service.ts", "web/src/app/core/auth.service.ts"),
    ("fixtures/ts-tsconfig-paths/src/app/feature/page.ts", "web/src/app/feature/page.ts"),
    ("fixtures/rust-crate-path-calls/Cargo.toml", "rs/Cargo.toml"),
    ("fixtures/rust-crate-path-calls/core/Cargo.toml", "rs/core/Cargo.toml"),
    ("fixtures/rust-crate-path-calls/core/src/api.rs", "rs/core/src/api.rs"),
    ("fixtures/rust-crate-path-calls/core/src/lib.rs", "rs/core/src/lib.rs"),
    ("fixtures/rust-crate-path-calls/app/Cargo.toml", "rs/app/Cargo.toml"),
    ("fixtures/rust-crate-path-calls/app/src/main.rs", "rs/app/src/main.rs"),
    ("fixtures/xcut-grpc-grpc_calls/server/user.proto", "grpc/server/user.proto"),
    ("fixtures/xcut-grpc-grpc_calls/client/main.go", "grpc/client/main.go"),
    ("fixtures/cpp-include-paths/CMakeLists.txt", "cpp/CMakeLists.txt"),
    ("fixtures/cpp-include-paths/src/cart.cpp", "cpp/src/cart.cpp"),
    ("fixtures/cpp-include-paths/include/shop/cart.hpp", "cpp/include/shop/cart.hpp"),
    ("fixtures/cpp-include-paths/include/shop/util.hpp", "cpp/include/shop/util.hpp"),
    ("matrix/scala/cron/Jobs.scala", "scala/Jobs.scala"),
    ("matrix/scala/cron/src/main/resources/application.conf", "scala/src/main/resources/application.conf"),
];

/// The fixture's language groups, in plan order: every one rebuilds on a full
/// rebuild other than `go_modules` (DC.7b).
const EVERY_GROUP: &[&str] = &["c_cpp", "go", "manifest", "proto", "python", "rust", "scala", "typescript"];

/// The fixture's own files, beside the copies.
const INLINE: &[(&str, &str)] = &[
    ("go.mod", "module example.com/parity\n\ngo 1.21\n"),
    ("tools/gen.go", "package tools\n\nfunc Gen() int { return 1 }\n"),
    (
        "cmd/main.go",
        "package main\n\nimport \"example.com/parity/tools\"\n\nfunc main() { tools.Gen() }\n",
    ),
    ("grpc/client/go.mod", "module example.com/client\n\ngo 1.21\n"),
    ("web/package.json", "{\n  \"name\": \"web\",\n  \"dependencies\": { \"axios\": \"^1.6.0\" }\n}\n"),
    (
        "web/src/app/core/api.ts",
        "import axios from 'axios';\n\nconst GATEWAY = process.env.GATEWAY_URL;\n\nexport async function listThings() {\n  return axios.get(`${GATEWAY}/things`);\n}\n",
    ),
    (
        "api/user.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\")\ndef get_user():\n    return {\"id\": 1}\n",
    ),
    ("api/svc.py", "from api.user import get_user\n\n\ndef handle():\n    return get_user()\n"),
    ("cpp/third/tp.hpp", "#pragma once\ninline int tp_value() { return 7; }\n"),
    ("cpp/src/extra.cpp", "#include <tp.hpp>\n\nint extra_value() { return tp_value(); }\n"),
    ("scala/build.sbt", "lazy val api = project.in(file(\"api\"))\n"),
    ("scala/api/Api.scala", "package shop.api\n\nobject Api {\n  def ping(): String = \"pong\"\n}\n"),
    ("README.md", "# Parity fixture\n\nThe API serves users through `get_user`.\n"),
    ("docs/guide.md", "# Guide\n\n`get_user` returns one user.\n"),
    (".gitignore", "*.log\n"),
    (".glia/overlay.toml", "version = 1\n\n[constants]\nGATEWAY = \"/gw\"\n"),
];

fn write(repo: &Path, rel: &str, body: &str) {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

/// The fixture at `<tmp>/repo` (every sequence's root has the same name, so
/// its identity, `dir:repo`, and every NodeId agree across sequences).
fn write_fixture(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    for (src, dst) in COPIED {
        let from = Path::new(BENCH).join(src);
        let body = std::fs::read_to_string(&from)
            .unwrap_or_else(|e| panic!("fixture source {} is gone: {e}", from.display()));
        write(&repo, dst, &body);
    }
    for (rel, body) in INLINE {
        write(&repo, rel, body);
    }
    repo
}

/// Replace the one `old` in `rel` with `new`; panics when `old` is not there
/// exactly once, so a moved probe file fails the step that edits it.
fn replace(repo: &Path, rel: &str, old: &str, new: &str) {
    let text = std::fs::read_to_string(repo.join(rel)).expect("read");
    assert_eq!(text.matches(old).count(), 1, "{rel}: {old:?} once");
    write(repo, rel, &text.replacen(old, new, 1));
}

// ---- the edits -------------------------------------------------------------

fn body_edit(r: &Path) {
    replace(r, "api/svc.py", "    return get_user()\n", "    user = get_user()\n    return user\n");
}
fn body_edit_polled(r: &Path) {
    body_edit(r);
    // A stamp unlike the record's, set rather than slept on.
    let t = settled() + Duration::from_secs(60);
    std::fs::File::open(r.join("api/svc.py")).and_then(|f| f.set_modified(t)).expect("mtime");
}
fn declaration_edit(r: &Path) {
    let user = std::fs::read_to_string(r.join("api/user.py")).expect("read");
    write(r, "api/user.py", &format!("{user}\n\ndef list_users():\n    return [get_user()]\n"));
    replace(r, "api/svc.py", "import get_user\n", "import get_user, list_users\n");
    let svc = std::fs::read_to_string(r.join("api/svc.py")).expect("read");
    write(r, "api/svc.py", &format!("{svc}\n\ndef handle_all():\n    return list_users()\n"));
}
fn add_go(r: &Path) {
    write(r, "grpc/client/extra.go", "package main\n\nfunc Extra() { FetchUser(\"x\") }\n");
}
fn delete_go(r: &Path) {
    std::fs::remove_file(r.join("grpc/client/extra.go")).expect("rm");
}
fn rename_py(r: &Path) {
    std::fs::rename(r.join("api/svc.py"), r.join("api/service.py")).expect("mv");
}
fn same_stem_add(r: &Path) {
    write(r, "api/user.ts", "export function userTs(): number {\n  return 1;\n}\n");
}
const CLIENT: &str = "\nexport async function getUsers() {\n  return axios.get('/users');\n}\n";
fn client_add(r: &Path) {
    let api = std::fs::read_to_string(r.join("web/src/app/core/api.ts")).expect("read");
    write(r, "web/src/app/core/api.ts", &format!("{api}{CLIENT}"));
}
fn client_remove(r: &Path) {
    replace(r, "web/src/app/core/api.ts", CLIENT, "");
}
fn go_mod_edit(r: &Path) {
    replace(r, "go.mod", "module example.com/parity\n", "module example.com/moved\n");
}
fn tsconfig_edit(r: &Path) {
    replace(r, "web/tsconfig.json", "[\"app/core/*\"]", "[\"app/none/*\"]");
}
fn cargo_member_add(r: &Path) {
    write(r, "rs/util/Cargo.toml", "[package]\nname = \"acme-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    write(r, "rs/util/src/lib.rs", "pub fn util() -> u32 {\n    3\n}\n");
    replace(r, "rs/Cargo.toml", "members = [\"core\", \"app\"]", "members = [\"core\", \"app\", \"util\"]");
}
fn overlay_edit(r: &Path) {
    replace(r, ".glia/overlay.toml", "GATEWAY = \"/gw\"", "GATEWAY = \"/gw2\"");
}
fn proto_rpc_add(r: &Path) {
    replace(
        r,
        "grpc/server/user.proto",
        "  rpc ListUsers",
        "  rpc DeleteUser (GetUserRequest) returns (User);\n  rpc ListUsers",
    );
}
fn proto_field_edit(r: &Path) {
    replace(r, "grpc/server/user.proto", "string name = 2;", "string display_name = 2;");
}
fn markdown_edit(r: &Path) {
    let readme = std::fs::read_to_string(r.join("README.md")).expect("read");
    write(r, "README.md", &format!("{readme}\n## Handling\n\nSee `handle`.\n"));
}
fn include_root_add(r: &Path) {
    replace(r, "cpp/CMakeLists.txt", "PUBLIC include)", "PUBLIC include third)");
}
fn gitignore_docs(r: &Path) {
    write(r, ".gitignore", "*.log\ndocs/\n");
}
fn untouched(_: &Path) {}
fn conf_edit(r: &Path) {
    replace(
        r,
        "scala/src/main/resources/application.conf",
        "\"0 0 3 * * ?\"",
        "\"0 0 4 * * ?\"",
    );
}
fn sbt_member_add(r: &Path) {
    write(r, "scala/util/Util.scala", "package shop.util\n\nobject Util {\n  def twice(n: Int): Int = n * 2\n}\n");
    let sbt = std::fs::read_to_string(r.join("scala/build.sbt")).expect("read");
    write(r, "scala/build.sbt", &format!("{sbt}lazy val util = project.in(file(\"util\"))\n"));
}

// ---- the harness -----------------------------------------------------------

/// How a step reaches the session: `apply` of the named repo-relative paths,
/// or the stat poll (the sequence's fixture is settled first).
enum Via {
    Apply(&'static [&'static str]),
    Poll,
}

/// What one step must show besides parity. Table entries are exact lists.
struct Expect {
    rebuilt_exact: Option<&'static [&'static str]>,
    rebuilt_contains: &'static [&'static str],
    rebuilt_excludes: &'static [&'static str],
    full_rebuild: Option<&'static str>,
    files_added: &'static [&'static str],
    files_modified: &'static [&'static str],
    files_removed: &'static [&'static str],
    /// Node kinds the update's rows must hold.
    row_kinds: &'static [&'static str],
    /// Node row changes (`added`, `moved`, ...) the update must hold.
    row_changes: &'static [&'static str],
    /// The clean builds' `pass_undo` before and after the step differ.
    moves_pass_undo: bool,
    /// The update is a noop: `seq` unchanged and no rows.
    noop: bool,
}

const EXPECT: Expect = Expect {
    rebuilt_exact: None,
    rebuilt_contains: &[],
    rebuilt_excludes: &[],
    full_rebuild: None,
    files_added: &[],
    files_modified: &[],
    files_removed: &[],
    row_kinds: &[],
    row_changes: &[],
    moves_pass_undo: false,
    noop: false,
};

struct Step {
    name: &'static str,
    edit: fn(&Path),
    via: Via,
    expect: Expect,
}

fn json<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).expect("serialise")
}

/// Every file under `dir`, recursively, by relative path.
fn layout_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).expect("under dir").to_string_lossy().into_owned();
                out.push((rel, std::fs::read(&p).expect("read")));
            }
        }
    }
    out.sort();
    out
}

/// Checks 1 and 2 (module doc): the session's result and layout against
/// `clean`'s, both persisted under `out/{watch,clean}`.
fn assert_parity(s: &WatchSession, clean: &GenerateResult, out: &Path, at: &str) {
    let w = s.result();
    assert_eq!(w.parse_errors, clean.parse_errors, "{at}: parse_errors");
    assert_eq!(w.merged.pass_undo, clean.merged.pass_undo, "{at}: pass_undo");
    assert_eq!((w.total_nodes, w.total_edges), (clean.total_nodes, clean.total_edges), "{at}: totals");
    s.persist(&out.join("watch")).expect("persist the session");
    persist_result(clean, &out.join("clean"), "watch").expect("persist the clean build");
    let (wf, cf) = (layout_bytes(&out.join("watch")), layout_bytes(&out.join("clean")));
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert!(!wf.is_empty(), "{at}: a layout");
    assert_eq!(names(&wf), names(&cf), "{at}: layout file sets differ");
    for ((name, wb), (_, cb)) in wf.iter().zip(&cf) {
        assert!(wb == cb, "{at}: {name} differs from a clean build + persist");
    }
}

/// Check 3 (module doc): the update's rows and counts are the public full
/// delta's from `prev` to `clean`. Returns whether that delta moved anything.
fn assert_delta(u: &WatchUpdate, prev: &GenerateResult, clean: &GenerateResult, at: &str) -> bool {
    let (d, nodes, edges) = located_delta(&prev.merged, &clean.merged);
    assert_eq!(json(&u.nodes), json(&nodes), "{at}: node rows equal the full delta's");
    assert_eq!(json(&u.edges), json(&edges), "{at}: edge rows equal the full delta's");
    let c = &u.counts;
    assert_eq!(
        [c.nodes_added, c.nodes_removed, c.nodes_modified, c.nodes_moved],
        [d.added_nodes.len(), d.removed_nodes.len(), d.modified_nodes.len(), d.moved_nodes.len()],
        "{at}: node counts"
    );
    assert_eq!(
        [c.edges_added, c.edges_removed, c.edges_reconfidenced, c.moves_ignored],
        [d.added_edges.len(), d.removed_edges.len(), d.reconfidenced_edges.len(), d.ignored_moves],
        "{at}: edge counts and ignored moves"
    );
    !nodes.is_empty() || !edges.is_empty()
}

/// Checks 3 and 4 (module doc) for one update.
fn assert_rows(u: &WatchUpdate, prev: &GenerateResult, clean: &GenerateResult, step: &Step, at: &str) {
    let moved = assert_delta(u, prev, clean, at);
    let e = &step.expect;
    if e.noop {
        assert!(!moved, "{at}: a noop moves nothing");
    } else {
        assert!(moved, "{at}: the step moves the graph");
    }
    if let Some(exact) = e.rebuilt_exact {
        assert_eq!(u.groups_rebuilt, exact, "{at}: groups rebuilt");
    }
    for key in e.rebuilt_contains {
        assert!(u.groups_rebuilt.iter().any(|g| g == key), "{at}: {key} rebuilt: {:?}", u.groups_rebuilt);
    }
    for key in e.rebuilt_excludes {
        assert!(!u.groups_rebuilt.iter().any(|g| g == key), "{at}: {key} reused: {:?}", u.groups_rebuilt);
    }
    assert_eq!(u.full_rebuild.as_deref(), e.full_rebuild, "{at}: full_rebuild");
    assert_eq!(u.files.added, e.files_added, "{at}: files added");
    assert_eq!(u.files.modified, e.files_modified, "{at}: files modified");
    assert_eq!(u.files.removed, e.files_removed, "{at}: files removed");
    for kind in e.row_kinds {
        assert!(u.nodes.iter().any(|n| n.kind == *kind), "{at}: a {kind} row: {:?}", u.nodes);
    }
    for change in e.row_changes {
        assert!(u.nodes.iter().any(|n| n.change == *change), "{at}: a {change} row: {:?}", u.nodes);
    }
    if e.moves_pass_undo {
        assert_ne!(prev.merged.pass_undo, clean.merged.pass_undo, "{at}: the step moves pass_undo");
    }
}

/// An hour ago: the stamp [`settle`] gives the fixture.
fn settled() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3600)
}

/// Every file and directory under `dir` stamped [`settled`], so nothing sits
/// in the racy window when the session opens and a quiet tree polls quiet.
fn settle(dir: &Path) {
    let t = settled();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                std::fs::File::open(&p).and_then(|f| f.set_modified(t)).expect("mtime");
            }
        }
        std::fs::File::open(&d).and_then(|f| f.set_modified(t)).expect("mtime");
    }
}

/// Run `steps` on a fresh fixture and session; returns every update.
fn run(sequence: &str, steps: &[Step]) -> Vec<WatchUpdate> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = write_fixture(tmp.path());
    let root = repo.to_str().expect("utf-8 root");
    if steps.iter().any(|s| matches!(s.via, Via::Poll)) {
        settle(&repo);
    }
    let mut s = WatchSession::open(root, &BuildOptions::default()).expect("open");
    let mut prev = generate_one(root).expect("clean build");
    assert!(!prev.merged.pass_undo.is_empty(), "{sequence}: the unpaired route leaves an undo record");
    assert_parity(&s, &prev, &tmp.path().join("out").join("0-open"), &format!("{sequence}/open"));
    let mut updates = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let at = format!("{sequence}/{}", step.name);
        (step.edit)(&repo);
        let seq = s.seq();
        let u = match step.via {
            Via::Apply(paths) => {
                let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                s.apply(&paths).expect("apply")
            }
            Via::Poll => s.poll().expect("poll").expect("the poll sees the edit"),
        };
        let want_seq = if step.expect.noop { seq } else { seq + 1 };
        assert_eq!((u.seq, s.seq()), (want_seq, want_seq), "{at}: seq");
        let clean = generate_one(root).expect("clean build");
        let out = tmp.path().join("out").join(format!("{}-{}", i + 1, step.name));
        assert_parity(&s, &clean, &out, &at);
        assert_rows(&u, &prev, &clean, step, &at);
        prev = clean;
        updates.push(u);
    }
    updates
}

// ---- the sequences, one per input class -------------------------------------

const BODY: Step = Step {
    name: "body",
    edit: body_edit,
    via: Via::Apply(&["api/svc.py"]),
    expect: Expect { rebuilt_exact: Some(&["python"]), files_modified: &["api/svc.py"], ..EXPECT },
};

#[test]
fn scripted_body() {
    run("body", &[BODY]);
}

#[test]
fn scripted_declaration() {
    run(
        "declaration",
        &[Step {
            name: "declaration",
            edit: declaration_edit,
            via: Via::Apply(&["api/user.py", "api/svc.py"]),
            expect: Expect {
                rebuilt_exact: Some(&["python"]),
                files_modified: &["api/svc.py", "api/user.py"],
                ..EXPECT
            },
        }],
    );
}

#[test]
fn scripted_add_delete_rename() {
    run(
        "add_delete_rename",
        &[
            Step {
                name: "add",
                edit: add_go,
                via: Via::Apply(&["grpc/client/extra.go"]),
                expect: Expect {
                    rebuilt_exact: Some(&["go"]),
                    files_added: &["grpc/client/extra.go"],
                    ..EXPECT
                },
            },
            Step {
                name: "delete",
                edit: delete_go,
                via: Via::Apply(&["grpc/client/extra.go"]),
                expect: Expect {
                    rebuilt_exact: Some(&["go"]),
                    files_removed: &["grpc/client/extra.go"],
                    ..EXPECT
                },
            },
            Step {
                name: "rename",
                edit: rename_py,
                via: Via::Apply(&["api/svc.py", "api/service.py"]),
                expect: Expect {
                    rebuilt_exact: Some(&["python"]),
                    files_added: &["api/service.py"],
                    files_removed: &["api/svc.py"],
                    row_changes: &["moved"],
                    ..EXPECT
                },
            },
        ],
    );
}

/// LB.9b: `api/user.ts` beside `api/user.py` renames the python MODULE.
#[test]
fn scripted_same_stem() {
    run(
        "same_stem",
        &[Step {
            name: "same_stem_add",
            edit: same_stem_add,
            via: Via::Apply(&["api/user.ts"]),
            expect: Expect {
                rebuilt_contains: &["python", "typescript"],
                files_added: &["api/user.ts"],
                row_changes: &["moved"],
                ..EXPECT
            },
        }],
    );
}

/// A client of the `/users` route, then gone again: the set-dependent HTTP
/// demotion and its undo record move both ways.
#[test]
fn scripted_client() {
    let client = |name: &'static str, edit: fn(&Path)| Step {
        name,
        edit,
        via: Via::Apply(&["web/src/app/core/api.ts"]),
        expect: Expect {
            rebuilt_exact: Some(&["typescript"]),
            files_modified: &["web/src/app/core/api.ts"],
            moves_pass_undo: true,
            ..EXPECT
        },
    };
    run("client", &[client("client_add", client_add), client("client_remove", client_remove)]);
}

#[test]
fn scripted_go_mod() {
    run(
        "go_mod",
        &[Step {
            name: "module_line",
            edit: go_mod_edit,
            via: Via::Apply(&["go.mod"]),
            expect: Expect {
                rebuilt_contains: &["go"],
                full_rebuild: Some("go_modules"),
                files_modified: &["go.mod"],
                ..EXPECT
            },
        }],
    );
}

#[test]
fn scripted_tsconfig() {
    run(
        "tsconfig",
        &[Step {
            name: "paths",
            edit: tsconfig_edit,
            via: Via::Apply(&["web/tsconfig.json"]),
            expect: Expect {
                rebuilt_contains: &["typescript"],
                rebuilt_excludes: &["rust"],
                ..EXPECT
            },
        }],
    );
}

#[test]
fn scripted_cargo_member() {
    run(
        "cargo_member",
        &[Step {
            name: "member_add",
            edit: cargo_member_add,
            via: Via::Apply(&["rs/Cargo.toml", "rs/util/Cargo.toml", "rs/util/src/lib.rs"]),
            expect: Expect {
                rebuilt_contains: &["rust"],
                files_added: &["rs/util/Cargo.toml", "rs/util/src/lib.rs"],
                files_modified: &["rs/Cargo.toml"],
                ..EXPECT
            },
        }],
    );
}

/// The `[constants]` pin re-keys the env-built `${GATEWAY}/things` ENDPOINT.
#[test]
fn scripted_overlay() {
    run(
        "overlay",
        &[Step {
            name: "constants_pin",
            edit: overlay_edit,
            via: Via::Apply(&[".glia/overlay.toml"]),
            expect: Expect {
                rebuilt_exact: Some(EVERY_GROUP),
                full_rebuild: Some("overlay"),
                row_kinds: &["ENDPOINT"],
                ..EXPECT
            },
        }],
    );
}

/// An rpc moves the proto service set (every group rebuilds); a message
/// field alone does not.
#[test]
fn scripted_proto() {
    run(
        "proto",
        &[
            Step {
                name: "rpc_add",
                edit: proto_rpc_add,
                via: Via::Apply(&["grpc/server/user.proto"]),
                expect: Expect {
                    rebuilt_exact: Some(EVERY_GROUP),
                    full_rebuild: Some("proto_services"),
                    files_modified: &["grpc/server/user.proto"],
                    ..EXPECT
                },
            },
            Step {
                name: "field_edit",
                edit: proto_field_edit,
                via: Via::Apply(&["grpc/server/user.proto"]),
                expect: Expect {
                    rebuilt_contains: &["proto"],
                    files_modified: &["grpc/server/user.proto"],
                    ..EXPECT
                },
            },
        ],
    );
}

#[test]
fn scripted_markdown() {
    run(
        "markdown",
        &[Step {
            name: "readme",
            edit: markdown_edit,
            via: Via::Apply(&["README.md"]),
            expect: Expect {
                rebuilt_exact: Some(&[]),
                files_modified: &["README.md"],
                row_kinds: &["DOC_SECTION"],
                ..EXPECT
            },
        }],
    );
}

/// `cpp/third` joins the search roots, so `<tp.hpp>` binds.
#[test]
fn scripted_include_roots() {
    run(
        "include_roots",
        &[Step {
            name: "cmake_dir",
            edit: include_root_add,
            via: Via::Apply(&["cpp/CMakeLists.txt"]),
            expect: Expect { rebuilt_exact: Some(&["c_cpp"]), ..EXPECT },
        }],
    );
}

#[test]
fn scripted_gitignore() {
    run(
        "gitignore",
        &[Step {
            name: "ignore_docs",
            edit: gitignore_docs,
            via: Via::Apply(&[".gitignore"]),
            expect: Expect { rebuilt_exact: Some(&[]), files_removed: &["docs/guide.md"], ..EXPECT },
        }],
    );
}

#[test]
fn scripted_noop() {
    run(
        "noop",
        &[Step {
            name: "unchanged",
            edit: untouched,
            via: Via::Apply(&["api/svc.py"]),
            expect: Expect { rebuilt_exact: Some(&[]), noop: true, ..EXPECT },
        }],
    );
}

/// `step` through `apply` and, on a second fresh fixture, through the stat
/// poll (`edit` writes the same change): the same rows and counts.
fn poll_matches_apply(sequence: &str, step: Step, edit: fn(&Path)) {
    let polled = Step { name: "polled", edit, via: Via::Poll, expect: Expect { ..step.expect } };
    let applied = run(&format!("{sequence}_apply"), &[step]);
    let polled = run(&format!("{sequence}_poll"), &[polled]);
    for (a, p) in applied.iter().zip(&polled) {
        assert_eq!(json(&p.nodes), json(&a.nodes), "{sequence}: poll node rows equal the apply path's");
        assert_eq!(json(&p.edges), json(&a.edges), "{sequence}: poll edge rows equal the apply path's");
        assert_eq!(json(&p.counts), json(&a.counts), "{sequence}: poll counts equal the apply path's");
    }
}

/// The body edit through the stat poll: the same rows the apply path gives.
#[test]
fn scripted_poll() {
    poll_matches_apply("poll", BODY, body_edit_polled);
}

/// DG.7b: the cron expression lives in a walked HOCON conf the graft reads.
const CONF: Step = Step {
    name: "cron_expression",
    edit: conf_edit,
    via: Via::Apply(&["scala/src/main/resources/application.conf"]),
    expect: Expect {
        rebuilt_contains: &["scala"],
        files_modified: &["scala/src/main/resources/application.conf"],
        row_kinds: &["CRON_JOB"],
        ..EXPECT
    },
};

#[test]
fn scripted_conf() {
    run("conf", &[CONF]);
}

/// DG.12: a sub-project the root build.sbt declares becomes a project root.
/// `build.sbt` is no table entry: root detection reads it.
const SBT: Step = Step {
    name: "member_add",
    edit: sbt_member_add,
    via: Via::Apply(&["scala/build.sbt", "scala/util/Util.scala"]),
    expect: Expect { files_added: &["scala/util/Util.scala"], row_kinds: &["PROJECT"], ..EXPECT },
};

#[test]
fn scripted_sbt() {
    run("sbt", &[SBT]);
}

/// The two readers that landed after the stat poll (DG.7b's walked confs,
/// DG.12's build.sbt, a watched `Input::Walk` manifest) are seen by it.
#[test]
fn scripted_poll_conf() {
    poll_matches_apply("conf", CONF, conf_edit);
}

#[test]
fn scripted_poll_sbt() {
    poll_matches_apply("sbt", SBT, sbt_member_add);
}

// ---- the random sequences (DC.10b) ------------------------------------------

/// Vigna's SplitMix64: every choice a random sequence makes hangs off it (the
/// candidate lists it indexes are sorted), so a seed replays exactly with no
/// `rand`, wall clock or HashMap order in the op choice.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// An index below `n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn splitmix64_is_the_reference() {
    let mut r = SplitMix64(0);
    assert_eq!([r.next(), r.next(), r.next()], [0xE220_A839_7B1D_CDAF, 0x6E78_9E6A_A1B9_65F4, 0x06C4_5D18_8009_454F]);
}

/// One random edit. Toggles flip between the fixture's state and one other.
#[derive(Clone, Copy)]
enum Op {
    /// A statement as the first line of the last function of a python / go /
    /// ts / rust file.
    Body,
    /// A new function, and a call to it from a sibling file.
    Declaration,
    /// A new python or go file with one function (a python one sometimes in a
    /// new directory).
    AddFile,
    /// Delete a file the sequence added.
    DeleteFile,
    /// Move a file the sequence added to a new name.
    Rename,
    /// `api/user.ts` beside `api/user.py` (LB.9b), added or removed.
    SameStem,
    Markdown,
    /// `File::set_modified` only.
    Touch,
    GoMod,
    Tsconfig,
    Proto,
    Overlay,
    /// The `/users` client (the set-dependent HTTP demotion's undo record).
    Client,
    /// A `.glia/scip-snapshot/` written or removed ([`write_snapshot`]).
    Scip,
}

impl Op {
    const ALL: [Op; 14] = [
        Op::Body,
        Op::Declaration,
        Op::AddFile,
        Op::DeleteFile,
        Op::Rename,
        Op::SameStem,
        Op::Markdown,
        Op::Touch,
        Op::GoMod,
        Op::Tsconfig,
        Op::Proto,
        Op::Overlay,
        Op::Client,
        Op::Scip,
    ];

    fn name(self) -> &'static str {
        match self {
            Op::Body => "body",
            Op::Declaration => "declaration",
            Op::AddFile => "add",
            Op::DeleteFile => "delete",
            Op::Rename => "rename",
            Op::SameStem => "same_stem",
            Op::Markdown => "markdown",
            Op::Touch => "touch",
            Op::GoMod => "go_mod",
            Op::Tsconfig => "tsconfig",
            Op::Proto => "proto_rpc",
            Op::Overlay => "overlay",
            Op::Client => "client",
            Op::Scip => "scip",
        }
    }

    /// Its cards in one [`World::pick`] bag: 15 in all, one default sequence.
    fn weight(self) -> usize {
        match self {
            Op::Body => 2,
            _ => 1,
        }
    }
}

/// How a random step reaches the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    /// `apply` of every path the batch touched (absolute or relative).
    Apply,
    /// Strictly increasing mtimes on the step's paths, then `poll`.
    Poll,
    /// Mtimes as `Poll`, no update: the next flush batches the step.
    Defer,
}

/// One op as run, as a failure message names it.
struct Done {
    step: usize,
    op: Op,
    paths: Vec<String>,
}

impl Done {
    fn describe(&self) -> String {
        format!("step={} op={} path={}", self.step, self.op.name(), self.paths.join(","))
    }
}

/// `rel`'s extension (`""` for none).
fn ext(rel: &str) -> &str {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.rsplit_once('.').map_or("", |(_, e)| e)
}

/// `text` with one statement for step `i` as the first line of its last
/// function's body, by `rel`'s language.
fn in_last_fn(rel: &str, text: &str, i: usize) -> String {
    let (at, stmt) = match ext(rel) {
        "py" => {
            let (mut off, mut hit) = (0, None);
            for line in text.split_inclusive('\n') {
                let t = line.trim();
                if t.starts_with("def ") && t.ends_with(':') {
                    hit = Some((off + line.len(), line.len() - line.trim_start().len()));
                }
                off += line.len();
            }
            let (end, indent) = hit.unwrap_or_else(|| panic!("{rel}: no def"));
            (end, format!("{}_b{i} = {i}\n", " ".repeat(indent + 4)))
        }
        lang @ ("go" | "rs") => {
            let kw = if lang == "go" { "func " } else { "fn " };
            let start = text
                .match_indices(kw)
                .map(|(p, _)| p)
                .filter(|&p| p == 0 || matches!(text.as_bytes()[p - 1], b'\n' | b' '))
                .last()
                .unwrap_or_else(|| panic!("{rel}: no {kw}"));
            let brace = start + text[start..].find('{').unwrap_or_else(|| panic!("{rel}: no body"));
            let stmt = if lang == "go" { format!("\n\t_ = {i}\n") } else { format!("\n    let _b{i} = {i};\n") };
            (brace + 1, stmt)
        }
        "ts" => {
            let (mut off, mut hit) = (0, None);
            for line in text.split_inclusive('\n') {
                let t = line.trim_end();
                let first = t.trim_start().split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                let opens = t.ends_with('{') || t.ends_with("{}");
                if opens && t.contains(')') && !["if", "for", "while", "switch", "catch"].contains(&first) {
                    hit = t.rfind('{').map(|b| off + b + 1);
                }
                off += line.len();
            }
            (hit.unwrap_or_else(|| panic!("{rel}: no function")), format!("\n  const _b{i} = {i};\n"))
        }
        other => panic!("{rel}: no body edit for .{other}"),
    };
    format!("{}{stmt}{}", &text[..at], &text[at..])
}

/// Every file under `repo` but the layout `.glia/graph/`, with its bytes.
fn tree_bytes(repo: &Path) -> BTreeMap<String, Vec<u8>> {
    let skip = repo.join(".glia").join("graph");
    let mut out = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("read_dir").flatten() {
            let p = e.path();
            if p.is_dir() {
                if p != skip {
                    stack.push(p);
                }
            } else {
                let rel = p.strip_prefix(repo).expect("under repo").to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&p).expect("read"));
            }
        }
    }
    out
}

/// The walked side of a tree: every file outside `.glia/`.
fn walked(tree: &BTreeMap<String, Vec<u8>>) -> BTreeSet<String> {
    tree.keys().filter(|r| !r.starts_with(".glia/")).cloned().collect()
}

const OVERLAY: &str = ".glia/overlay.toml";
const PROTO: &str = "grpc/server/user.proto";
const SAME_STEM: &str = "api/user.ts";
const CLIENT_HEAD: &str = "\nexport async function getUsers() {";
const SCIP_FILES: [&str; 3] =
    [".glia/scip-snapshot/documents.jsonl", ".glia/scip-snapshot/meta.json", ".glia/scip-snapshot/symbols.jsonl"];

/// The language group a changed path's group must rebuild for (`None` for a
/// doc or a `.glia` input).
fn group_of(rel: &str) -> Option<&'static str> {
    match ext(rel) {
        "py" => Some("python"),
        "go" => Some("go"),
        "ts" => Some("typescript"),
        "rs" => Some("rust"),
        "proto" => Some("proto"),
        _ if rel == "go.mod" => Some("go"),
        _ if rel.ends_with("tsconfig.json") => Some("typescript"),
        _ => None,
    }
}

/// A random sequence's fixture: the tree, the generator, and the mirror of
/// the walked file set every op keeps.
struct World {
    repo: PathBuf,
    rng: SplitMix64,
    /// The fixture's own walked files.
    initial: BTreeSet<String>,
    /// The walked files the ops expect on disk.
    mirror: BTreeSet<String>,
    /// The ops [`World::pick`] has left to draw.
    bag: Vec<Op>,
}

impl World {
    /// The files the sequence added (and still has), sorted.
    fn added(&self) -> Vec<String> {
        self.mirror.difference(&self.initial).cloned().collect()
    }

    fn choose(&mut self, xs: &[String]) -> String {
        xs[self.rng.below(xs.len())].clone()
    }

    /// A weighted pick without replacement: a bag holds every op
    /// [`Op::weight`] times, and the draw is a seeded card among those that
    /// apply (delete / rename need a file the sequence added); the bag refills
    /// when none does. So a 15-step sequence runs every input class in a
    /// seeded order, where an independent weighted pick left `go_mod` out of
    /// all four default seeds.
    fn pick(&mut self) -> Op {
        let none_added = self.added().is_empty();
        let applies = |o: &Op| !(none_added && matches!(o, Op::DeleteFile | Op::Rename));
        if !self.bag.iter().any(applies) {
            self.bag.extend(Op::ALL.iter().flat_map(|&o| std::iter::repeat_n(o, o.weight())));
        }
        let open: Vec<usize> = (0..self.bag.len()).filter(|&i| applies(&self.bag[i])).collect();
        let card = open[self.rng.below(open.len())];
        self.bag.remove(card)
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.repo.join(rel)).expect("read")
    }

    fn put(&mut self, rel: &str, body: &str) {
        write(&self.repo, rel, body);
        if !rel.starts_with(".glia/") {
            self.mirror.insert(rel.to_string());
        }
    }

    fn append(&mut self, rel: &str, tail: &str) {
        let text = self.read(rel);
        self.put(rel, &format!("{text}{tail}"));
    }

    fn remove(&mut self, rel: &str) {
        std::fs::remove_file(self.repo.join(rel)).expect("rm");
        self.mirror.remove(rel);
    }

    /// `off` -> `on` in `rel`, or back when `on` is there.
    fn toggle(&mut self, rel: &str, off: &str, on: &str) -> Vec<String> {
        if self.read(rel).contains(on) {
            replace(&self.repo, rel, on, off);
        } else {
            replace(&self.repo, rel, off, on);
        }
        vec![rel.to_string()]
    }

    /// Run `op` as step `i`; returns the repo-relative paths it touched.
    fn run(&mut self, op: Op, i: usize, stamp: SystemTime) -> Vec<String> {
        match op {
            Op::Body => {
                let files: Vec<String> =
                    self.mirror.iter().filter(|r| matches!(ext(r), "py" | "go" | "ts" | "rs")).cloned().collect();
                let rel = self.choose(&files);
                let edited = in_last_fn(&rel, &self.read(&rel), i);
                self.put(&rel, &edited);
                vec![rel]
            }
            Op::Declaration if self.rng.next().is_multiple_of(2) => {
                self.append("api/user.py", &format!("\n\ndef decl_{i}():\n    return {i}\n"));
                let call = format!("\nfrom api.user import decl_{i}\n\n\ndef call_{i}():\n    return decl_{i}()\n");
                self.append("api/svc.py", &call);
                vec!["api/svc.py".into(), "api/user.py".into()]
            }
            Op::Declaration => {
                self.append("tools/gen.go", &format!("\nfunc Decl{i}() int {{ return {i} }}\n"));
                self.append("cmd/main.go", &format!("\nfunc call{i}() int {{ return tools.Decl{i}() }}\n"));
                vec!["cmd/main.go".into(), "tools/gen.go".into()]
            }
            Op::AddFile => {
                let (rel, body) = match self.rng.below(3) {
                    0 => (
                        format!("api/added_{i}.py"),
                        format!("from api.user import get_user\n\n\ndef added_{i}():\n    return get_user()\n"),
                    ),
                    1 => (format!("tools/added_{i}.go"), format!("package tools\n\nfunc Added{i}() int {{\n\treturn Gen()\n}}\n")),
                    _ => (
                        format!("pkg{i}/mod_{i}.py"),
                        format!("from api.svc import handle\n\n\ndef mod_{i}():\n    return handle()\n"),
                    ),
                };
                self.put(&rel, &body);
                vec![rel]
            }
            Op::DeleteFile => {
                let rel = self.choose(&self.added());
                self.remove(&rel);
                vec![rel]
            }
            Op::Rename => {
                let rel = self.choose(&self.added());
                let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
                let to = format!("{dir}/moved_{i}.{}", ext(&rel));
                std::fs::rename(self.repo.join(&rel), self.repo.join(&to)).expect("mv");
                self.mirror.remove(&rel);
                self.mirror.insert(to.clone());
                vec![rel, to]
            }
            Op::SameStem if self.mirror.contains(SAME_STEM) => {
                self.remove(SAME_STEM);
                vec![SAME_STEM.into()]
            }
            Op::SameStem => {
                self.put(SAME_STEM, "export function userTs(): number {\n  return 1;\n}\n");
                vec![SAME_STEM.into()]
            }
            Op::Markdown => {
                let rel = ["README.md", "docs/guide.md"][self.rng.below(2)];
                self.append(rel, &format!("\n## Step {i}\n\nSee `handle`.\n"));
                vec![rel.into()]
            }
            Op::Touch => {
                let files: Vec<String> = self.mirror.iter().cloned().collect();
                let rel = self.choose(&files);
                std::fs::File::open(self.repo.join(&rel)).and_then(|f| f.set_modified(stamp)).expect("mtime");
                vec![rel]
            }
            Op::GoMod => self.toggle("go.mod", "module example.com/parity\n", "module example.com/moved\n"),
            Op::Tsconfig => self.toggle("web/tsconfig.json", "[\"app/core/*\"]", "[\"app/none/*\"]"),
            Op::Proto => self.toggle(PROTO, "  rpc ListUsers", "  rpc DeleteUser (GetUserRequest) returns (User);\n  rpc ListUsers"),
            Op::Overlay => self.toggle(OVERLAY, "GATEWAY = \"/gw\"", "GATEWAY = \"/gw2\""),
            Op::Client => {
                let rel = "web/src/app/core/api.ts";
                let api = self.read(rel);
                match api.find(CLIENT_HEAD) {
                    // A body edit may have grown the client: cut it whole.
                    Some(at) => self.put(rel, &api[..at]),
                    None => self.put(rel, &format!("{api}{CLIENT}")),
                }
                vec![rel.into()]
            }
            Op::Scip => {
                let dir = scip_dir(&self.repo);
                if dir.exists() {
                    std::fs::remove_dir_all(&dir).expect("rm the snapshot");
                } else {
                    write_snapshot(&self.repo);
                }
                SCIP_FILES.map(String::from).to_vec()
            }
        }
    }
}

/// The random sequences' files beside [`write_fixture`]'s: two heritage
/// edges [`write_snapshot`]'s index re-stamps, one glia binds by bare name
/// (`global_unique`: HEURISTIC, Strong) and one it infers (Go's implicit
/// IMPLEMENTS: DERIVED, Medium).
const RANDOM_INLINE: &[(&str, &str)] = &[
    ("api/base.py", "class Base:\n    def run(self):\n        return 0\n"),
    ("api/child.py", "class Child(Base):\n    def run(self):\n        return 1\n"),
    (
        "tools/shape.go",
        "package tools\n\ntype Shape interface {\n\tArea() int\n}\n\ntype Square struct{}\n\nfunc (Square) Area() int {\n\treturn 4\n}\n",
    ),
];

/// A SCIP snapshot of the tree as it is (the CORRECTION's reconfidence case,
/// DC.10a's handoff). `Square` implements `Shape`, whose symbol also has a
/// def row binding `listThings` in web/src/app/core/api.ts: ambiguous, so
/// dropped, until an edit of api.ts stales that document. Then the stage
/// re-stamps glia's inferred Go IMPLEMENTS and makes it Strong: an intra edge
/// reconfidenced in a group the TS-only update did not rebuild, which a
/// scoped delta would miss (the session's `scip_dir` guard; dropping it fails
/// here). `Child` implements `Base`: glia's name-only INHERITS_FROM is
/// re-stamped at once (its EVIDENCE cell only; it is Strong already).
/// `handle` (api/svc.py) calls `get_user`, and rust `main` calls
/// `Engine::run`: FACTs glia binds too (confirmed). `handle` calls `Gen`
/// (tools/gen.go), which only the index sees (a cross edge added). Each holds
/// until an edit of one of its files stales the document.
fn write_snapshot(repo: &Path) {
    const SYMBOLS: [&str; 7] = [
        "scip-go gomod example.com/parity v0 tools/Gen().",
        "scip-go gomod example.com/parity v0 tools/Shape#",
        "scip-go gomod example.com/parity v0 tools/Square#",
        "scip-python python parity 0.1 `api.base`/Base#",
        "scip-python python parity 0.1 `api.child`/Child#",
        "scip-python python parity 0.1 `api.user`/get_user().",
        "scip-rust cargo acme-core 0.1.0 api/Engine#run().",
    ];
    /// `(symbol, the symbols it implements)`.
    const IMPLEMENTS: [(u32, u32); 2] = [(2, 1), (4, 3)];
    type Defs = &'static [(u32, &'static str, &'static str)];
    type Refs = &'static [(u32, &'static str)];
    // (path, language, definitions (symbol, line needle, name), call
    // references (symbol, line needle)), by path.
    const DOCUMENTS: [(&str, &str, Defs, Refs); 9] = [
        ("api/base.py", "python", &[(3, "class Base", "Base")], &[]),
        ("api/child.py", "python", &[(4, "class Child", "Child")], &[]),
        ("api/svc.py", "python", &[], &[(0, "get_user()"), (5, "get_user()")]),
        ("api/user.py", "python", &[(5, "def get_user(", "get_user")], &[]),
        ("rs/app/src/main.rs", "rust", &[], &[(6, "e.run()")]),
        ("rs/core/src/api.rs", "rust", &[(6, "pub fn run(", "run")], &[]),
        ("tools/gen.go", "go", &[(0, "func Gen(", "Gen")], &[]),
        ("tools/shape.go", "go", &[(1, "type Shape interface", "Shape"), (2, "type Square struct", "Square")], &[]),
        ("web/src/app/core/api.ts", "typescript", &[(1, "function listThings(", "listThings")], &[]),
    ];
    let documents: Vec<ScipDocumentRecord> = DOCUMENTS
        .iter()
        .map(|&(path, language, defs, refs)| {
            let text = std::fs::read_to_string(repo.join(path)).expect("read");
            let line = |needle: &str| {
                let at = text.lines().position(|l| l.contains(needle));
                u32::try_from(at.unwrap_or_else(|| panic!("{path}: {needle}"))).expect("a short file")
            };
            ScipDocumentRecord {
                path: path.into(),
                language: language.into(),
                source_hash: source_hash(text.as_bytes()),
                defs: defs.iter().map(|&(s, needle, name)| ScipDefRow { s, line: line(needle), name: name.into() }).collect(),
                refs: refs
                    .iter()
                    .map(|&(s, needle)| ScipRefRow { s, line: line(needle), call: true, write: false, import: false })
                    .collect(),
            }
        })
        .collect();
    let symbols: Vec<ScipSymbolRecord> = (0u32..)
        .zip(SYMBOLS)
        .map(|(id, symbol)| ScipSymbolRecord {
            id,
            symbol: symbol.into(),
            implements: IMPLEMENTS.iter().filter(|(a, _)| *a == id).map(|&(_, b)| b).collect(),
        })
        .collect();
    let meta = ScipMeta::new("scip-parity".into(), "0".into(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write the snapshot");
}

/// What one flush must show, from the tree's bytes at the last flush and
/// now: whether anything changed (else a noop: `seq` stays, no rows), the
/// table entries added / removed (the mirror) and modified (changed bytes,
/// the non-table inputs aside), the `full_rebuild` reason (`full_rebuild_reason`'s
/// precedence), and the groups: every group under an overlay or proto
/// service change, exactly the edited files' groups when every change is an
/// in-place source / doc edit, else at least the changed paths' groups.
struct Want {
    changed: bool,
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
    full_rebuild: Option<&'static str>,
    rebuilt_exact: Option<Vec<&'static str>>,
    rebuilt_contains: BTreeSet<&'static str>,
}

impl Want {
    fn of(before: &BTreeMap<String, Vec<u8>>, after: &BTreeMap<String, Vec<u8>>) -> Self {
        let changed: BTreeSet<&str> =
            before.keys().chain(after.keys()).filter(|k| before.get(*k) != after.get(*k)).map(String::as_str).collect();
        let in_both = |k: &str| before.contains_key(k) && after.contains_key(k);
        let (files_before, files_after) = (walked(before), walked(after));
        let modified = changed
            .iter()
            .filter(|k| in_both(k) && !k.starts_with(".glia/") && **k != "web/tsconfig.json")
            .map(|k| k.to_string())
            .collect();
        let full_rebuild = if changed.contains("go.mod") {
            Some("go_modules")
        } else if changed.contains(OVERLAY) {
            Some("overlay")
        } else if changed.contains(PROTO) {
            Some("proto_services")
        } else {
            None
        };
        let mut contains: BTreeSet<&'static str> = changed.iter().filter_map(|k| group_of(k)).collect();
        if changed.contains(SAME_STEM) && !in_both(SAME_STEM) {
            contains.insert("python");
        }
        let in_place = changed.iter().all(|k| in_both(k) && matches!(ext(k), "py" | "go" | "ts" | "rs" | "md"));
        let rebuilt_exact = if changed.contains(OVERLAY) || changed.contains(PROTO) {
            Some(EVERY_GROUP.to_vec())
        } else if in_place {
            Some(EVERY_GROUP.iter().copied().filter(|g| contains.contains(g)).collect())
        } else {
            None
        };
        Want {
            changed: !changed.is_empty(),
            added: files_after.difference(&files_before).cloned().collect(),
            removed: files_before.difference(&files_after).cloned().collect(),
            modified,
            full_rebuild,
            rebuilt_exact,
            rebuilt_contains: contains,
        }
    }

    fn check(&self, u: &WatchUpdate, seq: u64, prev: &GenerateResult, clean: &GenerateResult, at: &str) {
        assert_eq!(u.seq, if self.changed { seq + 1 } else { seq }, "{at}: seq");
        let moved = assert_delta(u, prev, clean, at);
        assert!(self.changed || !moved, "{at}: an unchanged tree moves nothing");
        assert_eq!(u.files.added, self.added, "{at}: files added (the mirror)");
        assert_eq!(u.files.removed, self.removed, "{at}: files removed (the mirror)");
        assert_eq!(u.files.modified, self.modified, "{at}: files modified");
        assert_eq!(u.full_rebuild.as_deref(), self.full_rebuild, "{at}: full_rebuild");
        if let Some(exact) = &self.rebuilt_exact {
            assert_eq!(&u.groups_rebuilt, exact, "{at}: groups rebuilt");
        }
        for g in &self.rebuilt_contains {
            assert!(u.groups_rebuilt.iter().any(|r| r == g), "{at}: {g} rebuilt: {:?}", u.groups_rebuilt);
        }
    }
}

/// What a run covered, by op and by route.
#[derive(Default)]
struct Tally {
    ops: BTreeMap<&'static str, usize>,
    routes: BTreeMap<Route, usize>,
    noops: usize,
}

/// One seed's sequence of `steps` ops on a fresh fixture and session.
fn random_sequence(seed: u64, steps: usize, tally: &mut Tally) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = write_fixture(tmp.path());
    for (rel, body) in RANDOM_INLINE {
        write(&repo, rel, body);
    }
    let root = repo.to_str().expect("utf-8 root");
    settle(&repo);
    // Past every settled stamp, an hour behind the clock: step `i` stamps
    // `base + i + 1` s, never a value an earlier stamp or write left.
    let base = settled();
    let mut s = WatchSession::open(root, &BuildOptions::default()).expect("open");
    let mut prev = generate_one(root).expect("clean build");
    let out = tmp.path().join("out");
    assert_parity(&s, &prev, &out.join("open"), &format!("seed={seed} open"));
    let mut last = tree_bytes(&repo);
    let initial = walked(&last);
    let mut w = World { repo: repo.clone(), rng: SplitMix64(seed), mirror: initial.clone(), initial, bag: Vec::new() };
    let mut batch: Vec<Done> = Vec::new();
    for i in 0..steps {
        let op = w.pick();
        let stamp = base + Duration::from_secs(i as u64 + 1);
        let paths = w.run(op, i, stamp);
        let r = w.rng.next();
        let route = if i + 1 < steps && r % 7 == 6 {
            Route::Defer
        } else if r.is_multiple_of(2) {
            Route::Apply
        } else {
            Route::Poll
        };
        let done = Done { step: i, op, paths };
        let mut at = format!("seed={seed} {} via={route:?}", done.describe());
        if !batch.is_empty() {
            let earlier: Vec<String> = batch.iter().map(Done::describe).collect();
            at = format!("{at} batch=[{}]", earlier.join("; "));
        }
        eprintln!("[watch-parity] {at}");
        let now = tree_bytes(&repo);
        assert_eq!(w.mirror, walked(&now), "{at}: the mirror is the tree");
        if route != Route::Apply {
            for p in &done.paths {
                if let Ok(f) = std::fs::File::open(repo.join(p)) {
                    f.set_modified(stamp).expect("mtime");
                }
            }
        }
        *tally.ops.entry(op.name()).or_default() += 1;
        *tally.routes.entry(route).or_default() += 1;
        batch.push(done);
        if route == Route::Defer {
            continue;
        }
        let want = Want::of(&last, &now);
        let seq = s.seq();
        let update = if route == Route::Apply {
            let mut named: Vec<PathBuf> = batch
                .iter()
                .flat_map(|d| &d.paths)
                .map(|p| if r & 2 == 0 { PathBuf::from(p) } else { repo.join(p) })
                .collect();
            named.sort();
            named.dedup();
            Some(s.apply(&named).unwrap_or_else(|e| panic!("{at}: apply: {e}")))
        } else {
            s.poll().unwrap_or_else(|e| panic!("{at}: poll: {e}"))
        };
        let clean = generate_one(root).expect("clean build");
        let step_out = out.join(i.to_string());
        assert_parity(&s, &clean, &step_out, &at);
        std::fs::remove_dir_all(&step_out).expect("rm the step's layouts");
        match &update {
            Some(u) => want.check(u, seq, &prev, &clean, &at),
            None => {
                assert!(!want.changed, "{at}: the poll sees the edit");
                assert_eq!(s.seq(), seq, "{at}: seq");
                let (_, nodes, edges) = located_delta(&prev.merged, &clean.merged);
                assert!(nodes.is_empty() && edges.is_empty(), "{at}: an unchanged tree builds the same graph");
            }
        }
        if !want.changed {
            tally.noops += 1;
        }
        prev = clean;
        last = now;
        batch.clear();
    }
}

/// A count from the environment, else `default`.
fn env_count(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(v) => v.trim().parse().unwrap_or_else(|_| panic!("{name}={v}: not a count")),
        Err(_) => default,
    }
}

#[test]
fn random_sequences() {
    let soak = ["GLIA_WATCH_PARITY_SEED", "GLIA_WATCH_PARITY_SEEDS", "GLIA_WATCH_PARITY_STEPS"]
        .iter()
        .any(|v| std::env::var(v).is_ok());
    let seeds: Vec<u64> = match std::env::var("GLIA_WATCH_PARITY_SEED") {
        Ok(_) => vec![env_count("GLIA_WATCH_PARITY_SEED", 0)],
        Err(_) => (1..=env_count("GLIA_WATCH_PARITY_SEEDS", 4)).collect(),
    };
    let steps = usize::try_from(env_count("GLIA_WATCH_PARITY_STEPS", 15)).expect("a step count");
    // Stamps stay behind the clock (`random_sequence`).
    assert!(steps < 3000, "GLIA_WATCH_PARITY_STEPS={steps}: at most 2999");
    let mut tally = Tally::default();
    for &seed in &seeds {
        random_sequence(seed, steps, &mut tally);
    }
    eprintln!(
        "[watch-parity] random seeds={} steps={steps} ops={:?} routes={:?} noops={}",
        seeds.len(),
        tally.ops,
        tally.routes,
        tally.noops
    );
    if !soak {
        // The default seeds reach every op and route: a table edit that
        // drops one from them fails here, not silently.
        for op in Op::ALL {
            assert!(tally.ops.contains_key(op.name()), "seeds 1..=4 never ran {}: {:?}", op.name(), tally.ops);
        }
        assert_eq!(tally.routes.len(), 3, "seeds 1..=4 take every route: {:?}", tally.routes);
    }
}
