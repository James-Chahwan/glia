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
//! (`manifest.json` first). GA.17 dropped `overlay_fp` from the group
//! fingerprint (the pin reaches the one group it moves through that group's
//! post-graft parse): putting it back fails `overlay` at its `typescript` /
//! `[]` rebuilt expectations, and every layout byte holds either way.
//! Dropping only the `go_mod`
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
//!
//! GA.12, the memory budget ([`scripted_budgeted`]): every scripted sequence
//! re-run with a 1 MB budget, which evicts every cached group and the build
//! memos at every update: checks 1-3 hold unchanged (an eviction is a cache
//! drop, never a change of bytes).
//!
//! GA.4, the body-only fast path ([`scripted_py_fast_path`]): a four-file
//! Python fixture ([`write_py_fixture`]) through a body edit, one shifting
//! every later row of its file, two body edits in one update, a body edit
//! then a surface edit, and a body edit in the importing file: checks 1-3
//! after each, and `body_only` exactly `["python"]` on the body-only steps,
//! `[]` on the surface step. The multi-language `body` step pins it too. The
//! random half gains `py_body` (a call statement in a python file's last
//! `def`), checks `body_only` is a python subset of `groups_rebuilt` on every
//! flush, and the default seeds must take the fast path at least once.
//!
//! GA.7, the Go fast path ([`scripted_go_fast_path`]): a Go module fixture
//! ([`write_go_fixture`]: a package-var, range-element and closure receiver,
//! split-file methods above an embedding struct, a gin route file, a JSON
//! handler) through body edits (`body_only` `["go"]`), a route file and a
//! JSON handler edit (refused: `[]`, rebuilt) and a surface edit (`[]`),
//! checks 1-3 after each. The random half gains `go_body` (a call statement
//! in a go file's last `func`), and `body_only` may name `go` too.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use glia_code_domain::snapshots::{
    ScipDefRow, ScipDocumentRecord, ScipMeta, ScipRefRow, ScipSymbolRecord, scip_dir, source_hash,
    write_scip,
};
use glia_engine::delta::located_delta;
use glia_engine::memory::{MemoryBudget, rss_kb};
use glia_engine::persist::persist_result;
use glia_engine::watch::{WatchSession, WatchUpdate};
use glia_engine::{BuildOptions, GenerateResult, generate_one};

/// The probes the fixture copies from, relative to this crate.
const BENCH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench/substrate-gap");

/// `(source under BENCH, destination in the fixture)`.
const COPIED: &[(&str, &str)] = &[
    (
        "fixtures/ts-tsconfig-paths/tsconfig.json",
        "web/tsconfig.json",
    ),
    (
        "fixtures/ts-tsconfig-paths/src/app/core/auth.service.ts",
        "web/src/app/core/auth.service.ts",
    ),
    (
        "fixtures/ts-tsconfig-paths/src/app/feature/page.ts",
        "web/src/app/feature/page.ts",
    ),
    ("fixtures/rust-crate-path-calls/Cargo.toml", "rs/Cargo.toml"),
    (
        "fixtures/rust-crate-path-calls/core/Cargo.toml",
        "rs/core/Cargo.toml",
    ),
    (
        "fixtures/rust-crate-path-calls/core/src/api.rs",
        "rs/core/src/api.rs",
    ),
    (
        "fixtures/rust-crate-path-calls/core/src/lib.rs",
        "rs/core/src/lib.rs",
    ),
    (
        "fixtures/rust-crate-path-calls/app/Cargo.toml",
        "rs/app/Cargo.toml",
    ),
    (
        "fixtures/rust-crate-path-calls/app/src/main.rs",
        "rs/app/src/main.rs",
    ),
    (
        "fixtures/xcut-grpc-grpc_calls/server/user.proto",
        "grpc/server/user.proto",
    ),
    (
        "fixtures/xcut-grpc-grpc_calls/client/main.go",
        "grpc/client/main.go",
    ),
    (
        "fixtures/cpp-include-paths/CMakeLists.txt",
        "cpp/CMakeLists.txt",
    ),
    (
        "fixtures/cpp-include-paths/src/cart.cpp",
        "cpp/src/cart.cpp",
    ),
    (
        "fixtures/cpp-include-paths/include/shop/cart.hpp",
        "cpp/include/shop/cart.hpp",
    ),
    (
        "fixtures/cpp-include-paths/include/shop/util.hpp",
        "cpp/include/shop/util.hpp",
    ),
    ("matrix/scala/cron/Jobs.scala", "scala/Jobs.scala"),
    (
        "matrix/scala/cron/src/main/resources/application.conf",
        "scala/src/main/resources/application.conf",
    ),
];

/// The fixture's language groups, in plan order: every one rebuilds on a
/// `proto_services` full rebuild (DC.7b); a `go_modules` or (GA.17) an
/// `overlay` one rebuilds only the groups whose fingerprints it moved.
const EVERY_GROUP: &[&str] = &[
    "c_cpp",
    "go",
    "manifest",
    "proto",
    "python",
    "rust",
    "scala",
    "typescript",
];

/// The fixture's own files, beside the copies.
const INLINE: &[(&str, &str)] = &[
    ("go.mod", "module example.com/parity\n\ngo 1.21\n"),
    (
        "tools/gen.go",
        "package tools\n\nfunc Gen() int { return 1 }\n",
    ),
    (
        "cmd/main.go",
        "package main\n\nimport \"example.com/parity/tools\"\n\nfunc main() { tools.Gen() }\n",
    ),
    (
        "grpc/client/go.mod",
        "module example.com/client\n\ngo 1.21\n",
    ),
    (
        "web/package.json",
        "{\n  \"name\": \"web\",\n  \"dependencies\": { \"axios\": \"^1.6.0\" }\n}\n",
    ),
    (
        "web/src/app/core/api.ts",
        "import axios from 'axios';\n\nconst GATEWAY = process.env.GATEWAY_URL;\n\nexport async function listThings() {\n  return axios.get(`${GATEWAY}/things`);\n}\n",
    ),
    (
        "api/user.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\")\ndef get_user():\n    return {\"id\": 1}\n",
    ),
    (
        "api/svc.py",
        "from api.user import get_user\n\n\ndef handle():\n    return get_user()\n",
    ),
    (
        "cpp/third/tp.hpp",
        "#pragma once\ninline int tp_value() { return 7; }\n",
    ),
    (
        "cpp/src/extra.cpp",
        "#include <tp.hpp>\n\nint extra_value() { return tp_value(); }\n",
    ),
    (
        "scala/build.sbt",
        "lazy val api = project.in(file(\"api\"))\n",
    ),
    (
        "scala/api/Api.scala",
        "package shop.api\n\nobject Api {\n  def ping(): String = \"pong\"\n}\n",
    ),
    (
        "README.md",
        "# Parity fixture\n\nThe API serves users through `get_user`.\n",
    ),
    ("docs/guide.md", "# Guide\n\n`get_user` returns one user.\n"),
    (".gitignore", "*.log\n"),
    (
        ".glia/overlay.toml",
        "version = 1\n\n[constants]\nGATEWAY = \"/gw\"\n",
    ),
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
    replace(
        r,
        "api/svc.py",
        "    return get_user()\n",
        "    user = get_user()\n    return user\n",
    );
}
fn body_edit_polled(r: &Path) {
    body_edit(r);
    // A stamp unlike the record's, set rather than slept on.
    let t = settled() + Duration::from_secs(60);
    std::fs::File::open(r.join("api/svc.py"))
        .and_then(|f| f.set_modified(t))
        .expect("mtime");
}
fn declaration_edit(r: &Path) {
    let user = std::fs::read_to_string(r.join("api/user.py")).expect("read");
    write(
        r,
        "api/user.py",
        &format!("{user}\n\ndef list_users():\n    return [get_user()]\n"),
    );
    replace(
        r,
        "api/svc.py",
        "import get_user\n",
        "import get_user, list_users\n",
    );
    let svc = std::fs::read_to_string(r.join("api/svc.py")).expect("read");
    write(
        r,
        "api/svc.py",
        &format!("{svc}\n\ndef handle_all():\n    return list_users()\n"),
    );
}
fn add_go(r: &Path) {
    write(
        r,
        "grpc/client/extra.go",
        "package main\n\nfunc Extra() { FetchUser(\"x\") }\n",
    );
}
fn delete_go(r: &Path) {
    std::fs::remove_file(r.join("grpc/client/extra.go")).expect("rm");
}
fn rename_py(r: &Path) {
    std::fs::rename(r.join("api/svc.py"), r.join("api/service.py")).expect("mv");
}
fn same_stem_add(r: &Path) {
    write(
        r,
        "api/user.ts",
        "export function userTs(): number {\n  return 1;\n}\n",
    );
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
    replace(
        r,
        "go.mod",
        "module example.com/parity\n",
        "module example.com/moved\n",
    );
}
fn tsconfig_edit(r: &Path) {
    replace(
        r,
        "web/tsconfig.json",
        "[\"app/core/*\"]",
        "[\"app/none/*\"]",
    );
}
fn cargo_member_add(r: &Path) {
    write(
        r,
        "rs/util/Cargo.toml",
        "[package]\nname = \"acme-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        r,
        "rs/util/src/lib.rs",
        "pub fn util() -> u32 {\n    3\n}\n",
    );
    replace(
        r,
        "rs/Cargo.toml",
        "members = [\"core\", \"app\"]",
        "members = [\"core\", \"app\", \"util\"]",
    );
}
fn overlay_edit(r: &Path) {
    replace(
        r,
        ".glia/overlay.toml",
        "GATEWAY = \"/gw\"",
        "GATEWAY = \"/gw2\"",
    );
}
/// GA.17: an `[[edge]]` stanza between two python functions (a pass-time
/// section: no parse reads it).
fn overlay_edge_add(r: &Path) {
    let p = r.join(".glia/overlay.toml");
    let mut text = std::fs::read_to_string(&p).expect("read the overlay");
    text.push_str(
        "\n[[edge]]\nfrom = \"api::user::get_user\"\nto = \"api::svc::handle\"\ncategory = \"CALLS\"\n",
    );
    std::fs::write(p, text).expect("write the overlay");
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
    replace(
        r,
        "grpc/server/user.proto",
        "string name = 2;",
        "string display_name = 2;",
    );
}
fn markdown_edit(r: &Path) {
    let readme = std::fs::read_to_string(r.join("README.md")).expect("read");
    write(
        r,
        "README.md",
        &format!("{readme}\n## Handling\n\nSee `handle`.\n"),
    );
}
fn include_root_add(r: &Path) {
    replace(
        r,
        "cpp/CMakeLists.txt",
        "PUBLIC include)",
        "PUBLIC include third)",
    );
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
fn import_lib_add(r: &Path) {
    replace(
        r,
        "api/svc.py",
        "from api.user import get_user\n",
        "import yaml\nfrom api.user import get_user\n",
    );
}
fn local_module_add(r: &Path) {
    write(r, "yaml.py", "def load():\n    return {}\n");
}
fn local_module_remove(r: &Path) {
    std::fs::remove_file(r.join("yaml.py")).expect("rm");
}
fn sbt_member_add(r: &Path) {
    write(
        r,
        "scala/util/Util.scala",
        "package shop.util\n\nobject Util {\n  def twice(n: Int): Int = n * 2\n}\n",
    );
    let sbt = std::fs::read_to_string(r.join("scala/build.sbt")).expect("read");
    write(
        r,
        "scala/build.sbt",
        &format!("{sbt}lazy val util = project.in(file(\"util\"))\n"),
    );
}

// GA.21: the wrapper memos. `api` is an inferred HTTP wrapper from the
// seed on; `send` becomes one at `wrapper_create`, in its own file only.
const WRAP_HTTP: &str = "export async function api(path: string) {\n  return fetch(`/api${path}`);\n}\n\nexport async function send(path: string) {\n  return path;\n}\n";
const WRAP_VERBS: &str = "import { api, send } from \"./http\";\n\nexport function getJson(path: string) {\n  return api(path);\n}\n\nexport function postJson(path: string) {\n  return send(path);\n}\n";
const WRAP_USERS: &str = "import { getJson, postJson } from \"../lib/verbs\";\n\nexport function listUsers() {\n  return getJson(\"/users\");\n}\n\nexport function createUser() {\n  return postJson(\"/users\");\n}\n";
const WRAP_API_SERVICE: &str = "import axios from \"axios\";\n\nexport class APIService {\n  private axiosInstance = axios.create({});\n\n  get(url: string) {\n    return this.axiosInstance.get(url);\n  }\n}\n";
const WRAP_BASE_SERVICE: &str = "import { APIService } from \"./api.service\";\n\nexport class BaseService extends APIService {}\n";
const WRAP_VIEW_SERVICE: &str = "import { BaseService } from \"./base.service\";\n\nexport class ViewService extends BaseService {\n  async fetchView() {\n    return this.get(\"/api/views/\");\n  }\n}\n";
fn wrappers_seed(r: &Path) {
    write(r, "web/src/lib/http.ts", WRAP_HTTP);
    write(r, "web/src/lib/verbs.ts", WRAP_VERBS);
    write(r, "web/src/features/users.ts", WRAP_USERS);
    write(r, "web/src/services/api.service.ts", WRAP_API_SERVICE);
    write(r, "web/src/services/base.service.ts", WRAP_BASE_SERVICE);
    write(r, "web/src/services/view.service.ts", WRAP_VIEW_SERVICE);
}
/// `send` becomes a direct wrapper: the unchanged `verbs.ts` forwards it
/// (`postJson`, a new depth-2 frontier) and the unchanged `users.ts` holds
/// its site (a new stanza set).
fn wrapper_create(r: &Path) {
    replace(
        r,
        "web/src/lib/http.ts",
        "  return path;\n",
        "  return fetch(`/api${path}`, { method: \"POST\" });\n",
    );
}
fn wrapper_site_add(r: &Path) {
    let users = std::fs::read_to_string(r.join("web/src/features/users.ts")).expect("read");
    write(
        r,
        "web/src/features/users.ts",
        &format!("{users}\nexport function listTeams() {{\n  return getJson(\"/teams\");\n}}\n"),
    );
}
/// The middle class drops its base: the unchanged `view.service.ts` keeps
/// its text and the stanza set, only its heritage scope goes.
fn heritage_base_remove(r: &Path) {
    replace(
        r,
        "web/src/services/base.service.ts",
        "export class BaseService extends APIService {}\n",
        "export class BaseService {}\n",
    );
}
/// A same-stem `.js` beside `http.ts` renames its MODULE (LB.9b / LB.13):
/// the unchanged file's wrapper key must miss.
fn wrapper_module_move(r: &Path) {
    write(r, "web/src/lib/http.js", "export const unused = 1;\n");
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
    /// GA.4: the update's `body_only`, exactly (unchecked under a budget,
    /// whose evictions leave no cached group to re-stitch).
    body_only: Option<&'static [&'static str]>,
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
    body_only: None,
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
                let rel = p
                    .strip_prefix(dir)
                    .expect("under dir")
                    .to_string_lossy()
                    .into_owned();
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
    assert_eq!(
        w.merged.pass_undo, clean.merged.pass_undo,
        "{at}: pass_undo"
    );
    assert_eq!(
        (w.total_nodes, w.total_edges),
        (clean.total_nodes, clean.total_edges),
        "{at}: totals"
    );
    s.persist(&out.join("watch")).expect("persist the session");
    persist_result(clean, &out.join("clean"), "watch").expect("persist the clean build");
    let (wf, cf) = (
        layout_bytes(&out.join("watch")),
        layout_bytes(&out.join("clean")),
    );
    let names = |fs: &[(String, Vec<u8>)]| fs.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert!(!wf.is_empty(), "{at}: a layout");
    assert_eq!(names(&wf), names(&cf), "{at}: layout file sets differ");
    for ((name, wb), (_, cb)) in wf.iter().zip(&cf) {
        assert!(
            wb == cb,
            "{at}: {name} differs from a clean build + persist"
        );
    }
}

/// Check 3 (module doc): the update's rows and counts are the public full
/// delta's from `prev` to `clean`. Returns whether that delta moved anything.
fn assert_delta(u: &WatchUpdate, prev: &GenerateResult, clean: &GenerateResult, at: &str) -> bool {
    let (d, nodes, edges) = located_delta(&prev.merged, &clean.merged);
    assert_eq!(
        json(&u.nodes),
        json(&nodes),
        "{at}: node rows equal the full delta's"
    );
    assert_eq!(
        json(&u.edges),
        json(&edges),
        "{at}: edge rows equal the full delta's"
    );
    let c = &u.counts;
    assert_eq!(
        [
            c.nodes_added,
            c.nodes_removed,
            c.nodes_modified,
            c.nodes_moved
        ],
        [
            d.added_nodes.len(),
            d.removed_nodes.len(),
            d.modified_nodes.len(),
            d.moved_nodes.len()
        ],
        "{at}: node counts"
    );
    assert_eq!(
        [
            c.edges_added,
            c.edges_removed,
            c.edges_reconfidenced,
            c.edges_restamped,
            c.moves_ignored
        ],
        [
            d.added_edges.len(),
            d.removed_edges.len(),
            d.reconfidenced_edges.len(),
            d.restamped_edges.len(),
            d.ignored_moves
        ],
        "{at}: edge counts and ignored moves"
    );
    !nodes.is_empty() || !edges.is_empty()
}

/// Checks 3 and 4 (module doc) for one update.
fn assert_rows(
    u: &WatchUpdate,
    prev: &GenerateResult,
    clean: &GenerateResult,
    step: &Step,
    budgeted: bool,
    at: &str,
) {
    let moved = assert_delta(u, prev, clean, at);
    let e = &step.expect;
    if e.noop {
        assert!(!moved, "{at}: a noop moves nothing");
    } else {
        assert!(moved, "{at}: the step moves the graph");
    }
    // A budgeted run's evictions decide what rebuilds ([`run_with`]); a
    // group the step must rebuild still does.
    if let Some(exact) = e.rebuilt_exact.filter(|_| !budgeted) {
        assert_eq!(u.groups_rebuilt, exact, "{at}: groups rebuilt");
    }
    if let Some(exact) = e.body_only.filter(|_| !budgeted) {
        assert_eq!(u.body_only, exact, "{at}: body_only");
    }
    // GA.4: the fast path's groups are rebuilt groups.
    for key in &u.body_only {
        assert!(
            u.groups_rebuilt.contains(key),
            "{at}: body_only {key} is in groups_rebuilt: {:?}",
            u.groups_rebuilt
        );
    }
    for key in e.rebuilt_contains {
        assert!(
            u.groups_rebuilt.iter().any(|g| g == key),
            "{at}: {key} rebuilt: {:?}",
            u.groups_rebuilt
        );
    }
    for key in e.rebuilt_excludes.iter().filter(|_| !budgeted) {
        assert!(
            !u.groups_rebuilt.iter().any(|g| g == key),
            "{at}: {key} reused: {:?}",
            u.groups_rebuilt
        );
    }
    assert_eq!(
        u.full_rebuild.as_deref(),
        e.full_rebuild,
        "{at}: full_rebuild"
    );
    assert_eq!(u.files.added, e.files_added, "{at}: files added");
    assert_eq!(u.files.modified, e.files_modified, "{at}: files modified");
    assert_eq!(u.files.removed, e.files_removed, "{at}: files removed");
    for kind in e.row_kinds {
        assert!(
            u.nodes.iter().any(|n| n.kind == *kind),
            "{at}: a {kind} row: {:?}",
            u.nodes
        );
    }
    for change in e.row_changes {
        assert!(
            u.nodes.iter().any(|n| n.change == *change),
            "{at}: a {change} row: {:?}",
            u.nodes
        );
    }
    if e.moves_pass_undo {
        assert_ne!(
            prev.merged.pass_undo, clean.merged.pass_undo,
            "{at}: the step moves pass_undo"
        );
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
                std::fs::File::open(&p)
                    .and_then(|f| f.set_modified(t))
                    .expect("mtime");
            }
        }
        std::fs::File::open(&d)
            .and_then(|f| f.set_modified(t))
            .expect("mtime");
    }
}

/// Run `steps` on a fresh fixture and session; returns every update.
fn run(sequence: &str, steps: &[Step]) -> Vec<WatchUpdate> {
    run_with(sequence, steps, None)
}

/// A scripted sequence's fixture: the tree it writes (its root returned),
/// and whether its first clean build must hold a pass undo record.
#[derive(Clone, Copy)]
struct Fixture {
    write: fn(&Path) -> PathBuf,
    undo: bool,
}

/// The multi-language parity fixture ([`write_fixture`]).
const MAIN: Fixture = Fixture {
    write: write_fixture,
    undo: true,
};

/// GA.4's four-file Python fixture ([`write_py_fixture`]).
const PY: Fixture = Fixture {
    write: write_py_fixture,
    undo: false,
};

/// GA.7's Go fixture ([`write_go_fixture`]).
const GO: Fixture = Fixture {
    write: write_go_fixture,
    undo: false,
};

/// [`run`], the session's memory budget set to `budget` right after open
/// when one is given (GA.12). A budgeted run checks parity, rows and files
/// exactly as [`run`] does, but not which groups rebuilt: the budget's
/// evictions decide that (with a 1 MB budget every update rebuilds every
/// plan group, checked when RSS is readable, and leaves no cached group or
/// memo behind).
fn run_with(sequence: &str, steps: &[Step], budget: Option<MemoryBudget>) -> Vec<WatchUpdate> {
    run_on(MAIN, sequence, steps, budget)
}

/// [`run_with`] on `fixture`.
fn run_on(
    fixture: Fixture,
    sequence: &str,
    steps: &[Step],
    budget: Option<MemoryBudget>,
) -> Vec<WatchUpdate> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = (fixture.write)(tmp.path());
    let root = repo.to_str().expect("utf-8 root");
    if steps.iter().any(|s| matches!(s.via, Via::Poll)) {
        settle(&repo);
    }
    let mut s = WatchSession::open(root, &BuildOptions::default()).expect("open");
    if let Some(b) = budget {
        s.set_memory_budget(b);
    }
    let evicts_all = budget == Some(MemoryBudget::mb(1)) && rss_kb().is_some();
    let mut prev = generate_one(root).expect("clean build");
    assert!(
        !fixture.undo || !prev.merged.pass_undo.is_empty(),
        "{sequence}: the unpaired route leaves an undo record"
    );
    assert_parity(
        &s,
        &prev,
        &tmp.path().join("out").join("0-open"),
        &format!("{sequence}/open"),
    );
    let mut updates = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let at = format!("{sequence}/{}", step.name);
        (step.edit)(&repo);
        let seq = s.seq();
        let rank_before = s.rank_cache();
        let u = match step.via {
            Via::Apply(paths) => {
                let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                s.apply(&paths).expect("apply")
            }
            Via::Poll => s.poll().expect("poll").expect("the poll sees the edit"),
        };
        let want_seq = if step.expect.noop { seq } else { seq + 1 };
        assert_eq!((u.seq, s.seq()), (want_seq, want_seq), "{at}: seq");
        // EA.7: an update that changed the graph replaces the ranking cache
        // (a fresh Arc, no index of the old graph); a noop keeps the same one.
        assert_eq!(
            Arc::ptr_eq(&rank_before, &s.rank_cache()),
            step.expect.noop,
            "{at}: the rank cache is kept exactly when the update is a noop"
        );
        let clean = generate_one(root).expect("clean build");
        let out = tmp
            .path()
            .join("out")
            .join(format!("{}-{}", i + 1, step.name));
        assert_parity(&s, &clean, &out, &at);
        assert_rows(&u, &prev, &clean, step, budget.is_some(), &at);
        if evicts_all {
            let m = s.memory();
            assert_eq!(
                (m.cached_groups, m.memo_paths),
                (0, 0),
                "{at}: a 1 MB budget leaves no cache: {m:?}"
            );
            if !step.expect.noop {
                assert_eq!(
                    u.groups_rebuilt.len(),
                    m.groups,
                    "{at}: every group rebuilds"
                );
            }
        }
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
    expect: Expect {
        rebuilt_exact: Some(&["python"]),
        body_only: Some(&["python"]),
        files_modified: &["api/svc.py"],
        ..EXPECT
    },
};

#[test]
fn scripted_body() {
    run("body", &[BODY]);
}

const SEQ_DECLARATION: &[Step] = &[Step {
    name: "declaration",
    edit: declaration_edit,
    via: Via::Apply(&["api/user.py", "api/svc.py"]),
    expect: Expect {
        rebuilt_exact: Some(&["python"]),
        files_modified: &["api/svc.py", "api/user.py"],
        ..EXPECT
    },
}];

#[test]
fn scripted_declaration() {
    run("declaration", SEQ_DECLARATION);
}

const SEQ_ADD_DELETE_RENAME: &[Step] = &[
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
];

#[test]
fn scripted_add_delete_rename() {
    run("add_delete_rename", SEQ_ADD_DELETE_RENAME);
}

/// LB.9b: `api/user.ts` beside `api/user.py` renames the python MODULE.
const SEQ_SAME_STEM: &[Step] = &[Step {
    name: "same_stem_add",
    edit: same_stem_add,
    via: Via::Apply(&["api/user.ts"]),
    expect: Expect {
        rebuilt_contains: &["python", "typescript"],
        files_added: &["api/user.ts"],
        row_changes: &["moved"],
        ..EXPECT
    },
}];

#[test]
fn scripted_same_stem() {
    run("same_stem", SEQ_SAME_STEM);
}

/// One step of [`SEQ_CLIENT`].
const fn client_step(name: &'static str, edit: fn(&Path)) -> Step {
    Step {
        name,
        edit,
        via: Via::Apply(&["web/src/app/core/api.ts"]),
        expect: Expect {
            rebuilt_exact: Some(&["typescript"]),
            files_modified: &["web/src/app/core/api.ts"],
            moves_pass_undo: true,
            ..EXPECT
        },
    }
}

/// A client of the `/users` route, then gone again: the set-dependent HTTP
/// demotion and its undo record move both ways.
const SEQ_CLIENT: &[Step] = &[
    client_step("client_add", client_add),
    client_step("client_remove", client_remove),
];

#[test]
fn scripted_client() {
    run("client", SEQ_CLIENT);
}

const SEQ_GO_MOD: &[Step] = &[Step {
    name: "module_line",
    edit: go_mod_edit,
    via: Via::Apply(&["go.mod"]),
    expect: Expect {
        rebuilt_contains: &["go"],
        full_rebuild: Some("go_modules"),
        files_modified: &["go.mod"],
        ..EXPECT
    },
}];

#[test]
fn scripted_go_mod() {
    run("go_mod", SEQ_GO_MOD);
}

const SEQ_TSCONFIG: &[Step] = &[Step {
    name: "paths",
    edit: tsconfig_edit,
    via: Via::Apply(&["web/tsconfig.json"]),
    expect: Expect {
        rebuilt_contains: &["typescript"],
        rebuilt_excludes: &["rust"],
        ..EXPECT
    },
}];

#[test]
fn scripted_tsconfig() {
    run("tsconfig", SEQ_TSCONFIG);
}

const SEQ_CARGO_MEMBER: &[Step] = &[Step {
    name: "member_add",
    edit: cargo_member_add,
    via: Via::Apply(&["rs/Cargo.toml", "rs/util/Cargo.toml", "rs/util/src/lib.rs"]),
    expect: Expect {
        rebuilt_contains: &["rust"],
        files_added: &["rs/util/Cargo.toml", "rs/util/src/lib.rs"],
        files_modified: &["rs/Cargo.toml"],
        ..EXPECT
    },
}];

#[test]
fn scripted_cargo_member() {
    run("cargo_member", SEQ_CARGO_MEMBER);
}

/// GA.17: an overlay edit rebuilds only the groups whose post-graft parses
/// it moves, every other group reused (`[groups] .. reused=` > 0). The
/// `[constants]` pin re-keys the env-built `${GATEWAY}/things` ENDPOINT
/// through `web/src/app/core/api.ts`'s post-graft parse, so only
/// `typescript` rebuilds; an `[[edge]]` stanza is read only by the pass
/// tail (the overlay edge stage), so no group rebuilds and the update's
/// edge rows come from the passes alone. Both stay `full_rebuild=overlay`
/// (the full delta).
const SEQ_OVERLAY: &[Step] = &[
    Step {
        name: "constants_pin",
        edit: overlay_edit,
        via: Via::Apply(&[".glia/overlay.toml"]),
        expect: Expect {
            rebuilt_exact: Some(&["typescript"]),
            full_rebuild: Some("overlay"),
            row_kinds: &["ENDPOINT"],
            ..EXPECT
        },
    },
    Step {
        name: "edge_add",
        edit: overlay_edge_add,
        via: Via::Apply(&[".glia/overlay.toml"]),
        expect: Expect {
            rebuilt_exact: Some(&[]),
            full_rebuild: Some("overlay"),
            ..EXPECT
        },
    },
];

#[test]
fn scripted_overlay() {
    run("overlay", SEQ_OVERLAY);
}

/// An rpc moves the proto service set (every group rebuilds); a message
/// field alone does not.
const SEQ_PROTO: &[Step] = &[
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
];

#[test]
fn scripted_proto() {
    run("proto", SEQ_PROTO);
}

const SEQ_MARKDOWN: &[Step] = &[Step {
    name: "readme",
    edit: markdown_edit,
    via: Via::Apply(&["README.md"]),
    expect: Expect {
        rebuilt_exact: Some(&[]),
        files_modified: &["README.md"],
        row_kinds: &["DOC_SECTION"],
        ..EXPECT
    },
}];

#[test]
fn scripted_markdown() {
    run("markdown", SEQ_MARKDOWN);
}

/// `cpp/third` joins the search roots, so `<tp.hpp>` binds.
const SEQ_INCLUDE_ROOTS: &[Step] = &[Step {
    name: "cmake_dir",
    edit: include_root_add,
    via: Via::Apply(&["cpp/CMakeLists.txt"]),
    expect: Expect {
        rebuilt_exact: Some(&["c_cpp"]),
        ..EXPECT
    },
}];

#[test]
fn scripted_include_roots() {
    run("include_roots", SEQ_INCLUDE_ROOTS);
}

const SEQ_GITIGNORE: &[Step] = &[Step {
    name: "ignore_docs",
    edit: gitignore_docs,
    via: Via::Apply(&[".gitignore"]),
    expect: Expect {
        rebuilt_exact: Some(&[]),
        files_removed: &["docs/guide.md"],
        ..EXPECT
    },
}];

#[test]
fn scripted_gitignore() {
    run("gitignore", SEQ_GITIGNORE);
}

/// GA.20: the A16.4 IMPORTS filter's memo. `api/svc.py` imports `yaml`, then
/// a new `yaml.py` makes `yaml` an in-repo module and the file is no longer
/// edited: its cell must drop the name all the same (the memo's key holds the
/// repo's index fingerprint, not only the file's imports), and take it back
/// when the module goes. Falsifiable: a key without the index replays the
/// stale cell at `local_module_add` (layout bytes).
const SEQ_IMPORTS_MEMO: &[Step] = &[
    Step {
        name: "import_lib_add",
        edit: import_lib_add,
        via: Via::Apply(&["api/svc.py"]),
        expect: Expect {
            rebuilt_exact: Some(&["python"]),
            files_modified: &["api/svc.py"],
            ..EXPECT
        },
    },
    Step {
        name: "local_module_add",
        edit: local_module_add,
        via: Via::Apply(&["yaml.py"]),
        expect: Expect {
            rebuilt_contains: &["python"],
            files_added: &["yaml.py"],
            ..EXPECT
        },
    },
    Step {
        name: "body_after",
        edit: body_edit,
        via: Via::Apply(&["api/svc.py"]),
        expect: Expect {
            rebuilt_exact: Some(&["python"]),
            files_modified: &["api/svc.py"],
            ..EXPECT
        },
    },
    Step {
        name: "local_module_remove",
        edit: local_module_remove,
        via: Via::Apply(&["yaml.py"]),
        expect: Expect {
            rebuilt_contains: &["python"],
            files_removed: &["yaml.py"],
            ..EXPECT
        },
    },
];

#[test]
fn scripted_imports_memo() {
    run("imports_memo", SEQ_IMPORTS_MEMO);
}

const SEQ_NOOP: &[Step] = &[Step {
    name: "unchanged",
    edit: untouched,
    via: Via::Apply(&["api/svc.py"]),
    expect: Expect {
        rebuilt_exact: Some(&[]),
        noop: true,
        ..EXPECT
    },
}];

#[test]
fn scripted_noop() {
    run("noop", SEQ_NOOP);
}

/// `step` through `apply` and, on a second fresh fixture, through the stat
/// poll (`edit` writes the same change): the same rows and counts.
fn poll_matches_apply(sequence: &str, step: Step, edit: fn(&Path)) {
    let polled = Step {
        name: "polled",
        edit,
        via: Via::Poll,
        expect: Expect { ..step.expect },
    };
    let applied = run(&format!("{sequence}_apply"), &[step]);
    let polled = run(&format!("{sequence}_poll"), &[polled]);
    for (a, p) in applied.iter().zip(&polled) {
        assert_eq!(
            json(&p.nodes),
            json(&a.nodes),
            "{sequence}: poll node rows equal the apply path's"
        );
        assert_eq!(
            json(&p.edges),
            json(&a.edges),
            "{sequence}: poll edge rows equal the apply path's"
        );
        assert_eq!(
            json(&p.counts),
            json(&a.counts),
            "{sequence}: poll counts equal the apply path's"
        );
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
    expect: Expect {
        files_added: &["scala/util/Util.scala"],
        row_kinds: &["PROJECT"],
        ..EXPECT
    },
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

/// GA.12: every scripted sequence again with a 1 MB memory budget, below any
/// process, so each update evicts every cached group and the build memos:
/// the eviction is a cache drop, so every step still equals a clean build
/// byte for byte, with the same rows and files ([`run_with`]).
/// GA.21: the wrapper stages' per-file memos (`build::memo` WrapperMemo /
/// ScanMemo). Each step leaves an unchanged file whose stale replay would
/// differ from a clean build; falsified by dropping one key part (the
/// frontier fingerprint fails `wrapper_create` at `postJson`, the stanza
/// fingerprint at `users.ts`' POST site, the content hash at
/// `wrapper_site_add`, the heritage-scope fingerprint at
/// `heritage_base_remove`).
const SEQ_WRAPPERS: &[Step] = &[
    Step {
        name: "wrappers_seed",
        edit: wrappers_seed,
        via: Via::Apply(&[
            "web/src/features/users.ts",
            "web/src/lib/http.ts",
            "web/src/lib/verbs.ts",
            "web/src/services/api.service.ts",
            "web/src/services/base.service.ts",
            "web/src/services/view.service.ts",
        ]),
        expect: Expect {
            rebuilt_contains: &["typescript"],
            files_added: &[
                "web/src/features/users.ts",
                "web/src/lib/http.ts",
                "web/src/lib/verbs.ts",
                "web/src/services/api.service.ts",
                "web/src/services/base.service.ts",
                "web/src/services/view.service.ts",
            ],
            row_kinds: &["ENDPOINT"],
            ..EXPECT
        },
    },
    Step {
        name: "wrapper_create",
        edit: wrapper_create,
        via: Via::Apply(&["web/src/lib/http.ts"]),
        expect: Expect {
            rebuilt_contains: &["typescript"],
            files_modified: &["web/src/lib/http.ts"],
            row_kinds: &["ENDPOINT"],
            ..EXPECT
        },
    },
    Step {
        name: "wrapper_site_add",
        edit: wrapper_site_add,
        via: Via::Apply(&["web/src/features/users.ts"]),
        expect: Expect {
            rebuilt_contains: &["typescript"],
            files_modified: &["web/src/features/users.ts"],
            row_kinds: &["ENDPOINT"],
            ..EXPECT
        },
    },
    Step {
        name: "heritage_base_remove",
        edit: heritage_base_remove,
        via: Via::Apply(&["web/src/services/base.service.ts"]),
        expect: Expect {
            rebuilt_contains: &["typescript"],
            files_modified: &["web/src/services/base.service.ts"],
            row_kinds: &["ENDPOINT"],
            ..EXPECT
        },
    },
    Step {
        name: "wrapper_module_move",
        edit: wrapper_module_move,
        via: Via::Apply(&["web/src/lib/http.js"]),
        expect: Expect {
            rebuilt_contains: &["typescript"],
            files_added: &["web/src/lib/http.js"],
            ..EXPECT
        },
    },
];

#[test]
fn scripted_wrappers_memo() {
    run("wrappers_memo", SEQ_WRAPPERS);
}

// ---- GA.4: the body-only fast path ------------------------------------------

/// GA.4's Python fixture at `<tmp>/repo`: four files, each importing the one
/// before (`app` -> `pkg.service` -> `pkg.store` -> `pkg.models`).
fn write_py_fixture(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    for (rel, body) in PY_INLINE {
        write(&repo, rel, body);
    }
    repo
}

const PY_INLINE: &[(&str, &str)] = &[
    (
        "pkg/models.py",
        "class User:\n    def __init__(self, name):\n        self.name = name\n\n    def greet(self):\n        return \"hi \" + self.name\n",
    ),
    (
        "pkg/store.py",
        "from pkg.models import User\n\n\ndef load(name):\n    return User(name)\n",
    ),
    (
        "pkg/service.py",
        "from pkg.store import load\n\n\ndef greet_all(names):\n    out = []\n    for n in names:\n        out.append(load(n).greet())\n    return out\n",
    ),
    (
        "app.py",
        "from pkg.service import greet_all\n\n\ndef main():\n    return greet_all([\"a\", \"b\"])\n",
    ),
];

fn py_body(r: &Path) {
    replace(
        r,
        "pkg/store.py",
        "    return User(name)\n",
        "    user = User(name)\n    return user\n",
    );
}
/// Two statements ahead of `greet`: every later row of models.py moves.
fn py_body_shift(r: &Path) {
    replace(
        r,
        "pkg/models.py",
        "        self.name = name\n",
        "        cleaned = name.strip()\n        assert cleaned is not None\n        self.name = name\n",
    );
}
fn py_two_bodies(r: &Path) {
    replace(
        r,
        "pkg/service.py",
        "        out.append(load(n).greet())\n",
        "        user = load(n)\n        out.append(user.greet())\n",
    );
    replace(
        r,
        "app.py",
        "    return greet_all([\"a\", \"b\"])\n",
        "    names = [\"a\", \"b\"]\n    return greet_all(names)\n",
    );
}
fn py_body_again(r: &Path) {
    replace(
        r,
        "pkg/store.py",
        "    return user\n",
        "    user.greet()\n    return user\n",
    );
}
/// A new function: store.py's surface moves.
fn py_surface(r: &Path) {
    let store = std::fs::read_to_string(r.join("pkg/store.py")).expect("read");
    write(
        r,
        "pkg/store.py",
        &format!("{store}\n\ndef drop(name):\n    return name\n"),
    );
}
/// The importing end of the chain.
fn py_importer_body(r: &Path) {
    replace(
        r,
        "app.py",
        "    return greet_all(names)\n",
        "    greeted = greet_all(names)\n    return greeted\n",
    );
}

/// The fast path's expectation for a body-only step of `files`.
const fn body_step(name: &'static str, edit: fn(&Path), files: &'static [&'static str]) -> Step {
    Step {
        name,
        edit,
        via: Via::Apply(files),
        expect: Expect {
            rebuilt_exact: Some(&["python"]),
            body_only: Some(&["python"]),
            files_modified: files,
            ..EXPECT
        },
    }
}

const SEQ_PY_FAST: &[Step] = &[
    body_step("py_body", py_body, &["pkg/store.py"]),
    body_step("py_body_shift", py_body_shift, &["pkg/models.py"]),
    body_step(
        "py_two_bodies",
        py_two_bodies,
        &["app.py", "pkg/service.py"],
    ),
    body_step("py_body_again", py_body_again, &["pkg/store.py"]),
    Step {
        name: "py_surface",
        edit: py_surface,
        via: Via::Apply(&["pkg/store.py"]),
        expect: Expect {
            rebuilt_exact: Some(&["python"]),
            body_only: Some(&[]),
            files_modified: &["pkg/store.py"],
            row_changes: &["added"],
            ..EXPECT
        },
    },
    body_step("py_importer_body", py_importer_body, &["app.py"]),
];

/// GA.4: the body-only fast path on the Python fixture: after every step
/// the layout is a clean build's (checks 1-3), and `body_only` is
/// `["python"]` exactly on the body-only steps, `[]` on the surface step.
#[test]
fn scripted_py_fast_path() {
    run_on(PY, "py_fast_path", SEQ_PY_FAST, None);
}

/// GA.7's Go fixture at `<tmp>/repo`: a module whose service types its
/// receivers through a package var, a range element and a closure, a store
/// whose methods sit in a second file (split receivers) above an embedding
/// struct, a gin route file and a JSON handler (both refused by the fast
/// path: rebuilt, still byte-identical).
fn write_go_fixture(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    for (rel, body) in GO_INLINE {
        write(&repo, rel, body);
    }
    repo
}

const GO_INLINE: &[(&str, &str)] = &[
    ("go.mod", "module example.com/goparity\n\ngo 1.21\n"),
    (
        "models/models.go",
        "package models\n\ntype Item struct {\n\tID   string\n\tName string\n}\n\nfunc (i *Item) Label() string {\n\treturn i.Name\n}\n\nfunc NewItem(id string) *Item {\n\treturn &Item{ID: id}\n}\n\ntype Namer interface {\n\tLabel() string\n}\n",
    ),
    (
        "store/store.go",
        "package store\n\nimport \"example.com/goparity/models\"\n\ntype Store struct {\n\titems []*models.Item\n}\n\nfunc New() *Store {\n\treturn &Store{}\n}\n\nfunc (s *Store) Items() []*models.Item {\n\treturn s.items\n}\n",
    ),
    (
        "store/extra.go",
        "package store\n\nimport \"example.com/goparity/models\"\n\nfunc (s *Store) Count() int {\n\treturn len(s.Items())\n}\n\nfunc (s *Store) Add(id string) {\n\ts.items = append(s.items, models.NewItem(id))\n}\n\ntype Cached struct {\n\tStore\n\thits int\n}\n",
    ),
    (
        "service/service.go",
        "package service\n\nimport (\n\t\"example.com/goparity/models\"\n\t\"example.com/goparity/store\"\n)\n\nvar shared = store.New()\n\nfunc helper() int {\n\treturn 1\n}\n\nfunc Run() int {\n\tshared.Add(\"a\")\n\tfor _, it := range shared.Items() {\n\t\tit.Label()\n\t}\n\trun := func() int { return helper() }\n\titem := &models.Item{ID: \"x\"}\n\t_ = item\n\treturn run()\n}\n",
    ),
    (
        "routes/routes.go",
        "package routes\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc Register(r *gin.Engine) {\n\tr.GET(\"/items\", list)\n}\n\nfunc list(c *gin.Context) {\n\tc.String(200, \"ok\")\n}\n",
    ),
    (
        "api/handler.go",
        "package api\n\nimport (\n\t\"net/http\"\n\n\t\"example.com/goparity/models\"\n\t\"github.com/gin-gonic/gin\"\n)\n\nfunc Show(c *gin.Context) {\n\tc.JSON(http.StatusOK, models.Item{})\n}\n",
    ),
];

/// A typed-receiver call through the package var, two rows ahead of the
/// range loop.
fn go_body(r: &Path) {
    replace(
        r,
        "service/service.go",
        "\tshared.Add(\"a\")\n",
        "\tn := shared.Count()\n\t_ = n\n\tshared.Add(\"a\")\n",
    );
}
/// A split-file method's body grows: the embedding struct below it moves.
fn go_split_body(r: &Path) {
    replace(
        r,
        "store/extra.go",
        "\treturn len(s.Items())\n",
        "\titems := s.Items()\n\ttotal := len(items)\n\treturn total\n",
    );
}
fn go_two_bodies(r: &Path) {
    replace(
        r,
        "models/models.go",
        "\treturn i.Name\n",
        "\tif i == nil {\n\t\treturn \"\"\n\t}\n\treturn i.Name\n",
    );
    replace(
        r,
        "service/service.go",
        "\trun := func() int { return helper() }\n",
        "\trun := func() int {\n\t\tshared.Count()\n\t\treturn helper()\n\t}\n",
    );
}
/// A route file's handler body: refused (`route`), the group rebuilt.
fn go_routes_body(r: &Path) {
    replace(
        r,
        "routes/routes.go",
        "\tc.String(200, \"ok\")\n",
        "\tc.String(201, \"ok\")\n",
    );
}
/// A JSON handler's body: refused (`wire`), the group rebuilt.
fn go_wire_body(r: &Path) {
    replace(
        r,
        "api/handler.go",
        "\tc.JSON(http.StatusOK, models.Item{})\n",
        "\tc.Header(\"x\", \"y\")\n\tc.JSON(http.StatusOK, models.Item{})\n",
    );
}
/// A new function: store.go's surface moves.
fn go_surface(r: &Path) {
    let store = std::fs::read_to_string(r.join("store/store.go")).expect("read");
    write(
        r,
        "store/store.go",
        &format!("{store}\nfunc Drop(s *Store) int {{\n\treturn s.Count()\n}}\n"),
    );
}
fn go_body_again(r: &Path) {
    replace(
        r,
        "service/service.go",
        "\t_ = item\n",
        "\t_ = item.Label()\n",
    );
}

/// GA.7: a Go step rebuilding the `go` group, through the fast path
/// (`body_only` `["go"]`) or not (`[]`: refused, or the surface moved).
const fn go_step(
    name: &'static str,
    edit: fn(&Path),
    files: &'static [&'static str],
    fast: bool,
) -> Step {
    Step {
        name,
        edit,
        via: Via::Apply(files),
        expect: Expect {
            rebuilt_exact: Some(&["go"]),
            body_only: Some(if fast { &["go"] } else { &[] }),
            files_modified: files,
            ..EXPECT
        },
    }
}

const SEQ_GO_FAST: &[Step] = &[
    go_step("go_body", go_body, &["service/service.go"], true),
    go_step("go_split_body", go_split_body, &["store/extra.go"], true),
    go_step(
        "go_two_bodies",
        go_two_bodies,
        &["models/models.go", "service/service.go"],
        true,
    ),
    go_step(
        "go_routes_body",
        go_routes_body,
        &["routes/routes.go"],
        false,
    ),
    go_step("go_wire_body", go_wire_body, &["api/handler.go"], false),
    Step {
        name: "go_surface",
        edit: go_surface,
        via: Via::Apply(&["store/store.go"]),
        expect: Expect {
            rebuilt_exact: Some(&["go"]),
            body_only: Some(&[]),
            files_modified: &["store/store.go"],
            row_changes: &["added"],
            ..EXPECT
        },
    },
    go_step(
        "go_body_again",
        go_body_again,
        &["service/service.go"],
        true,
    ),
];

/// GA.7: the Go body-only fast path: after every step the layout is a clean
/// build's (checks 1-3); `body_only` is `["go"]` on the body-only steps and
/// `[]` on a route file, a JSON handler (both refused, then rebuilt) and the
/// surface step.
#[test]
fn scripted_go_fast_path() {
    run_on(GO, "go_fast_path", SEQ_GO_FAST, None);
}

#[test]
fn scripted_budgeted() {
    let sequences: &[(&str, &[Step])] = &[
        ("body", &[BODY]),
        ("declaration", SEQ_DECLARATION),
        ("add_delete_rename", SEQ_ADD_DELETE_RENAME),
        ("same_stem", SEQ_SAME_STEM),
        ("client", SEQ_CLIENT),
        ("go_mod", SEQ_GO_MOD),
        ("tsconfig", SEQ_TSCONFIG),
        ("cargo_member", SEQ_CARGO_MEMBER),
        ("overlay", SEQ_OVERLAY),
        ("proto", SEQ_PROTO),
        ("markdown", SEQ_MARKDOWN),
        ("include_roots", SEQ_INCLUDE_ROOTS),
        ("gitignore", SEQ_GITIGNORE),
        ("imports_memo", SEQ_IMPORTS_MEMO),
        ("noop", SEQ_NOOP),
        ("conf", &[CONF]),
        ("sbt", &[SBT]),
        ("wrappers_memo", SEQ_WRAPPERS),
    ];
    for (name, steps) in sequences {
        run_with(
            &format!("budgeted_{name}"),
            steps,
            Some(MemoryBudget::mb(1)),
        );
    }
    run_on(
        PY,
        "budgeted_py_fast_path",
        SEQ_PY_FAST,
        Some(MemoryBudget::mb(1)),
    );
    run_on(
        GO,
        "budgeted_go_fast_path",
        SEQ_GO_FAST,
        Some(MemoryBudget::mb(1)),
    );
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
    assert_eq!(
        [r.next(), r.next(), r.next()],
        [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F
        ]
    );
}

/// One random edit. Toggles flip between the fixture's state and one other.
#[derive(Clone, Copy)]
enum Op {
    /// A statement as the first line of the last function of a python / go /
    /// ts / rust file.
    Body,
    /// GA.4: a call statement as the first line of the last function of a
    /// python file: a body-only edit the fast path may re-stitch.
    PyBody,
    /// GA.7: the same in a go file (the Go fast path, or its refusal).
    GoBody,
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
    const ALL: [Op; 16] = [
        Op::Body,
        Op::PyBody,
        Op::GoBody,
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
            Op::PyBody => "py_body",
            Op::GoBody => "go_body",
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

    /// Its cards in one [`World::pick`] bag: 16 in all, one default
    /// sequence (GA.4's `py_body` took one of `body`'s two; GA.7 added
    /// `go_body`).
    fn weight(self) -> usize {
        1
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
        format!(
            "step={} op={} path={}",
            self.step,
            self.op.name(),
            self.paths.join(",")
        )
    }
}

/// `rel`'s extension (`""` for none).
fn ext(rel: &str) -> &str {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.rsplit_once('.').map_or("", |(_, e)| e)
}

/// `text` (python) with `stmt` as the first line of its last `def`'s body.
fn in_last_py_fn(text: &str, stmt: &str) -> String {
    let (mut off, mut hit) = (0, None);
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("def ") && t.ends_with(':') {
            hit = Some((off + line.len(), line.len() - line.trim_start().len()));
        }
        off += line.len();
    }
    let (end, indent) = hit.unwrap_or_else(|| panic!("no def in {text:?}"));
    let stmt = format!("{}{stmt}\n", " ".repeat(indent + 4));
    format!("{}{stmt}{}", &text[..end], &text[end..])
}

/// `text` with one statement for step `i` as the first line of its last
/// function's body, by `rel`'s language.
fn in_last_fn(rel: &str, text: &str, i: usize) -> String {
    let (at, stmt) = match ext(rel) {
        "py" => return in_last_py_fn(text, &format!("_b{i} = {i}")),
        lang @ ("go" | "rs") => {
            let kw = if lang == "go" { "func " } else { "fn " };
            let start = text
                .match_indices(kw)
                .map(|(p, _)| p)
                .filter(|&p| p == 0 || matches!(text.as_bytes()[p - 1], b'\n' | b' '))
                .last()
                .unwrap_or_else(|| panic!("{rel}: no {kw}"));
            let brace = start
                + text[start..]
                    .find('{')
                    .unwrap_or_else(|| panic!("{rel}: no body"));
            let stmt = if lang == "go" {
                format!("\n\t_ = {i}\n")
            } else {
                format!("\n    let _b{i} = {i};\n")
            };
            (brace + 1, stmt)
        }
        "ts" => {
            let (mut off, mut hit) = (0, None);
            for line in text.split_inclusive('\n') {
                let t = line.trim_end();
                let first = t
                    .trim_start()
                    .split(|c: char| !c.is_alphanumeric())
                    .next()
                    .unwrap_or("");
                let opens = t.ends_with('{') || t.ends_with("{}");
                if opens
                    && t.contains(')')
                    && !["if", "for", "while", "switch", "catch"].contains(&first)
                {
                    hit = t.rfind('{').map(|b| off + b + 1);
                }
                off += line.len();
            }
            (
                hit.unwrap_or_else(|| panic!("{rel}: no function")),
                format!("\n  const _b{i} = {i};\n"),
            )
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
                let rel = p
                    .strip_prefix(repo)
                    .expect("under repo")
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, std::fs::read(&p).expect("read"));
            }
        }
    }
    out
}

/// The walked side of a tree: every file outside `.glia/`.
fn walked(tree: &BTreeMap<String, Vec<u8>>) -> BTreeSet<String> {
    tree.keys()
        .filter(|r| !r.starts_with(".glia/"))
        .cloned()
        .collect()
}

const OVERLAY: &str = ".glia/overlay.toml";
const PROTO: &str = "grpc/server/user.proto";
const SAME_STEM: &str = "api/user.ts";
const CLIENT_HEAD: &str = "\nexport async function getUsers() {";
const SCIP_FILES: [&str; 3] = [
    ".glia/scip-snapshot/documents.jsonl",
    ".glia/scip-snapshot/meta.json",
    ".glia/scip-snapshot/symbols.jsonl",
];

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
            self.bag.extend(
                Op::ALL
                    .iter()
                    .flat_map(|&o| std::iter::repeat_n(o, o.weight())),
            );
        }
        let open: Vec<usize> = (0..self.bag.len())
            .filter(|&i| applies(&self.bag[i]))
            .collect();
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
                let files: Vec<String> = self
                    .mirror
                    .iter()
                    .filter(|r| matches!(ext(r), "py" | "go" | "ts" | "rs"))
                    .cloned()
                    .collect();
                let rel = self.choose(&files);
                let edited = in_last_fn(&rel, &self.read(&rel), i);
                self.put(&rel, &edited);
                vec![rel]
            }
            Op::PyBody => {
                let files: Vec<String> = self
                    .mirror
                    .iter()
                    .filter(|r| ext(r) == "py")
                    .cloned()
                    .collect();
                let rel = self.choose(&files);
                let edited = in_last_py_fn(&self.read(&rel), &format!("_p{i} = len([{i}])"));
                self.put(&rel, &edited);
                vec![rel]
            }
            Op::GoBody => {
                let files: Vec<String> = self
                    .mirror
                    .iter()
                    .filter(|r| ext(r) == "go")
                    .cloned()
                    .collect();
                let rel = self.choose(&files);
                let text = self.read(&rel);
                let edited = in_last_fn(&rel, &text, i).replacen(
                    &format!("\t_ = {i}\n"),
                    &format!("\t_ = len(\"g{i}\")\n"),
                    1,
                );
                self.put(&rel, &edited);
                vec![rel]
            }
            Op::Declaration if self.rng.next().is_multiple_of(2) => {
                self.append(
                    "api/user.py",
                    &format!("\n\ndef decl_{i}():\n    return {i}\n"),
                );
                let call = format!(
                    "\nfrom api.user import decl_{i}\n\n\ndef call_{i}():\n    return decl_{i}()\n"
                );
                self.append("api/svc.py", &call);
                vec!["api/svc.py".into(), "api/user.py".into()]
            }
            Op::Declaration => {
                self.append(
                    "tools/gen.go",
                    &format!("\nfunc Decl{i}() int {{ return {i} }}\n"),
                );
                self.append(
                    "cmd/main.go",
                    &format!("\nfunc call{i}() int {{ return tools.Decl{i}() }}\n"),
                );
                vec!["cmd/main.go".into(), "tools/gen.go".into()]
            }
            Op::AddFile => {
                let (rel, body) = match self.rng.below(3) {
                    0 => (
                        format!("api/added_{i}.py"),
                        format!(
                            "from api.user import get_user\n\n\ndef added_{i}():\n    return get_user()\n"
                        ),
                    ),
                    1 => (
                        format!("tools/added_{i}.go"),
                        format!("package tools\n\nfunc Added{i}() int {{\n\treturn Gen()\n}}\n"),
                    ),
                    _ => (
                        format!("pkg{i}/mod_{i}.py"),
                        format!(
                            "from api.svc import handle\n\n\ndef mod_{i}():\n    return handle()\n"
                        ),
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
                self.put(
                    SAME_STEM,
                    "export function userTs(): number {\n  return 1;\n}\n",
                );
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
                std::fs::File::open(self.repo.join(&rel))
                    .and_then(|f| f.set_modified(stamp))
                    .expect("mtime");
                vec![rel]
            }
            Op::GoMod => self.toggle(
                "go.mod",
                "module example.com/parity\n",
                "module example.com/moved\n",
            ),
            Op::Tsconfig => {
                self.toggle("web/tsconfig.json", "[\"app/core/*\"]", "[\"app/none/*\"]")
            }
            Op::Proto => self.toggle(
                PROTO,
                "  rpc ListUsers",
                "  rpc DeleteUser (GetUserRequest) returns (User);\n  rpc ListUsers",
            ),
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
    (
        "api/base.py",
        "class Base:\n    def run(self):\n        return 0\n",
    ),
    (
        "api/child.py",
        "class Child(Base):\n    def run(self):\n        return 1\n",
    ),
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
        (
            "api/child.py",
            "python",
            &[(4, "class Child", "Child")],
            &[],
        ),
        (
            "api/svc.py",
            "python",
            &[],
            &[(0, "get_user()"), (5, "get_user()")],
        ),
        (
            "api/user.py",
            "python",
            &[(5, "def get_user(", "get_user")],
            &[],
        ),
        ("rs/app/src/main.rs", "rust", &[], &[(6, "e.run()")]),
        (
            "rs/core/src/api.rs",
            "rust",
            &[(6, "pub fn run(", "run")],
            &[],
        ),
        ("tools/gen.go", "go", &[(0, "func Gen(", "Gen")], &[]),
        (
            "tools/shape.go",
            "go",
            &[
                (1, "type Shape interface", "Shape"),
                (2, "type Square struct", "Square"),
            ],
            &[],
        ),
        (
            "web/src/app/core/api.ts",
            "typescript",
            &[(1, "function listThings(", "listThings")],
            &[],
        ),
    ];
    let documents: Vec<ScipDocumentRecord> = DOCUMENTS
        .iter()
        .map(|&(path, language, defs, refs)| {
            let text = std::fs::read_to_string(repo.join(path)).expect("read");
            let line = |needle: &str| {
                let at = text.lines().position(|l| l.contains(needle));
                u32::try_from(at.unwrap_or_else(|| panic!("{path}: {needle}")))
                    .expect("a short file")
            };
            ScipDocumentRecord {
                path: path.into(),
                language: language.into(),
                source_hash: source_hash(text.as_bytes()),
                defs: defs
                    .iter()
                    .map(|&(s, needle, name)| ScipDefRow {
                        s,
                        line: line(needle),
                        name: name.into(),
                    })
                    .collect(),
                refs: refs
                    .iter()
                    .map(|&(s, needle)| ScipRefRow {
                        s,
                        line: line(needle),
                        call: true,
                        write: false,
                        import: false,
                    })
                    .collect(),
            }
        })
        .collect();
    let symbols: Vec<ScipSymbolRecord> = (0u32..)
        .zip(SYMBOLS)
        .map(|(id, symbol)| ScipSymbolRecord {
            id,
            symbol: symbol.into(),
            implements: IMPLEMENTS
                .iter()
                .filter(|(a, _)| *a == id)
                .map(|&(_, b)| b)
                .collect(),
        })
        .collect();
    let meta = ScipMeta::new("scip-parity".into(), "0".into(), String::new(), 0);
    write_scip(repo, meta, &documents, &symbols).expect("write the snapshot");
}

/// What one flush must show, from the tree's bytes at the last flush and
/// now: whether anything changed (else a noop: `seq` stays, no rows), the
/// table entries added / removed (the mirror) and modified (changed bytes,
/// the non-table inputs aside), the `full_rebuild` reason (`full_rebuild_reason`'s
/// precedence), and the groups: every group under a proto service change,
/// exactly the edited files' groups when every change is an in-place source
/// / doc edit (GA.17: or the overlay, whose `GATEWAY` pin moves the
/// `typescript` group alone), else at least the changed paths' groups.
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
        let changed: BTreeSet<&str> = before
            .keys()
            .chain(after.keys())
            .filter(|k| before.get(*k) != after.get(*k))
            .map(String::as_str)
            .collect();
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
        let mut contains: BTreeSet<&'static str> =
            changed.iter().filter_map(|k| group_of(k)).collect();
        if changed.contains(SAME_STEM) && !in_both(SAME_STEM) {
            contains.insert("python");
        }
        // GA.17: the overlay's `GATEWAY` pin re-keys api.ts's ENDPOINT, so
        // it moves the typescript group's post-graft parse and no other.
        if changed.contains(OVERLAY) {
            contains.insert("typescript");
        }
        let in_place = changed
            .iter()
            .filter(|k| **k != OVERLAY)
            .all(|k| in_both(k) && matches!(ext(k), "py" | "go" | "ts" | "rs" | "md"));
        let rebuilt_exact = if changed.contains(PROTO) {
            Some(EVERY_GROUP.to_vec())
        } else if in_place {
            Some(
                EVERY_GROUP
                    .iter()
                    .copied()
                    .filter(|g| contains.contains(g))
                    .collect(),
            )
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

    fn check(
        &self,
        u: &WatchUpdate,
        seq: u64,
        prev: &GenerateResult,
        clean: &GenerateResult,
        at: &str,
    ) {
        assert_eq!(u.seq, if self.changed { seq + 1 } else { seq }, "{at}: seq");
        let moved = assert_delta(u, prev, clean, at);
        assert!(
            self.changed || !moved,
            "{at}: an unchanged tree moves nothing"
        );
        assert_eq!(u.files.added, self.added, "{at}: files added (the mirror)");
        assert_eq!(
            u.files.removed, self.removed,
            "{at}: files removed (the mirror)"
        );
        assert_eq!(u.files.modified, self.modified, "{at}: files modified");
        assert_eq!(
            u.full_rebuild.as_deref(),
            self.full_rebuild,
            "{at}: full_rebuild"
        );
        if let Some(exact) = &self.rebuilt_exact {
            assert_eq!(&u.groups_rebuilt, exact, "{at}: groups rebuilt");
        }
        // GA.4 / GA.7: only python and go have a fast path, and its groups
        // are rebuilt ones.
        for key in &u.body_only {
            assert!(
                (key == "python" || key == "go") && u.groups_rebuilt.contains(key),
                "{at}: body_only {key}: {:?} of {:?}",
                u.body_only,
                u.groups_rebuilt
            );
        }
        for g in &self.rebuilt_contains {
            assert!(
                u.groups_rebuilt.iter().any(|r| r == g),
                "{at}: {g} rebuilt: {:?}",
                u.groups_rebuilt
            );
        }
    }
}

/// What a run covered, by op and by route.
#[derive(Default)]
struct Tally {
    ops: BTreeMap<&'static str, usize>,
    routes: BTreeMap<Route, usize>,
    noops: usize,
    /// GA.4: updates that took the body-only fast path.
    body_only: usize,
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
    let mut w = World {
        repo: repo.clone(),
        rng: SplitMix64(seed),
        mirror: initial.clone(),
        initial,
        bag: Vec::new(),
    };
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
                .map(|p| {
                    if r & 2 == 0 {
                        PathBuf::from(p)
                    } else {
                        repo.join(p)
                    }
                })
                .collect();
            named.sort();
            named.dedup();
            Some(
                s.apply(&named)
                    .unwrap_or_else(|e| panic!("{at}: apply: {e}")),
            )
        } else {
            s.poll().unwrap_or_else(|e| panic!("{at}: poll: {e}"))
        };
        let clean = generate_one(root).expect("clean build");
        let step_out = out.join(i.to_string());
        assert_parity(&s, &clean, &step_out, &at);
        std::fs::remove_dir_all(&step_out).expect("rm the step's layouts");
        match &update {
            Some(u) => {
                want.check(u, seq, &prev, &clean, &at);
                if !u.body_only.is_empty() {
                    tally.body_only += 1;
                }
            }
            None => {
                assert!(!want.changed, "{at}: the poll sees the edit");
                assert_eq!(s.seq(), seq, "{at}: seq");
                let (_, nodes, edges) = located_delta(&prev.merged, &clean.merged);
                assert!(
                    nodes.is_empty() && edges.is_empty(),
                    "{at}: an unchanged tree builds the same graph"
                );
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
        Ok(v) => v
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}={v}: not a count")),
        Err(_) => default,
    }
}

#[test]
fn random_sequences() {
    let soak = [
        "GLIA_WATCH_PARITY_SEED",
        "GLIA_WATCH_PARITY_SEEDS",
        "GLIA_WATCH_PARITY_STEPS",
    ]
    .iter()
    .any(|v| std::env::var(v).is_ok());
    let seeds: Vec<u64> = match std::env::var("GLIA_WATCH_PARITY_SEED") {
        Ok(_) => vec![env_count("GLIA_WATCH_PARITY_SEED", 0)],
        Err(_) => (1..=env_count("GLIA_WATCH_PARITY_SEEDS", 4)).collect(),
    };
    let steps = usize::try_from(env_count("GLIA_WATCH_PARITY_STEPS", 15)).expect("a step count");
    // Stamps stay behind the clock (`random_sequence`).
    assert!(
        steps < 3000,
        "GLIA_WATCH_PARITY_STEPS={steps}: at most 2999"
    );
    let mut tally = Tally::default();
    for &seed in &seeds {
        random_sequence(seed, steps, &mut tally);
    }
    eprintln!(
        "[watch-parity] random seeds={} steps={steps} ops={:?} routes={:?} noops={} body_only={}",
        seeds.len(),
        tally.ops,
        tally.routes,
        tally.noops,
        tally.body_only
    );
    if !soak {
        // The default seeds reach every op and route: a table edit that
        // drops one from them fails here, not silently.
        for op in Op::ALL {
            assert!(
                tally.ops.contains_key(op.name()),
                "seeds 1..=4 never ran {}: {:?}",
                op.name(),
                tally.ops
            );
        }
        assert_eq!(
            tally.routes.len(),
            3,
            "seeds 1..=4 take every route: {:?}",
            tally.routes
        );
        // GA.4: the default seeds reach the body-only fast path.
        assert!(
            tally.body_only > 0,
            "seeds 1..=4 never took the body-only fast path"
        );
    }
}
