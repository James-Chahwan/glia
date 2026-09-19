//! LA.17 (A10.13) acceptance: Connect and Twirp procedures and calls on REAL
//! multi-dir builds of the committed `xcut-connect-rpc` / `xcut-twirp-rpc`
//! fixtures, paired by the unchanged `RpcStackResolver`.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until the
//! end-of-wave rebuild; these tests grade the working tree directly. Run with
//! `-- --nocapture` to see the `[proto-rpc]` marker.

use std::path::Path;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use repo_graph_engine::{
    GenerateResult, ParseCache, generate_many, generate_one, generate_one_with_cache, service_map,
};
use repo_graph_graph::MergedGraph;

fn build(fixture: &str, dirs: &[&str]) -> GenerateResult {
    let root = format!(
        "{}/../bench/substrate-gap/fixtures/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let dirs: Vec<String> = dirs.iter().map(|d| format!("{root}/{d}")).collect();
    generate_many(&dirs).expect("fixture builds")
}

/// `(id, repo)` of every node of `kind` whose qname is exactly `qname`,
/// deduplicated (a node id can sit in several language graphs).
fn nodes_of(m: &MergedGraph, kind: NodeKindId, qname: &str) -> Vec<(NodeId, RepoId)> {
    let mut out: Vec<(NodeId, RepoId)> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&kind)
                    && g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
                .then_some((n.id, n.repo))
            })
        })
        .collect();
    out.sort_by_key(|(id, r)| (r.0, id.0));
    out.dedup();
    out
}

/// Every qname of a node of `kind`, sorted and deduplicated.
fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id).cloned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The one node whose qname is exactly `qname`, whatever its kind.
fn by_qname(m: &MergedGraph, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nodes
                .iter()
                .find(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
                .map(|n| n.id)
        })
        .unwrap_or_else(|| panic!("no node {qname}"))
}

fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
    m.all_edges()
        .any(|e| e.from == from && e.to == to && e.category == cat)
}

fn positions(m: &MergedGraph, id: NodeId) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| g.nodes.iter())
        .filter(|n| n.id == id)
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::POSITION)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => Some(j.clone()),
            _ => None,
        })
        .collect()
}

/// The RepoId whose human label is `label`.
fn repo_labelled(r: &GenerateResult, label: &str) -> RepoId {
    r.repo_labels
        .iter()
        .find_map(|(id, l)| (l == label).then_some(RepoId(*id)))
        .unwrap_or_else(|| panic!("no repo labelled {label}: {:?}", r.repo_labels))
}

const SAY: &str = "rpc:connectrpc.eliza.v1.ElizaService.Say";
const INTRODUCE: &str = "rpc:connectrpc.eliza.v1.ElizaService.Introduce";
const SAY_CALL: &str = "rpc_call:connectrpc.eliza.v1.ElizaService.Say";

#[test]
fn connect_procedures_and_calls_pair() {
    let r = build("xcut-connect-rpc", &["server", "client", "web"]);
    let m = &r.merged;
    assert_eq!(
        qnames_of(m, node_kind::RPC_PROCEDURE),
        vec![INTRODUCE.to_string(), SAY.to_string()],
        "one procedure per proto rpc of the registered service"
    );
    assert_eq!(
        qnames_of(m, node_kind::RPC_CALL),
        vec![SAY_CALL.to_string()],
        "no client calls Introduce"
    );
    let say = nodes_of(m, node_kind::RPC_PROCEDURE, SAY);
    let intro = nodes_of(m, node_kind::RPC_PROCEDURE, INTRODUCE);
    assert_eq!((say.len(), intro.len()), (1, 1));
    let (say, intro) = (say[0].0, intro[0].0);

    // One call node per calling repo: the Go client and the connect-es web client.
    let (client, web) = (repo_labelled(&r, "client"), repo_labelled(&r, "web"));
    let calls = nodes_of(m, node_kind::RPC_CALL, SAY_CALL);
    let repos: Vec<RepoId> = calls.iter().map(|(_, repo)| *repo).collect();
    let mut want = vec![client, web];
    want.sort_by_key(|r| r.0);
    assert_eq!(repos, want, "rpc_call in the client repo AND the web repo");
    for (call, _) in &calls {
        assert!(
            has_edge(m, *call, say, edge_category::RPC_CALLS),
            "every call pairs to the Say procedure"
        );
    }
    let go_call = calls
        .iter()
        .find(|(_, repo)| *repo == client)
        .map(|(id, _)| *id);
    let ts_call = calls
        .iter()
        .find(|(_, repo)| *repo == web)
        .map(|(id, _)| *id);
    let (Some(go_call), Some(ts_call)) = (go_call, ts_call) else {
        panic!("calls by repo: {calls:?}")
    };

    // The procedure is served by the registered type's method; the rpc served
    // by the Unimplemented embed is HANDLED_BY nothing (never `main`).
    assert!(has_edge(
        m,
        say,
        by_qname(m, "cmd::main::elizaServer::Say"),
        edge_category::HANDLED_BY
    ));
    assert!(
        !m.all_edges()
            .any(|e| e.from == intro && e.category == edge_category::HANDLED_BY),
        "Introduce has no HANDLED_BY edge at all"
    );
    assert!(has_edge(
        m,
        by_qname(m, "cmd::main"),
        intro,
        edge_category::CONTAINS
    ));

    // Callers USE their calls.
    assert!(has_edge(
        m,
        by_qname(m, "main::main"),
        go_call,
        edge_category::USES
    ));
    assert!(has_edge(
        m,
        by_qname(m, "src::eliza::talk"),
        ts_call,
        edge_category::USES
    ));
    assert_eq!(
        positions(m, go_call),
        vec![r#"{"file":"main.go","start_line":14,"end_line":14}"#.to_string()],
        "located at the first call site (0-based row)"
    );
    assert_eq!(
        positions(m, ts_call),
        vec![r#"{"file":"src/eliza.ts","start_line":8,"end_line":8}"#.to_string()]
    );
    // Say is located at the implementing method's span.
    assert!(
        positions(m, say)
            .iter()
            .all(|p| p.starts_with(r#"{"file":"cmd/main.go","#)),
        "{:?}",
        positions(m, say)
    );
    // The Go client keeps its legacy service-level stub (declared overlap).
    assert!(!nodes_of(m, node_kind::GRPC_CLIENT, "grpc_client:ElizaService").is_empty());
    // Grafted nodes carry the file's IMPORTS cell like every parsed node.
    assert!(
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .filter(|n| n.id == go_call)
            .all(|n| n.cells.iter().any(|c| c.kind == cell_type::IMPORTS))
    );
}

#[test]
fn twirp_procedures_and_calls_pair() {
    let r = build("xcut-twirp-rpc", &["server", "client"]);
    let m = &r.merged;
    let procs = nodes_of(
        m,
        node_kind::RPC_PROCEDURE,
        "rpc:example.haberdasher.Haberdasher.MakeHat",
    );
    let calls = nodes_of(
        m,
        node_kind::RPC_CALL,
        "rpc_call:example.haberdasher.Haberdasher.MakeHat",
    );
    assert_eq!((procs.len(), calls.len()), (1, 1), "{procs:?} {calls:?}");
    let (procedure, call) = (procs[0].0, calls[0].0);
    assert_eq!(calls[0].1, repo_labelled(&r, "client"));
    assert!(has_edge(m, call, procedure, edge_category::RPC_CALLS));
    assert!(has_edge(
        m,
        procedure,
        by_qname(m, "cmd::server::main::HaberdasherServer::MakeHat"),
        edge_category::HANDLED_BY
    ));
    assert!(has_edge(
        m,
        by_qname(m, "main::main"),
        call,
        edge_category::USES
    ));
    assert!(
        qnames_of(m, node_kind::GRPC_SERVER).is_empty(),
        "Twirp is not gRPC: no GRPC_SERVER"
    );
}

#[test]
fn arch_shows_rpc_links() {
    assert!(repo_graph_engine::arch::FLOW_MECHANISMS.contains(&edge_category::RPC_CALLS));
    let r = build("xcut-connect-rpc", &["server", "client", "web"]);
    let map = service_map(&r.merged, &r.repo_labels);
    let rpc: Vec<(&str, &str)> = map
        .links
        .iter()
        .filter(|l| l.mechanism == "RPC_CALLS")
        .map(|l| (l.from.as_str(), l.to.as_str()))
        .collect();
    let server = map
        .services
        .iter()
        .find(|s| s.repo == "server")
        .map(|s| s.id.as_str())
        .unwrap_or("server");
    let from_repo = |label: &str| {
        map.services
            .iter()
            .filter(|s| s.repo == label)
            .any(|s| rpc.iter().any(|(f, t)| *f == s.id && *t == server))
    };
    assert!(
        from_repo("client") && from_repo("web"),
        "client -> server and web -> server RPC_CALLS links; got {rpc:?} over {:?}",
        map.services
            .iter()
            .map(|s| (&s.id, &s.repo))
            .collect::<Vec<_>>()
    );
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("rel has a parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

fn store_bytes(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
    repo_graph_store::write_merged_sharded(m, dir).expect("store writes");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("store dir")
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                std::fs::read(e.path()).expect("read shard"),
            )
        })
        .collect();
    out.sort();
    out
}

/// The Twirp gate is the repo's go.mod, not the file: a go.mod-only edit must
/// add and drop the call on a cache-served `main.go`, byte-identical to clean.
#[test]
fn twirp_gate_follows_go_mod_under_a_warm_cache() {
    const PROTO: &str = "syntax = \"proto3\";\npackage example.haberdasher;\nservice Haberdasher {\n  rpc MakeHat(Size) returns (Hat);\n}\n";
    const MAIN: &str = "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\tpb \"example.com/twirp/rpc/haberdasher\"\n)\n\nfunc main() {\n\tclient := pb.NewHaberdasherProtobufClient(\"http://localhost:8080\", &http.Client{})\n\t_, _ = client.MakeHat(context.Background(), &pb.Size{Inches: 12})\n}\n";
    const TWIRP_MOD: &str = "module example.com/twirpclient\n\ngo 1.22\n\nrequire github.com/twitchtv/twirp v8.1.3+incompatible\n";
    const PLAIN_MOD: &str = "module example.com/twirpclient\n\ngo 1.22\n";
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write(&repo, "rpc/service.proto", PROTO);
    write(&repo, "main.go", MAIN);
    write(&repo, "go.mod", TWIRP_MOD);
    let repo_s = repo.to_str().expect("utf-8 path");
    let calls = |m: &MergedGraph| qnames_of(m, node_kind::RPC_CALL);
    let want = vec!["rpc_call:example.haberdasher.Haberdasher.MakeHat".to_string()];

    let mut cache = ParseCache::new();
    let cold = generate_one_with_cache(repo_s, &mut cache).expect("cold build");
    assert_eq!(calls(&cold.merged), want);

    write(&repo, "go.mod", PLAIN_MOD);
    let warm = generate_one_with_cache(repo_s, &mut cache).expect("warm build");
    assert!(cache.stats.reused >= 1, "main.go must come from the cache");
    assert!(
        calls(&warm.merged).is_empty(),
        "stale Twirp call replayed from cache"
    );
    let clean = generate_one(repo_s).expect("clean build");
    assert_eq!(
        store_bytes(&warm.merged, &tmp.path().join("warm")),
        store_bytes(&clean.merged, &tmp.path().join("clean")),
        "incremental vs clean after a go.mod-only edit"
    );

    write(&repo, "go.mod", TWIRP_MOD);
    let warm2 = generate_one_with_cache(repo_s, &mut cache).expect("warm build 2");
    assert_eq!(calls(&warm2.merged), want);
    let clean2 = generate_one(repo_s).expect("clean build 2");
    assert_eq!(
        store_bytes(&warm2.merged, &tmp.path().join("warm2")),
        store_bytes(&clean2.merged, &tmp.path().join("clean2")),
        "incremental vs clean with a Twirp call present"
    );
}
