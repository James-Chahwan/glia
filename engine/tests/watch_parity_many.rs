//! GA.10: the multi-repo watch parity gate, DC.10a / DC.10b's bar for
//! [`WatchSession::open_many`] (GA.9). One session over two repos written by
//! the test is stepped through edits, and after EVERY step:
//!
//! 1. the session's `parse_errors`, `merged.pass_undo`, totals, `repo_labels`
//!    and `repo_roots` equal a clean `generate_many_opts(roots, false, ..)` of
//!    the trees as they now are;
//! 2. the session's persisted layout equals the clean build's
//!    `persist_result`, file by file, byte for byte (both written at the same
//!    depth, so their recorded relative roots agree);
//! 3. the update's node / edge rows serialise equal to the public
//!    `delta::located_delta(previous clean, clean)` and its counts to that
//!    delta's list lengths;
//! 4. the ranking cache is the same `Arc` exactly when the update is a noop
//!    (EA.7: one session-level `RankCache` over the joint result's roots);
//! 5. the step's expectation: `<label>:`-prefixed groups rebuilt and table
//!    entries, the full-rebuild reason and the rows the step must show.
//!
//! The fixture ([`Fixture`]) is two repos, `<tmp>/server` first:
//! - **server**: `proto/greeter.proto` (service `Greeter`, rpc `Hi`), a Go
//!   gRPC server registering `Greeter`, a Go chi handler `GET /api/orders`,
//!   `go.mod` module `example.com/server`;
//! - **client**: a Go `GreeterClient.Hi` caller, a Go `FarewellClient.Bye`
//!   caller (no proto declares `Farewell` until the proto step adds it, so
//!   the client repo's needles move on a server-repo edit), a TS fetch of
//!   `http://server:8080/api/orders`, a TS fetch through an env-var base the
//!   overlay step folds, and a `go.mod` requiring `example.com/server`.
//!
//! So the joint build holds the inputs one repo never exercises: the proto
//! service set unioned over both repos, a cross-repo HTTP_CALLS (TS client ->
//! Go route) and GRPC_CALLS, and a cross-repo `manifest_name` SAME_AS
//! (client's PACKAGE_DEP -> server's PROJECT); [`assert_fixture_shape`]
//! checks each is there at open.
//!
//! The scripted sequences ([`scripted_body_client`] .. [`scripted_overlay`])
//! are the spec's (a)-(g); the random half ([`random_sequences`]) is seeded
//! [`SplitMix64`] sequences over the op table [`Op`], each step reaching the
//! session through `apply` of its absolute paths or the stat poll, checked
//! against the oracle [`Want`] built from byte snapshots of both trees.
//! Seeds `1..=3` x 15 steps.
//!
//! Falsifiability, checked on a scratch copy of HEAD when this landed:
//! keying each repo's needles on its own files instead of the unioned proto
//! service set fails every sequence at open (the client's GreeterClient
//! stub is no GRPC_CLIENT: totals 29 / 20 against 30 / 22), and an update
//! that never recomputes the service fingerprint fails `proto` at its
//! groups (only `server:proto` rebuilt) and the random sequences at their
//! first proto op (`full_rebuild` none).
//!
//! fired_on (stderr, not gated here): the session's `[watch] open repos=2
//! repo=<server>,<client> ...` line at open and `[watch] update ...` per
//! rebuilding update; the test asserts the same facts through
//! [`WatchSession::roots`] and `WatchUpdate::full_rebuild`
//! (`proto_services` on the proto steps).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use glia_code_domain::edge_category;
use glia_engine::delta::located_delta;
use glia_engine::persist::persist_result;
use glia_engine::watch::{WatchSession, WatchUpdate};
use glia_engine::{BuildOptions, GenerateResult, generate_many_opts};

// ---- the fixture ------------------------------------------------------------

/// Which repo of the fixture: the server is the first root, the client the
/// second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Server,
    Client,
}

impl Side {
    const BOTH: [Side; 2] = [Side::Server, Side::Client];

    fn slot(self) -> usize {
        match self {
            Side::Server => 0,
            Side::Client => 1,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Side::Server => "server",
            Side::Client => "client",
        }
    }

    /// The side's language groups, in plan order: every one rebuilds under a
    /// `proto_services` full rebuild.
    fn groups(self) -> &'static [&'static str] {
        match self {
            Side::Server => SERVER_GROUPS,
            Side::Client => CLIENT_GROUPS,
        }
    }
}

const SERVER_GROUPS: &[&str] = &["go", "proto"];
const CLIENT_GROUPS: &[&str] = &["go", "manifest", "typescript"];

const PROTO: &str = "proto/greeter.proto";
const GREETER: &str = "syntax = \"proto3\";\n\npackage greet;\n\nservice Greeter {\n  rpc Hi (HiRequest) returns (HiReply);\n}\n\nmessage HiRequest {\n  string name = 1;\n}\n\nmessage HiReply {\n  string text = 1;\n}\n";
const FAREWELL: &str = "\nservice Farewell {\n  rpc Bye (HiRequest) returns (HiReply);\n}\n";
const REQUIRE: &str = "require example.com/server v0.1.0";
const OVERLAY: &str = ".glia/overlay.toml";

const SERVER_FILES: &[(&str, &str)] = &[
    ("go.mod", "module example.com/server\n\ngo 1.21\n"),
    (PROTO, GREETER),
    (
        "rpc/greeter.go",
        "package rpc\n\nimport (\n\t\"context\"\n\n\tpb \"example.com/server/gen/greet\"\n\t\"google.golang.org/grpc\"\n)\n\ntype greeterServer struct{}\n\nfunc (s *greeterServer) Hi(ctx context.Context, req *pb.HiRequest) (*pb.HiReply, error) {\n\treturn &pb.HiReply{Text: req.Name}, nil\n}\n\nfunc Serve(srv *grpc.Server) {\n\tpb.RegisterGreeterServer(srv, &greeterServer{})\n}\n",
    ),
    (
        "web/orders.go",
        "package web\n\nimport (\n\t\"net/http\"\n\n\t\"github.com/go-chi/chi/v5\"\n)\n\nfunc listOrders(w http.ResponseWriter, r *http.Request) {}\n\nfunc Routes() http.Handler {\n\tr := chi.NewRouter()\n\tr.Get(\"/api/orders\", listOrders)\n\treturn r\n}\n",
    ),
];

const CLIENT_FILES: &[(&str, &str)] = &[
    (
        "go.mod",
        "module example.com/client\n\ngo 1.21\n\nrequire example.com/server v0.1.0\n",
    ),
    (
        "rpc/hi.go",
        "package rpc\n\nimport (\n\t\"context\"\n\n\tpb \"example.com/server/gen/greet\"\n\t\"google.golang.org/grpc\"\n)\n\nfunc SayHi(conn *grpc.ClientConn) {\n\tc := pb.NewGreeterClient(conn)\n\tc.Hi(context.Background(), &pb.HiRequest{Name: \"a\"})\n}\n",
    ),
    (
        "rpc/bye.go",
        "package rpc\n\nimport (\n\t\"context\"\n\n\tpb \"example.com/server/gen/greet\"\n\t\"google.golang.org/grpc\"\n)\n\nfunc SayBye(conn *grpc.ClientConn) {\n\tc := pb.NewFarewellClient(conn)\n\tc.Bye(context.Background(), &pb.HiRequest{Name: \"b\"})\n}\n",
    ),
    (
        "web/orders.ts",
        "export async function listOrders(): Promise<unknown> {\n  const res = await fetch(\"http://server:8080/api/orders\");\n  return res.json();\n}\n",
    ),
    (
        "web/gateway.ts",
        "const GATEWAY = process.env.GATEWAY_URL;\n\nexport async function ping(): Promise<unknown> {\n  return fetch(`${GATEWAY}/ping`);\n}\n",
    ),
];

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).expect("read")
}

/// `old` (present exactly once) replaced by `new` in `dir/rel`.
fn replace(dir: &Path, rel: &str, old: &str, new: &str) {
    let text = read(dir, rel);
    assert_eq!(text.matches(old).count(), 1, "{rel}: one {old:?}");
    write(dir, rel, &text.replacen(old, new, 1));
}

/// The two repos under one tempdir, the roots as the session is given them
/// (absolute, so each label is its root), and a scratch dir for layouts.
struct Fixture {
    tmp: tempfile::TempDir,
    dirs: [PathBuf; 2],
    roots: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = Side::BOTH.map(|side| tmp.path().join(side.name()));
        for (side, files) in [(Side::Server, SERVER_FILES), (Side::Client, CLIENT_FILES)] {
            for (rel, body) in files {
                write(&dirs[side.slot()], rel, body);
            }
        }
        let roots = dirs
            .iter()
            .map(|d| d.to_str().expect("utf-8 root").to_string())
            .collect();
        Fixture { tmp, dirs, roots }
    }

    fn dir(&self, side: Side) -> &Path {
        &self.dirs[side.slot()]
    }

    /// `rel` as a multi-repo update names it: `<label>:<rel>`.
    fn tag(&self, side: Side, rel: &str) -> String {
        format!("{}:{rel}", self.roots[side.slot()])
    }

    fn tags(&self, rows: &[(Side, &str)]) -> Vec<String> {
        rows.iter()
            .map(|(side, rel)| self.tag(*side, rel))
            .collect()
    }

    /// Every group of both repos, tagged, in build order.
    fn every_group(&self) -> Vec<String> {
        Side::BOTH
            .iter()
            .flat_map(|&side| side.groups().iter().map(move |g| self.tag(side, g)))
            .collect()
    }

    fn open(&self) -> WatchSession {
        WatchSession::open_many(&self.roots, &BuildOptions::default()).expect("open_many")
    }

    /// The clean joint build of the trees as they are.
    fn clean(&self) -> GenerateResult {
        generate_many_opts(&self.roots, false, &BuildOptions::default()).expect("clean build")
    }

    fn out(&self, at: &str) -> PathBuf {
        self.tmp.path().join("out").join(at)
    }
}

// ---- the checks -------------------------------------------------------------

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

/// Checks 1 and 2 (module doc), both layouts persisted under `out/`, which
/// is removed afterwards.
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
    assert_eq!(w.repo_labels, clean.repo_labels, "{at}: repo_labels");
    assert_eq!(w.repo_roots, clean.repo_roots, "{at}: repo_roots");
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
            "{at}: {name} differs from a clean generate_many + persist"
        );
    }
    std::fs::remove_dir_all(out).expect("rm the step's layouts");
}

/// Check 3 (module doc). Returns whether the full delta moved anything.
fn assert_delta(u: &WatchUpdate, prev: &GenerateResult, clean: &GenerateResult, at: &str) -> bool {
    let (d, nodes, edges) = located_delta(&prev.merged, &clean.merged);
    assert_eq!(json(&u.nodes), json(&nodes), "{at}: node rows");
    assert_eq!(json(&u.edges), json(&edges), "{at}: edge rows");
    let c = &u.counts;
    assert_eq!(
        [
            c.nodes_added,
            c.nodes_removed,
            c.nodes_modified,
            c.nodes_moved,
            c.edges_added,
            c.edges_removed,
            c.edges_reconfidenced,
            c.moves_ignored
        ],
        [
            d.added_nodes.len(),
            d.removed_nodes.len(),
            d.modified_nodes.len(),
            d.moved_nodes.len(),
            d.added_edges.len(),
            d.removed_edges.len(),
            d.reconfidenced_edges.len(),
            d.ignored_moves
        ],
        "{at}: counts"
    );
    !nodes.is_empty() || !edges.is_empty()
}

/// The cross edges of each category whose two ends sit in different repos.
fn cross_repo(r: &GenerateResult) -> BTreeMap<u32, usize> {
    let mut repo_of = HashMap::new();
    for g in &r.merged.graphs {
        for n in &g.nodes {
            repo_of.insert(n.id, n.repo);
        }
    }
    let mut out = BTreeMap::new();
    for e in &r.merged.cross_edges {
        if let (Some(a), Some(b)) = (repo_of.get(&e.from), repo_of.get(&e.to))
            && a != b
        {
            *out.entry(e.category.0).or_default() += 1;
        }
    }
    out
}

/// The fixture holds what the gate is for (module doc): one cross-repo
/// HTTP_CALLS, GRPC_CALLS and SAME_AS each.
fn assert_fixture_shape(r: &GenerateResult) {
    let x = cross_repo(r);
    for (cat, name) in [
        (edge_category::HTTP_CALLS, "HTTP_CALLS"),
        (edge_category::GRPC_CALLS, "GRPC_CALLS"),
        (edge_category::SAME_AS, "SAME_AS"),
    ] {
        assert_eq!(x.get(&cat.0), Some(&1), "one cross-repo {name}: {x:?}");
    }
    assert_eq!(r.repo_labels.len(), 2, "two repos");
}

// ---- the scripted sequences ---------------------------------------------------

/// Which groups a step must rebuild.
enum Rebuilt {
    /// Exactly these, in build order.
    Exact(&'static [(Side, &'static str)]),
    /// Every group of both repos.
    Every,
    /// None of this side's (the other side's are not pinned).
    NoneOn(Side),
}

struct Expect {
    rebuilt: Rebuilt,
    full_rebuild: Option<&'static str>,
    added: &'static [(Side, &'static str)],
    modified: &'static [(Side, &'static str)],
    removed: &'static [(Side, &'static str)],
    /// `(change, qname)` node rows the update must hold.
    node_rows: &'static [(&'static str, &'static str)],
    /// `(change, category)` edge rows the update must hold.
    edge_rows: &'static [(&'static str, &'static str)],
}

const EXPECT: Expect = Expect {
    rebuilt: Rebuilt::Every,
    full_rebuild: None,
    added: &[],
    modified: &[],
    removed: &[],
    node_rows: &[],
    edge_rows: &[],
};

struct Step {
    name: &'static str,
    edit: fn(&Fixture),
    /// The paths handed to `apply`, made absolute against their repo.
    paths: &'static [(Side, &'static str)],
    expect: Expect,
}

/// Run `steps` on a fresh fixture and session.
fn run(sequence: &str, steps: &[Step]) {
    let fx = Fixture::new();
    let mut s = fx.open();
    assert_eq!(
        s.roots(),
        fx.roots.iter().map(String::as_str).collect::<Vec<_>>(),
        "{sequence}: the `[watch] open repos=2` session"
    );
    assert_eq!(
        s.memory().groups,
        SERVER_GROUPS.len() + CLIENT_GROUPS.len(),
        "{sequence}: the plan's groups"
    );
    let mut prev = fx.clean();
    assert_fixture_shape(&prev);
    assert_parity(&s, &prev, &fx.out("open"), &format!("{sequence}/open"));
    for (i, step) in steps.iter().enumerate() {
        let at = format!("{sequence}/{}", step.name);
        (step.edit)(&fx);
        let seq = s.seq();
        let rank_before = s.rank_cache();
        let paths: Vec<PathBuf> = step
            .paths
            .iter()
            .map(|(side, rel)| fx.dir(*side).join(rel))
            .collect();
        let u = s
            .apply(&paths)
            .unwrap_or_else(|e| panic!("{at}: apply: {e}"));
        assert_eq!((u.seq, s.seq()), (seq + 1, seq + 1), "{at}: seq");
        assert!(
            !Arc::ptr_eq(&rank_before, &s.rank_cache()),
            "{at}: an update that changed the graph replaces the rank cache"
        );
        let clean = fx.clean();
        assert_parity(&s, &clean, &fx.out(&format!("{i}-{}", step.name)), &at);
        assert!(assert_delta(&u, &prev, &clean, &at), "{at}: rows move");
        let e = &step.expect;
        match e.rebuilt {
            Rebuilt::Exact(groups) => {
                assert_eq!(u.groups_rebuilt, fx.tags(groups), "{at}: groups rebuilt")
            }
            Rebuilt::Every => {
                assert_eq!(u.groups_rebuilt, fx.every_group(), "{at}: every group")
            }
            Rebuilt::NoneOn(side) => {
                let prefix = fx.tag(side, "");
                assert!(
                    !u.groups_rebuilt.iter().any(|g| g.starts_with(&prefix)),
                    "{at}: no {} group rebuilt: {:?}",
                    side.name(),
                    u.groups_rebuilt
                );
            }
        }
        assert_eq!(
            u.full_rebuild.as_deref(),
            e.full_rebuild,
            "{at}: full_rebuild"
        );
        assert_eq!(u.files.added, fx.tags(e.added), "{at}: files added");
        assert_eq!(
            u.files.modified,
            fx.tags(e.modified),
            "{at}: files modified"
        );
        assert_eq!(u.files.removed, fx.tags(e.removed), "{at}: files removed");
        for (change, qname) in e.node_rows {
            assert!(
                u.nodes
                    .iter()
                    .any(|n| n.change == *change && n.qname == *qname),
                "{at}: a {change} {qname} row: {:?}",
                u.nodes
            );
        }
        for (change, category) in e.edge_rows {
            assert!(
                u.edges
                    .iter()
                    .any(|r| r.change == *change && r.category == *category),
                "{at}: a {change} {category} edge row: {:?}",
                u.edges
            );
        }
        prev = clean;
    }
    // The steps leave the fixture's cross-repo shape where it started or
    // moved it on purpose; either way the session still equals a clean build
    // after a quiet poll.
    assert!(
        s.poll().expect("poll").is_none(),
        "{sequence}: quiet after the steps"
    );
}

fn client_body(fx: &Fixture) {
    replace(
        fx.dir(Side::Client),
        "rpc/hi.go",
        "\tc.Hi(",
        "\tc.Hi(context.TODO(), nil)\n\tc.Hi(",
    );
}

/// (a) A body edit in the client rebuilds only the client's go group.
#[test]
fn scripted_body_client() {
    run(
        "body_client",
        &[Step {
            name: "client_body",
            edit: client_body,
            paths: &[(Side::Client, "rpc/hi.go")],
            expect: Expect {
                rebuilt: Rebuilt::Exact(&[(Side::Client, "go")]),
                modified: &[(Side::Client, "rpc/hi.go")],
                ..EXPECT
            },
        }],
    );
}

fn proto_rpc_add(fx: &Fixture) {
    replace(
        fx.dir(Side::Server),
        PROTO,
        "  rpc Hi (HiRequest) returns (HiReply);\n",
        "  rpc Hi (HiRequest) returns (HiReply);\n  rpc Bye (HiRequest) returns (HiReply);\n",
    );
}

fn proto_service_add(fx: &Fixture) {
    let text = read(fx.dir(Side::Server), PROTO);
    write(fx.dir(Side::Server), PROTO, &format!("{text}{FAREWELL}"));
}

/// (b) The proto service set is the union of both repos: an rpc added to the
/// server's proto, then a service, is `proto_services` and rebuilds every
/// group of both repos; the new service's client stub in the client repo
/// becomes a GRPC_CLIENT and a cross-repo GRPC_CALLS.
#[test]
fn scripted_proto() {
    run(
        "proto",
        &[
            Step {
                name: "rpc_add",
                edit: proto_rpc_add,
                paths: &[(Side::Server, PROTO)],
                expect: Expect {
                    full_rebuild: Some("proto_services"),
                    modified: &[(Side::Server, PROTO)],
                    node_rows: &[("added", "grpc:greet.Greeter::Bye")],
                    ..EXPECT
                },
            },
            Step {
                name: "service_add",
                edit: proto_service_add,
                paths: &[(Side::Server, PROTO)],
                expect: Expect {
                    full_rebuild: Some("proto_services"),
                    modified: &[(Side::Server, PROTO)],
                    node_rows: &[
                        ("added", "grpc:greet.Farewell"),
                        ("added", "grpc_client:Farewell"),
                    ],
                    edge_rows: &[("added", "GRPC_CALLS")],
                    ..EXPECT
                },
            },
        ],
    );
}

fn route_move(fx: &Fixture) {
    replace(
        fx.dir(Side::Server),
        "web/orders.go",
        "\"/api/orders\"",
        "\"/api/v2/orders\"",
    );
}

fn route_back(fx: &Fixture) {
    replace(
        fx.dir(Side::Server),
        "web/orders.go",
        "\"/api/v2/orders\"",
        "\"/api/orders\"",
    );
}

/// (c) The server's route path moves away from the client's URL and back:
/// the cross-repo HTTP_CALLS is removed, then added again.
#[test]
fn scripted_http_route() {
    let expect = |edge: &'static [(&'static str, &'static str)]| Expect {
        rebuilt: Rebuilt::Exact(&[(Side::Server, "go")]),
        modified: &[(Side::Server, "web/orders.go")],
        edge_rows: edge,
        ..EXPECT
    };
    run(
        "http_route",
        &[
            Step {
                name: "route_move",
                edit: route_move,
                paths: &[(Side::Server, "web/orders.go")],
                expect: expect(&[("removed", "HTTP_CALLS")]),
            },
            Step {
                name: "route_back",
                edit: route_back,
                paths: &[(Side::Server, "web/orders.go")],
                expect: expect(&[("added", "HTTP_CALLS")]),
            },
        ],
    );
}

fn require_other(fx: &Fixture) {
    replace(
        fx.dir(Side::Client),
        "go.mod",
        REQUIRE,
        "require example.com/other v0.1.0",
    );
}

fn require_server(fx: &Fixture) {
    replace(
        fx.dir(Side::Client),
        "go.mod",
        "require example.com/other v0.1.0",
        REQUIRE,
    );
}

/// (d) The client's `go.mod` require (a High input in the client only, its
/// module line unchanged, so no `go_modules`): the cross-repo
/// `manifest_name` SAME_AS goes with the server dependency and comes back
/// with it, and no server group rebuilds.
#[test]
fn scripted_go_mod_require() {
    let expect = |edge: &'static [(&'static str, &'static str)]| Expect {
        rebuilt: Rebuilt::NoneOn(Side::Server),
        modified: &[(Side::Client, "go.mod")],
        edge_rows: edge,
        ..EXPECT
    };
    run(
        "go_mod_require",
        &[
            Step {
                name: "require_other",
                edit: require_other,
                paths: &[(Side::Client, "go.mod")],
                expect: expect(&[("removed", "SAME_AS")]),
            },
            Step {
                name: "require_server",
                edit: require_server,
                paths: &[(Side::Client, "go.mod")],
                expect: expect(&[("added", "SAME_AS")]),
            },
        ],
    );
}

const EXTRA: &str = "extra/extra.go";

fn server_file_add(fx: &Fixture) {
    write(
        fx.dir(Side::Server),
        EXTRA,
        "package extra\n\nfunc Extra() int {\n\treturn 1\n}\n",
    );
}

fn server_file_delete(fx: &Fixture) {
    std::fs::remove_file(fx.dir(Side::Server).join(EXTRA)).expect("rm");
}

/// (e) A file added to the server in a new directory, then deleted.
#[test]
fn scripted_add_delete() {
    run(
        "add_delete",
        &[
            Step {
                name: "add",
                edit: server_file_add,
                paths: &[(Side::Server, EXTRA)],
                expect: Expect {
                    rebuilt: Rebuilt::NoneOn(Side::Client),
                    added: &[(Side::Server, EXTRA)],
                    node_rows: &[("added", "extra::extra::Extra")],
                    ..EXPECT
                },
            },
            Step {
                name: "delete",
                edit: server_file_delete,
                paths: &[(Side::Server, EXTRA)],
                expect: Expect {
                    rebuilt: Rebuilt::NoneOn(Side::Client),
                    removed: &[(Side::Server, EXTRA)],
                    node_rows: &[("removed", "extra::extra::Extra")],
                    ..EXPECT
                },
            },
        ],
    );
}

fn both_bodies(fx: &Fixture) {
    replace(
        fx.dir(Side::Server),
        "rpc/greeter.go",
        "\tpb.RegisterGreeterServer(",
        "\t_ = srv\n\tpb.RegisterGreeterServer(",
    );
    replace(
        fx.dir(Side::Client),
        "web/orders.ts",
        "  return res.json();",
        "  console.log(res);\n  return res.json();",
    );
}

/// (f) One update naming paths in both repos rebuilds exactly the two
/// groups, server first.
#[test]
fn scripted_two_repos() {
    run(
        "two_repos",
        &[Step {
            name: "both",
            edit: both_bodies,
            paths: &[
                (Side::Client, "web/orders.ts"),
                (Side::Server, "rpc/greeter.go"),
            ],
            expect: Expect {
                rebuilt: Rebuilt::Exact(&[(Side::Server, "go"), (Side::Client, "typescript")]),
                modified: &[
                    (Side::Server, "rpc/greeter.go"),
                    (Side::Client, "web/orders.ts"),
                ],
                ..EXPECT
            },
        }],
    );
}

fn overlay_add(fx: &Fixture) {
    write(
        fx.dir(Side::Client),
        OVERLAY,
        "version = 1\n\n[constants]\nGATEWAY = \"/gw\"\n",
    );
}

/// (g) A `.glia/overlay.toml` added to the client (no server overlay): an
/// `overlay` full rebuild whose constant folds the gateway endpoint. GA.17:
/// the group key holds no overlay, so only the client's `typescript` group,
/// whose post-graft parse the pin moves, rebuilds; the client's other groups
/// and every server group are reused.
#[test]
fn scripted_overlay() {
    run(
        "overlay",
        &[Step {
            name: "overlay_add",
            edit: overlay_add,
            paths: &[(Side::Client, OVERLAY)],
            expect: Expect {
                rebuilt: Rebuilt::Exact(&[(Side::Client, "typescript")]),
                full_rebuild: Some("overlay"),
                node_rows: &[("added", "endpoint:GET:/gw/ping")],
                ..EXPECT
            },
        }],
    );
}

// ---- the random sequences -------------------------------------------------------

/// Vigna's SplitMix64 (watch_parity.rs's): every choice a random sequence
/// makes hangs off it (the candidate lists it indexes are sorted).
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

/// One random edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Op {
    /// A statement as the first line of the last function of one of the
    /// side's Go / TS files.
    Body(Side),
    /// A new function appended to one of the side's Go / TS files.
    Decl(Side),
    /// A new Go file (in a new directory the first time), or on the client
    /// sometimes a TS file.
    Add(Side),
    /// Delete a file the sequence added on that side (an add when it has
    /// none).
    Remove(Side),
    /// The server proto's `Farewell` service, added or removed.
    Proto,
    /// The client `go.mod`'s required version, toggled.
    GoMod,
    /// `File::set_modified` on a file of either side, bytes unchanged.
    Touch,
}

impl Op {
    /// One [`World::pick`] bag: every op once, the body edits and the proto
    /// twice; 15 cards, one default sequence.
    const BAG: [Op; 15] = [
        Op::Body(Side::Server),
        Op::Body(Side::Client),
        Op::Body(Side::Server),
        Op::Body(Side::Client),
        Op::Decl(Side::Server),
        Op::Decl(Side::Client),
        Op::Add(Side::Server),
        Op::Add(Side::Client),
        Op::Remove(Side::Server),
        Op::Remove(Side::Client),
        Op::Proto,
        Op::Proto,
        Op::GoMod,
        Op::Touch,
        Op::Touch,
    ];

    fn name(self) -> String {
        match self {
            Op::Body(s) => format!("body_{}", s.name()),
            Op::Decl(s) => format!("decl_{}", s.name()),
            Op::Add(s) => format!("add_{}", s.name()),
            Op::Remove(s) => format!("remove_{}", s.name()),
            Op::Proto => "proto".to_string(),
            Op::GoMod => "go_mod".to_string(),
            Op::Touch => "touch".to_string(),
        }
    }
}

/// `rel`'s extension (`""` for none).
fn ext(rel: &str) -> &str {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.rsplit_once('.').map_or("", |(_, e)| e)
}

/// The language group a changed source path rebuilds (`None` for any other
/// input).
fn group_of(rel: &str) -> Option<&'static str> {
    match ext(rel) {
        "go" => Some("go"),
        "ts" => Some("typescript"),
        "proto" => Some("proto"),
        _ => None,
    }
}

/// Every file under `dir`, with its bytes.
fn tree_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
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
                out.insert(rel, std::fs::read(&p).expect("read"));
            }
        }
    }
    out
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

/// A random sequence's fixture and generator; ops read and write the trees
/// themselves, the oracle reads byte snapshots of both.
struct World {
    fx: Fixture,
    rng: SplitMix64,
    /// Files the sequence added and still has, per side.
    added: [BTreeSet<String>; 2],
    bag: Vec<Op>,
}

impl World {
    fn pick(&mut self) -> Op {
        if self.bag.is_empty() {
            self.bag = Op::BAG.to_vec();
        }
        let i = self.rng.below(self.bag.len());
        self.bag.swap_remove(i)
    }

    fn choose(&mut self, xs: &[String]) -> String {
        xs[self.rng.below(xs.len())].clone()
    }

    /// The side's Go / TS files, sorted.
    fn sources(&self, side: Side) -> Vec<String> {
        tree_bytes(self.fx.dir(side))
            .into_keys()
            .filter(|r| matches!(ext(r), "go" | "ts") && !r.starts_with(".glia/"))
            .collect()
    }

    /// Run `op` for step `i`; returns the op as run (a remove with nothing
    /// to remove runs as an add) and the paths it touched, by side.
    fn run(&mut self, op: Op, i: usize, stamp: SystemTime) -> (Op, Vec<(Side, String)>) {
        match op {
            Op::Body(side) => {
                let files = self.sources(side);
                let rel = self.choose(&files);
                let dir = self.fx.dir(side);
                let text = read(dir, &rel);
                let at = text.rfind("{\n").expect("a function body") + 2;
                let stmt = if ext(&rel) == "go" {
                    format!("\t_ = {i}\n")
                } else {
                    format!("  void {i};\n")
                };
                write(dir, &rel, &format!("{}{stmt}{}", &text[..at], &text[at..]));
                (op, vec![(side, rel)])
            }
            Op::Decl(side) => {
                let files = self.sources(side);
                let rel = self.choose(&files);
                let dir = self.fx.dir(side);
                let decl = if ext(&rel) == "go" {
                    format!("\nfunc Decl{i}() int {{\n\treturn {i}\n}}\n")
                } else {
                    format!("\nexport function decl{i}(): number {{\n  return {i};\n}}\n")
                };
                write(dir, &rel, &format!("{}{decl}", read(dir, &rel)));
                (op, vec![(side, rel)])
            }
            Op::Remove(side) if !self.added[side.slot()].is_empty() => {
                let xs: Vec<String> = self.added[side.slot()].iter().cloned().collect();
                let rel = self.choose(&xs);
                std::fs::remove_file(self.fx.dir(side).join(&rel)).expect("rm");
                self.added[side.slot()].remove(&rel);
                (op, vec![(side, rel)])
            }
            Op::Add(side) | Op::Remove(side) => {
                let ts = side == Side::Client && self.rng.next() % 2 == 1;
                let (rel, body) = if ts {
                    (
                        format!("web/f{i}.ts"),
                        format!("export function f{i}(): number {{\n  return {i};\n}}\n"),
                    )
                } else {
                    (
                        format!("extra/f{i}.go"),
                        format!("package extra\n\nfunc F{i}() int {{\n\treturn {i}\n}}\n"),
                    )
                };
                write(self.fx.dir(side), &rel, &body);
                self.added[side.slot()].insert(rel.clone());
                (Op::Add(side), vec![(side, rel)])
            }
            Op::Proto => {
                let dir = self.fx.dir(Side::Server);
                let text = read(dir, PROTO);
                let next = match text.strip_suffix(FAREWELL) {
                    Some(base) => base.to_string(),
                    None => format!("{text}{FAREWELL}"),
                };
                write(dir, PROTO, &next);
                (op, vec![(Side::Server, PROTO.to_string())])
            }
            Op::GoMod => {
                let dir = self.fx.dir(Side::Client);
                let text = read(dir, "go.mod");
                let next = if text.contains(REQUIRE) {
                    text.replacen(REQUIRE, "require example.com/server v0.2.0", 1)
                } else {
                    text.replacen("require example.com/server v0.2.0", REQUIRE, 1)
                };
                write(dir, "go.mod", &next);
                (op, vec![(Side::Client, "go.mod".to_string())])
            }
            Op::Touch => {
                let side = Side::BOTH[self.rng.below(2)];
                let files: Vec<String> = tree_bytes(self.fx.dir(side)).into_keys().collect();
                let rel = self.choose(&files);
                std::fs::File::open(self.fx.dir(side).join(&rel))
                    .and_then(|f| f.set_modified(stamp))
                    .expect("mtime");
                (op, vec![(side, rel)])
            }
        }
    }
}

/// What the session must report for one update, from byte snapshots of
/// both trees before and after it: whether anything changed (else a noop:
/// `seq` stays, no rows), the table entries added / removed / modified per
/// repo (each sorted, the server's first, `<label>:`-tagged), the full
/// rebuild reason (`proto_services` when the proto moved: every proto op
/// toggles a service), and the groups: every group under a proto change,
/// exactly the edited files' groups when every change is an in-place Go /
/// TS edit, else at least the changed sources' groups; never a group of a
/// repo with no change.
struct Want {
    changed: bool,
    added: Vec<String>,
    removed: Vec<String>,
    modified: Vec<String>,
    full_rebuild: Option<&'static str>,
    rebuilt_exact: Option<Vec<String>>,
    rebuilt_contains: Vec<String>,
    quiet_sides: Vec<Side>,
}

type Snapshot = [BTreeMap<String, Vec<u8>>; 2];

impl Want {
    fn of(fx: &Fixture, before: &Snapshot, after: &Snapshot) -> Self {
        let mut want = Want {
            changed: false,
            added: Vec::new(),
            removed: Vec::new(),
            modified: Vec::new(),
            full_rebuild: None,
            rebuilt_exact: None,
            rebuilt_contains: Vec::new(),
            quiet_sides: Vec::new(),
        };
        let mut every_in_place = true;
        let mut exact = Vec::new();
        for side in Side::BOTH {
            let (b, a) = (&before[side.slot()], &after[side.slot()]);
            let changed: BTreeSet<&str> = b
                .keys()
                .chain(a.keys())
                .filter(|k| b.get(*k) != a.get(*k))
                .map(String::as_str)
                .collect();
            if changed.is_empty() {
                want.quiet_sides.push(side);
                continue;
            }
            want.changed = true;
            for k in &changed {
                let tagged = fx.tag(side, k);
                match (b.contains_key(*k), a.contains_key(*k)) {
                    (false, true) => want.added.push(tagged),
                    (true, false) => want.removed.push(tagged),
                    _ => want.modified.push(tagged),
                }
                if *k == PROTO {
                    want.full_rebuild = Some("proto_services");
                }
                if !(b.contains_key(*k) && a.contains_key(*k) && matches!(ext(k), "go" | "ts")) {
                    every_in_place = false;
                }
            }
            let groups: BTreeSet<&str> = changed.iter().filter_map(|k| group_of(k)).collect();
            for g in side.groups().iter().filter(|g| groups.contains(*g)) {
                exact.push(fx.tag(side, g));
            }
        }
        want.rebuilt_exact = if want.full_rebuild.is_some() {
            Some(fx.every_group())
        } else if every_in_place {
            Some(exact.clone())
        } else {
            None
        };
        want.rebuilt_contains = exact;
        want
    }

    fn check(
        &self,
        fx: &Fixture,
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
        assert_eq!(u.files.added, self.added, "{at}: files added");
        assert_eq!(u.files.removed, self.removed, "{at}: files removed");
        assert_eq!(u.files.modified, self.modified, "{at}: files modified");
        assert_eq!(
            u.full_rebuild.as_deref(),
            self.full_rebuild,
            "{at}: full_rebuild"
        );
        if let Some(exact) = &self.rebuilt_exact {
            assert_eq!(&u.groups_rebuilt, exact, "{at}: groups rebuilt");
        }
        for g in &self.rebuilt_contains {
            assert!(
                u.groups_rebuilt.contains(g),
                "{at}: {g} rebuilt: {:?}",
                u.groups_rebuilt
            );
        }
        if self.full_rebuild.is_none() {
            for side in &self.quiet_sides {
                let prefix = fx.tag(*side, "");
                assert!(
                    !u.groups_rebuilt.iter().any(|g| g.starts_with(&prefix)),
                    "{at}: the untouched {} rebuilt nothing: {:?}",
                    side.name(),
                    u.groups_rebuilt
                );
            }
        }
    }
}

/// What a run covered.
#[derive(Default)]
struct Tally {
    ops: BTreeMap<String, usize>,
    polls: usize,
    applies: usize,
    noops: usize,
}

/// One seed's sequence of `steps` ops on a fresh fixture and session.
fn random_sequence(seed: u64, steps: usize, tally: &mut Tally) {
    let fx = Fixture::new();
    for dir in &fx.dirs {
        settle(dir);
    }
    // Past every settled stamp, an hour behind the clock: step `i` stamps
    // `base + i + 1` s, never a value an earlier stamp or write left.
    let base = settled();
    let mut s = fx.open();
    let mut prev = fx.clean();
    assert_parity(&s, &prev, &fx.out("open"), &format!("seed={seed} open"));
    let mut last: Snapshot = Side::BOTH.map(|side| tree_bytes(fx.dir(side)));
    let mut w = World {
        fx,
        rng: SplitMix64(seed),
        added: [BTreeSet::new(), BTreeSet::new()],
        bag: Vec::new(),
    };
    for i in 0..steps {
        let op = w.pick();
        let stamp = base + Duration::from_secs(i as u64 + 1);
        let (op, paths) = w.run(op, i, stamp);
        let poll = w.rng.next() % 2 == 1;
        let names: Vec<String> = paths
            .iter()
            .map(|(side, rel)| w.fx.tag(*side, rel))
            .collect();
        let at = format!(
            "seed={seed} step={i} op={} path={} via={}",
            op.name(),
            names.join(","),
            if poll { "poll" } else { "apply" }
        );
        eprintln!("[watch-parity-many] {at}");
        let now: Snapshot = Side::BOTH.map(|side| tree_bytes(w.fx.dir(side)));
        let want = Want::of(&w.fx, &last, &now);
        let seq = s.seq();
        let rank_before = s.rank_cache();
        let update = if poll {
            for (side, rel) in &paths {
                if let Ok(f) = std::fs::File::open(w.fx.dir(*side).join(rel)) {
                    f.set_modified(stamp).expect("mtime");
                }
            }
            tally.polls += 1;
            s.poll().unwrap_or_else(|e| panic!("{at}: poll: {e}"))
        } else {
            let named: Vec<PathBuf> = paths
                .iter()
                .map(|(side, rel)| w.fx.dir(*side).join(rel))
                .collect();
            tally.applies += 1;
            Some(
                s.apply(&named)
                    .unwrap_or_else(|e| panic!("{at}: apply: {e}")),
            )
        };
        assert_eq!(
            Arc::ptr_eq(&rank_before, &s.rank_cache()),
            !want.changed,
            "{at}: the rank cache is kept exactly when the update is a noop"
        );
        let clean = w.fx.clean();
        assert_parity(&s, &clean, &w.fx.out(&format!("{seed}-{i}")), &at);
        match &update {
            Some(u) => want.check(&w.fx, u, seq, &prev, &clean, &at),
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
        *tally.ops.entry(op.name()).or_default() += 1;
        prev = clean;
        last = now;
    }
}

/// Seeds `1..=3` x 15 steps (the spec's op table, every op in each seed's
/// bag), both routes.
#[test]
fn random_sequences() {
    let mut tally = Tally::default();
    for seed in 1..=3 {
        random_sequence(seed, 15, &mut tally);
    }
    eprintln!(
        "[watch-parity-many] random seeds=3 steps=15 ops={:?} applies={} polls={} noops={}",
        tally.ops, tally.applies, tally.polls, tally.noops
    );
    for op in [
        Op::Body(Side::Server),
        Op::Body(Side::Client),
        Op::Decl(Side::Server),
        Op::Decl(Side::Client),
        Op::Add(Side::Server),
        Op::Add(Side::Client),
        Op::Remove(Side::Server),
        Op::Remove(Side::Client),
        Op::Proto,
        Op::GoMod,
        Op::Touch,
    ] {
        assert!(
            tally.ops.contains_key(&op.name()),
            "seeds 1..=3 never ran {}: {:?}",
            op.name(),
            tally.ops
        );
    }
    assert!(
        tally.polls > 0 && tally.applies > 0 && tally.noops > 0,
        "both routes and a noop: applies={} polls={} noops={}",
        tally.applies,
        tally.polls,
        tally.noops
    );
}
