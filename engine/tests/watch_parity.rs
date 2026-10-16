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

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

/// Checks 3 and 4 (module doc) for one update.
fn assert_rows(u: &WatchUpdate, prev: &GenerateResult, clean: &GenerateResult, step: &Step, at: &str) {
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
    let e = &step.expect;
    if e.noop {
        assert!(nodes.is_empty() && edges.is_empty(), "{at}: a noop moves nothing");
    } else {
        assert!(!nodes.is_empty() || !edges.is_empty(), "{at}: the step moves the graph");
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
