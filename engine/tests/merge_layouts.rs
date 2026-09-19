//! LC.10b: merge pre-built layouts into the graph one build of every member
//! would give (`glia_engine::merge`).
//!
//! Measured before LC.10b: there was no layout-level merge. `glia merge` over
//! two layout dirs walks them as source trees and fails with
//! `error: no graphs produced from 2 paths; first error: `, and a web-only
//! layout's `endpoint:GET:/users` is Medium where the joint build has it
//! Strong, so concatenating layouts cannot reproduce the joint build.
//!
//! Stderr markers are asserted from a child run of this test binary
//! ([`child_merge_for_stderr`]), the channel_owner_rpc_event.rs pattern.

use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::edge_category;
use glia_code_domain::evidence::Evidence;
use glia_core::{Confidence, Edge, Node, NodeId, NodeKindId, RepoId};
use glia_engine::merge::{
    ForeignShard, MergeMember, MergeResult, merge_layouts, persist_merge, read_workspace,
};
use glia_engine::persist::{layout_meta, load_layout, persist_layout, persist_result};
use glia_engine::{GenerateResult, generate_many};
use glia_store::{
    Container, Header, MANIFEST_NAME, encode_section, inspect_path, read_layout_extras,
    read_merged_sharded_meta, write_container, write_merged_sharded_extras,
};

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// A Flask route in `api/` (with a README naming a web symbol) and a
/// TypeScript client fetching it in `web/`.
fn api_web(tmp: &Path) -> (PathBuf, PathBuf) {
    let api = tmp.join("api");
    let web = tmp.join("web");
    write(
        &api,
        "app.py",
        "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/users\")\n\
         def list_users():\n    return []\n",
    );
    write(&api, "README.md", "# API\n\nThe web client calls `loadUsers` to list users.\n");
    write(
        &web,
        "client.ts",
        "export async function loadUsers() {\n  const res = await fetch(\"/users\");\n  \
         return res.json();\n}\n",
    );
    (api, web)
}

fn build(repos: &[&Path]) -> GenerateResult {
    let paths: Vec<String> = repos.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    generate_many(&paths).unwrap()
}

/// `generate_many(repos)` persisted to `dir` by the one writer.
fn layout(repos: &[&Path], dir: &Path) -> GenerateResult {
    let r = build(repos);
    persist_result(&r, dir, "test").unwrap();
    r
}

fn gmap(name: &str, dir: &Path) -> MergeMember {
    read_one(&format!(r#"{{"name":"{name}","gmap":"{}"}}"#, dir.display()))
}

fn repo(name: &str, root: &Path) -> MergeMember {
    read_one(&format!(r#"{{"name":"{name}","repo":"{}"}}"#, root.display()))
}

/// One member through the workspace reader (absolute paths), so the tests
/// build members the way a caller outside the crate does.
fn read_one(member: &str) -> MergeMember {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("glia.workspace.json");
    std::fs::write(&path, format!(r#"{{"version":1,"members":[{member}]}}"#)).unwrap();
    read_workspace(&path).unwrap().remove(0)
}

/// Every `.gmap` file directly in `dir`, by name.
fn gmaps(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".gmap"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

fn manifest(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST_NAME)).unwrap()).unwrap()
}

/// The merged layout at `m` holds exactly the joint build's bytes at `t`:
/// every `.gmap` byte-identical, the manifest equal once `members` is gone.
fn assert_same_layout(m: &Path, t: &Path) {
    let (gm, gt) = (gmaps(m), gmaps(t));
    let names = |g: &[(String, Vec<u8>)]| g.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    assert_eq!(names(&gm), names(&gt), "shard sets differ");
    for ((name, bm), (_, bt)) in gm.iter().zip(&gt) {
        assert!(bm == bt, "{name}: merged bytes differ from the joint build's");
    }
    let mut mm = manifest(m);
    assert!(mm.get("members").is_some(), "the merged manifest records its members: {mm}");
    mm.as_object_mut().unwrap().remove("members");
    assert_eq!(mm, manifest(t));
}

fn count(r: &MergeResult, pred: impl Fn(&Edge) -> bool) -> usize {
    r.result.merged.all_edges().filter(|e| pred(e)).count()
}

// ---------------------------------------------------------------------------
// Child run: the markers go to stderr, so a parent re-runs this binary.
// ---------------------------------------------------------------------------

const CHILD_ENV: &str = "LC10B_MERGE_WORKSPACE";

/// Merges the workspace named by [`CHILD_ENV`] and prints nothing else; a
/// no-op in a normal run.
#[test]
fn child_merge_for_stderr() {
    let Ok(ws) = std::env::var(CHILD_ENV) else { return };
    let members = read_workspace(Path::new(&ws)).unwrap();
    merge_layouts(&members).unwrap();
}

/// The `[merge] ...` / `[gmap] skipped ...` stderr lines of merging `members`
/// (`(name, layout dir)`), from a child process.
fn merge_stderr(tmp: &Path, members: &[(&str, &Path)]) -> Vec<String> {
    let list: Vec<String> = members
        .iter()
        .map(|(n, d)| format!(r#"{{"name":"{n}","gmap":"{}"}}"#, d.display()))
        .collect();
    let ws = tmp.join("child.workspace.json");
    std::fs::write(&ws, format!(r#"{{"version":1,"members":[{}]}}"#, list.join(","))).unwrap();
    let out = Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--exact", "child_merge_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, &ws)
        .output()
        .expect("re-run the test binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "child merge failed:\n{stderr}");
    stderr
        .lines()
        .filter(|l| l.starts_with("[merge]") || l.starts_with("[gmap] skipped"))
        .map(String::from)
        .collect()
}

// ---------------------------------------------------------------------------

/// The acceptance test: layouts built one repo at a time merge into the bytes
/// of building them together. Pre-LC.10a/b it cannot hold: the web-only
/// layout's endpoint was demoted (no route in its build), and the doc link
/// from api's README to `loadUsers` exists only when web is present.
#[test]
fn merge_equals_building_together() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = api_web(tmp.path());
    let (a, b, t) = (tmp.path().join("out_a"), tmp.path().join("out_b"), tmp.path().join("out_t"));
    layout(&[&api], &a);
    let web_only = layout(&[&web], &b);
    assert!(!web_only.merged.pass_undo.is_empty(), "control: web alone demotes its endpoint");
    let together = layout(&[&api, &web], &t);
    assert!(
        together.merged.cross_edges.iter().any(|e| e.category == edge_category::DOCUMENTS),
        "control: the joint build links api's README to web's loadUsers"
    );

    let m = merge_layouts(&[gmap("api", &a), gmap("web", &b)]).unwrap();
    assert!(m.caveats.is_empty(), "{:?}", m.caveats);
    assert!(m.foreign.is_empty());
    assert_eq!(m.result.repo_labels, together.repo_labels);
    assert_eq!(m.result.parse_errors, together.parse_errors);
    assert_eq!(m.result.total_nodes, together.total_nodes);
    assert_eq!(m.result.total_edges, together.total_edges);
    let out_m = tmp.path().join("out_m");
    persist_merge(&m, &out_m, "test").unwrap();
    assert_same_layout(&out_m, &t);
    assert_eq!(
        manifest(&out_m)["members"],
        serde_json::json!([
            {"name": "api", "source": "gmap", "build_stamp": glia_engine::BUILD_STAMP},
            {"name": "web", "source": "gmap", "build_stamp": glia_engine::BUILD_STAMP},
        ])
    );

    // A source member is built into its own default layout first (LC.8), then
    // merged the same way.
    let m2 = merge_layouts(&[gmap("api", &a), repo("web", &web)]).unwrap();
    assert!(web.join(".glia/graph").join(MANIFEST_NAME).is_file(), "the repo member was persisted");
    let out_m2 = tmp.path().join("out_m2");
    persist_merge(&m2, &out_m2, "test").unwrap();
    assert_same_layout(&out_m2, &t);
    assert_eq!(manifest(&out_m2)["members"][1]["source"], "repo");

    // A re-merge into the same dir with one member leaves no shard of the
    // other behind.
    persist_merge(&merge_layouts(&[gmap("api", &a)]).unwrap(), &out_m2, "test").unwrap();
    assert_eq!(gmaps(&out_m2).len(), gmaps(&a).len(), "{:?}", gmaps(&out_m2).len());

    let lines = merge_stderr(tmp.path(), &[("api", &a), ("web", &b)]);
    let marker = lines.iter().find(|l| l.starts_with("[merge] members=")).expect("marker");
    let want = format!(
        "[merge] members=2 (gmap=2 repo=0) repos=2 code_shards={} foreign_shards=0 cross_edges={} \
         kept_member_edges=0 labels_stored=0",
        together.merged.graphs.len(),
        together.merged.cross_edges.len()
    );
    assert_eq!(marker, &want);
}

/// A5.2's inexact spot, reported: the client member was built without the
/// server member's proto services, so its gRPC client needles never saw them.
#[test]
fn grpc_caveat_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("server");
    let client = tmp.path().join("client");
    write(
        &server,
        "proto/greeter.proto",
        "syntax = \"proto3\";\npackage hello;\nservice Greeter {\n  rpc SayHello (HelloRequest) \
         returns (HelloReply);\n}\nmessage HelloRequest { string name = 1; }\nmessage HelloReply \
         { string message = 1; }\n",
    );
    write(
        &client,
        "main.py",
        "import grpc\nimport greeter_pb2_grpc\n\n\ndef hello():\n    stub = \
         greeter_pb2_grpc.GreeterStub(grpc.insecure_channel(\"x\"))\n    return stub.SayHello(None)\n",
    );
    let (s, c) = (tmp.path().join("out_s"), tmp.path().join("out_c"));
    layout(&[&server], &s);
    layout(&[&client], &c);

    let m = merge_layouts(&[gmap("server", &s), gmap("client", &c)]).unwrap();
    assert_eq!(
        m.caveats,
        vec![
            "grpc client needles are per-member: 1 proto services from other members were not \
             applied to member 'client'"
                .to_string()
        ]
    );
    let lines = merge_stderr(tmp.path(), &[("server", &s), ("client", &c)]);
    assert!(
        lines.iter().any(|l| l.starts_with("[merge] caveat: grpc client needles are per-member")),
        "{lines:?}"
    );
}

/// Two layouts built separately from one checkout identity share a RepoId
/// (LB.1): never fused, named instead.
#[test]
fn repo_id_collision_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, _web) = api_web(tmp.path());
    let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
    layout(&[&api], &one);
    layout(&[&api], &two);
    let err = match merge_layouts(&[gmap("first", &one), gmap("second", &two)]) {
        Ok(_) => panic!("a shared RepoId must not merge"),
        Err(e) => e,
    };
    assert!(err.contains("'first'") && err.contains("'second'"), "{err}");
    assert!(err.contains("both hold repo"), "{err}");

    // Bad member lists are refused before anything is read.
    let dup = merge_layouts(&[gmap("x", &one), gmap("x", &two)]).err().unwrap_or_default();
    assert!(dup.contains("two members are named 'x'"), "{dup}");
    assert!(merge_layouts(&[]).is_err());
}

/// A shard of another domain rides the merge: renamed `<member>-<name>`,
/// bytes untouched, listed in the merged manifest with its graph_type and the
/// same content hash; readers skip it as code and report it.
#[test]
fn foreign_shard_passes_through() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = api_web(tmp.path());
    let (a, c) = (tmp.path().join("out_a"), tmp.path().join("out_c"));
    layout(&[&api], &a);
    let built = layout(&[&web], &c);

    // A toy-domain file: its own graph_type and registries (LC.4) and one
    // section the store knows nothing about (LC.5b).
    let repo = RepoId::from_canonical("toy://clip-1");
    let node = |id: u64| Node { id: NodeId(id), repo, confidence: Confidence::Strong, cells: vec![] };
    let mut core = Container {
        header: Header::for_domain("toy", &[(9001, "FRAME")], &[(1, "NEXT")], &[]).unwrap(),
        repo,
        nodes: vec![node(10), node(20)],
        edges: Vec::new(),
        node_kinds: vec![(NodeId(10), NodeKindId(9001)), (NodeId(20), NodeKindId(9001))],
        sections: Vec::new(),
    };
    let section = encode_section("toy", &vec![(0u64, 7u32), (40, 9)]).unwrap();
    let toy_path = tmp.path().join("toy.gmap");
    write_container(&toy_path, &mut core, &[section]).unwrap();
    let toy = std::fs::read(&toy_path).unwrap();
    let meta = layout_meta(&built.repo_labels, &built.repo_roots, &built.parse_errors, &c);
    let foreign = [ForeignShard { name: "clip".into(), graph_type: "toy".into(), bytes: toy.clone() }];
    write_merged_sharded_extras(&built.merged, &meta, &foreign, &[], &c).unwrap();
    assert_eq!(read_layout_extras(&c).unwrap().foreign, foreign, "the member carries it");

    let m = merge_layouts(&[gmap("api", &a), gmap("cam", &c)]).unwrap();
    assert_eq!(m.foreign.len(), 1);
    assert_eq!((m.foreign[0].name.as_str(), m.foreign[0].graph_type.as_str()), ("cam-clip", "toy"));
    assert!(m.foreign[0].bytes == toy, "bytes untouched");
    let out = tmp.path().join("out_m");
    persist_merge(&m, &out, "test").unwrap();

    let shards = manifest(&out)["shards"].as_array().unwrap().clone();
    let entry = shards.iter().find(|e| e["name"] == "cam-clip").expect("listed in the manifest");
    let member_entry = manifest(&c)["shards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "clip")
        .cloned()
        .unwrap();
    assert_eq!(entry["graph_type"], "toy");
    assert_eq!(entry["content_hash"], member_entry["content_hash"], "same bytes, same hash");
    assert!(std::fs::read(out.join("cam-clip.gmap")).unwrap() == toy);
    assert!(shards.iter().filter(|e| e["name"] != "cam-clip").all(|e| e.get("graph_type").is_none()));

    // Read as code: the toy shard is skipped (and reported), the code graphs load.
    let (merged, _) = read_merged_sharded_meta(&out).unwrap();
    assert_eq!(merged.graphs.len(), m.result.merged.graphs.len());
    assert_eq!(read_layout_extras(&out).unwrap().foreign[0].bytes, toy);
    assert_eq!(load_layout(&out).unwrap().merged.graphs.len(), merged.graphs.len());
    // LC.4: the layout inspects with the toy shard named by its own header.
    let seen = inspect_path(&out).unwrap();
    assert!(seen.shards.iter().any(|s| s.graph_type == "toy"), "{}", seen.marker());

    // A merge of the merge carries it on again.
    let again = merge_layouts(&[gmap("all", &out)]).unwrap();
    assert_eq!(again.foreign[0].name, "all-cam-clip");

    let lines = merge_stderr(tmp.path(), &[("api", &a), ("cam", &c)]);
    assert!(lines.iter().any(|l| l == "[gmap] skipped foreign shard clip graph_type=toy"), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains(" foreign_shards=1 ")), "{lines:?}");
}

/// A member cross edge no resolver or pass emitted (an overlay's) is kept
/// once; the member's stored resolver edges are dropped and recomputed, so
/// HTTP_CALLS is not doubled.
#[test]
fn external_edges_survive_resolver_edges_do_not_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let (api, web) = api_web(tmp.path());
    let other = tmp.path().join("other");
    write(&other, "jobs.py", "def nightly():\n    return 1\n");
    let (aw, o) = (tmp.path().join("out_aw"), tmp.path().join("out_o"));

    // Member `aw` is itself a two-repo build: it stores the HTTP_CALLS edge.
    let mut r = build(&[&api, &web]);
    let http = |e: &Edge| e.category == edge_category::HTTP_CALLS;
    assert_eq!(r.merged.cross_edges.iter().filter(|e| http(e)).count(), 1, "control");
    let ids: Vec<NodeId> = r.merged.graphs[0].nodes.iter().take(2).map(|n| n.id).collect();
    let overlay = Edge::new(ids[0], ids[1], edge_category::CALLS, Confidence::Weak)
        .with_cell(Evidence::emitter("overlay:llm").rule("declared").to_cell());
    r.merged.cross_edges.push(overlay);
    r.merged.sort_cross_edges();
    let meta = layout_meta(&r.repo_labels, &r.repo_roots, &r.parse_errors, &aw);
    persist_layout(&r.merged, &meta, &aw, "test").unwrap();
    layout(&[&other], &o);

    let together = build(&[&api, &web, &other]);
    let m = merge_layouts(&[gmap("aw", &aw), gmap("other", &o)]).unwrap();
    let is_overlay = |e: &Edge| Evidence::of(e).is_some_and(|ev| ev.emitter == "overlay:llm");
    assert_eq!(count(&m, is_overlay), 1, "the overlay edge is kept once");
    assert_eq!(count(&m, http), together.merged.all_edges().filter(|e| http(e)).count());
    assert_eq!(count(&m, http), 1);
    assert_eq!(m.result.merged.cross_edges.len(), together.merged.cross_edges.len() + 1);

    let lines = merge_stderr(tmp.path(), &[("aw", &aw), ("other", &o)]);
    assert!(lines.iter().any(|l| l.ends_with(" kept_member_edges=1 labels_stored=0")), "{lines:?}");
}

/// Mark `root` a git checkout of `url`, so two same-basename dirs have
/// distinct identities (LB.1) whether built together or apart.
fn git_remote(root: &Path, url: &str) {
    write(root, ".git/config", &format!("[remote \"origin\"]\n\turl = {url}\n"));
}

/// Labels are set-dependent (`api` deepens to `x/api` only beside another
/// `api`), so the merge recomputes them over the union from the recorded
/// roots; a member whose root is gone keeps its stored label.
#[test]
fn labels_recomputed_over_the_union() {
    let tmp = tempfile::tempdir().unwrap();
    let (x, y) = (tmp.path().join("x/api"), tmp.path().join("y/api"));
    write(&x, "a.py", "def one():\n    return 1\n");
    write(&y, "b.py", "def two():\n    return 2\n");
    git_remote(&x, "https://example.com/x/api.git");
    git_remote(&y, "https://example.com/y/api.git");
    let (lx, ly) = (tmp.path().join("out_x"), tmp.path().join("out_y"));
    let bx = layout(&[&x], &lx);
    let by = layout(&[&y], &ly);
    assert_eq!(bx.repo_labels.values().collect::<Vec<_>>(), ["api"], "alone: depth 1");
    assert_eq!(by.repo_labels.values().collect::<Vec<_>>(), ["api"], "alone: depth 1");

    let together = build(&[&x, &y]);
    let m = merge_layouts(&[gmap("x", &lx), gmap("y", &ly)]).unwrap();
    assert_eq!(m.result.repo_labels, together.repo_labels);
    let mut labels: Vec<&String> = m.result.repo_labels.values().collect();
    labels.sort();
    assert_eq!(labels, ["x/api", "y/api"], "depth-2 labels, not two `api`s");
    let lines = merge_stderr(tmp.path(), &[("x", &lx), ("y", &ly)]);
    assert!(lines.iter().any(|l| l.ends_with(" labels_stored=0")), "{lines:?}");

    // y's root is gone: its stored label is kept (made distinct from x's,
    // which no longer collides and is recomputed back to depth 1).
    std::fs::remove_dir_all(&y).unwrap();
    let m = merge_layouts(&[gmap("x", &lx), gmap("y", &ly)]).unwrap();
    let x_id = *bx.repo_labels.keys().next().unwrap();
    let y_id = *by.repo_labels.keys().next().unwrap();
    assert_eq!(m.result.repo_labels[&x_id], "api");
    assert_eq!(m.result.repo_labels[&y_id], format!("api#{}", y_id % 1000));
    let lines = merge_stderr(tmp.path(), &[("x", &lx), ("y", &ly)]);
    assert!(lines.iter().any(|l| l.ends_with(" labels_stored=1")), "{lines:?}");
}

/// The workspace manifest: paths relative to the file, exactly one of
/// `gmap` / `repo`, no unknown keys, no URLs, version 1.
#[test]
fn workspace_manifest_is_strict() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ws/glia.workspace.json");
    let read = |body: &str| {
        write(tmp.path(), "ws/glia.workspace.json", body);
        read_workspace(&path)
    };
    let members = read(
        r#"{"version":1,"members":[{"name":"api","gmap":"../api/.glia/graph"},{"name":"web","repo":"../web"}]}"#,
    )
    .unwrap();
    let base = tmp.path().join("ws");
    assert_eq!(members[0], gmap("api", &base.join("../api/.glia/graph")));
    assert_eq!(members[1].name(), "web");
    for (body, want) in [
        (r#"{"version":2,"members":[]}"#, "unsupported version 2"),
        (r#"{"version":1,"members":[{"name":"a","gmap":"x","repo":"y"}]}"#, "exactly one"),
        (r#"{"version":1,"members":[{"name":"a"}]}"#, "exactly one"),
        (r#"{"version":1,"members":[{"name":"a","gmap":"x","branch":"main"}]}"#, "unknown field"),
        (r#"{"version":1,"members":[],"fetch":true}"#, "unknown field"),
        (r#"{"version":1,"members":[{"name":"a","repo":"https://github.com/x/y"}]}"#, "local paths only"),
        (r#"{"version":1,"members":[{"name":"../a","gmap":"x"}]}"#, "plain name"),
        (r#"{"version":1,"members":[]}"#, "no members"),
    ] {
        let err = read(body).err().unwrap_or_default();
        assert!(err.contains(want), "{body}: {err}");
    }
}
