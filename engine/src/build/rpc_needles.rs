//! The build-wide proto service set (A5.2) and the post-cache needle passes
//! keyed on it: gRPC data-driven clients (A5.2) and server markers (A5.3),
//! anchored as they are grafted (A5.8), and the Connect / Twirp procedures and
//! calls (LA.17). `grafts::apply_post_cache` runs them.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use repo_graph_code_domain::evidence::{self, Evidence};
use repo_graph_code_domain::{FileParse, GRAPH_TYPE, attach_imports_cell, node_kind};
use repo_graph_code_extractors::anchor;
use repo_graph_code_extractors::grpc::{self, ProtoServiceRef};
use repo_graph_core::{NodeId, RepoId};

use crate::extract::{detect_language, merge_nav, path_to_qname};

/// Every gRPC service a `.proto` declares anywhere in this build (A5.2). The
/// client-needle pass keys on it, so a client repo with no `.proto` of its own
/// still recognises stubs for the server repo's services.
#[derive(Default)]
pub(super) struct RpcContext {
    /// Sorted and deduplicated, so needle order (and therefore GRPC_CLIENT
    /// emission order) does not depend on walk or repo order.
    pub(super) services: Vec<ProtoServiceRef>,
}

impl RpcContext {
    /// Fold in every service the `.proto` files among `files` declare.
    pub(super) fn add_files(&mut self, files: &[(String, String)]) {
        for (path, source) in files {
            if detect_language(path) == Some("proto") {
                self.services.extend(grpc::proto_service_refs(source));
            }
        }
        self.services.sort_unstable();
        self.services.dedup();
    }
}

/// The data-driven gRPC client pass (A5.2). It runs here, on the router's
/// output, not inside the per-file cross-cutting extractors: its input (the
/// build's proto service set) is not a function of the file's own content, so a
/// cached `FileParse` (WP-D) would otherwise replay clients minted against a
/// stale service set. Running after the cache keeps incremental == clean.
///
/// Every code parser emits the file's MODULE node first, and that is how a
/// parse is paired back to its source. Returns the GRPC_CLIENT nodes it added.
///
/// A5.8: the added clients are anchored here too (POSITION + the owning
/// method's USES edge), because the per-file anchor pass in
/// `apply_cross_cutting_extractors` ran before they existed.
///
/// A5.3: the server pass runs in the same loop, for the same reason: its input
/// (the build's proto services and their rpc names) is not a function of the
/// file. It mints GRPC_SERVER markers the gRPC resolver pairs to their service
/// by HANDLED_BY, anchored `marker --HANDLED_BY--> rpc method`.
///
/// LA.17 (A10.13): the Connect / Twirp pass runs here too, for the same
/// reason. It mints method-level RPC_PROCEDURE / RPC_CALL nodes
/// (`rpc:<proto package>.<Service>.<Method>`) that `RpcStackResolver` pairs.
/// Twirp is gated per repo (`twirp_repo`: a go.mod requiring
/// github.com/twitchtv/twirp, or a generated `.twirp.go`), because a Twirp
/// client file imports nothing but its generated package.
pub(super) fn apply_rpc_needles(
    parses_by_lang: &mut HashMap<&'static str, Vec<FileParse>>,
    files: &[(String, String)],
    repo: RepoId,
    rpc: &RpcContext,
    parse_errors: &mut Vec<String>,
) -> RpcNeedleCounts {
    let mut added = RpcNeedleCounts::default();
    if rpc.services.is_empty() {
        return added;
    }
    let twirp_repo = is_twirp_repo(files);
    for (path, source) in files {
        let client_side = grpc::file_has_grpc_context(source);
        // Superset of `client_side`; the cheap text checks keep the parse
        // lookup below off files that can hold neither half.
        let grpc_side = grpc::may_hold_grpc_server(source);
        let proto_rpc_side = grpc::may_hold_proto_rpc(source, twirp_repo);
        if !grpc_side && !proto_rpc_side {
            continue;
        }
        let Some(lang) = detect_language(path) else { continue };
        if lang == "proto" {
            continue;
        }
        let Some(parses) = parses_by_lang.get_mut(lang) else { continue };
        let module_id =
            NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, &path_to_qname(path));
        // No match = the file failed to parse; there is no module to hang a marker on.
        let Some(fp) = parses
            .iter_mut()
            .find(|fp| fp.nodes.first().is_some_and(|n| n.id == module_id))
        else {
            continue;
        };
        if grpc_side {
            if client_side {
                match catch_unwind(AssertUnwindSafe(|| {
                    grpc::extract_known_grpc_client_nodes(source, module_id, repo, &rpc.services)
                })) {
                    Ok(out) => added.clients += graft_rpc_markers(fp, out, path, module_id, lang),
                    Err(_) => parse_errors.push(format!("{path}: PANIC (grpc client needles)")),
                }
            }
            match catch_unwind(AssertUnwindSafe(|| {
                grpc::extract_grpc_server_nodes(source, module_id, repo, &rpc.services, &fp.nodes, &fp.nav)
            })) {
                Ok(out) => added.servers += graft_rpc_markers(fp, out, path, module_id, lang),
                Err(_) => parse_errors.push(format!("{path}: PANIC (grpc server needles)")),
            }
        }
        if proto_rpc_side {
            match catch_unwind(AssertUnwindSafe(|| {
                grpc::extract_proto_rpc_nodes(
                    source,
                    path,
                    lang,
                    module_id,
                    repo,
                    &rpc.services,
                    &fp.nodes,
                    &fp.nav,
                    twirp_repo,
                )
            })) {
                Ok(out) => {
                    added.proto_rpc.add(out.counts);
                    graft_proto_rpc(fp, out, lang);
                }
                Err(_) => parse_errors.push(format!("{path}: PANIC (proto rpc needles)")),
            }
        }
    }
    added
}

/// LA.17: does this repo speak Twirp? Its go.mod requires the runtime, or it
/// holds a generated `.twirp.go`. Read off the walk's `files` (go.mod is a
/// manifest the walk keeps), so it is the same for cached and fresh parses.
fn is_twirp_repo(files: &[(String, String)]) -> bool {
    files.iter().any(|(p, s)| {
        (p.rsplit('/').next() == Some("go.mod") && s.contains("github.com/twitchtv/twirp"))
            || p.ends_with(".twirp.go")
    })
}

/// Markers the post-cache RPC pass added to one repo's parses.
#[derive(Default)]
pub(super) struct RpcNeedleCounts {
    pub(super) clients: usize,
    pub(super) servers: usize,
    /// LA.17: the Connect / Twirp pass's tallies, summed over the repo.
    pub(super) proto_rpc: grpc::ProtoRpcCounts,
}

/// Graft one post-cache marker batch onto its file's parse: the nodes, their
/// nav, their anchors (POSITION + owner edge), then the file's IMPORTS cell.
/// Returns how many nodes it added.
fn graft_rpc_markers(
    fp: &mut FileParse,
    out: grpc::GrpcNodes,
    path: &str,
    module_id: NodeId,
    lang: &str,
) -> usize {
    if out.nodes.is_empty() {
        return 0;
    }
    let grpc::GrpcNodes {
        nodes,
        nav,
        mut anchors,
    } = out;
    // Anchor first, then add the IMPORTS cell, so a data-driven client's
    // cells come in the same order as a fallback client's (whose POSITION
    // lands in the extractor pass, before the router's IMPORTS cell).
    let first_new = fp.nodes.len();
    fp.nodes.extend(nodes);
    merge_nav(&mut fp.nav, nav);
    // LC.3a: the anchor pass only appends, so the owner edges it adds here,
    // post-cache, are the tail.
    let first_edge = fp.edges.len();
    anchor::attach(fp, path, module_id, &mut anchors);
    if let Some(added) = fp.edges.get_mut(first_edge..) {
        let ev = Evidence::emitter("extractor:rpc_needles").rule("anchor");
        evidence::stamp_missing_with(added, &ev);
    }
    // The same raw G15 IMPORTS cell the router gave every other node in the
    // file; `filter_imports_cells` rewrites it with the rest of the file's.
    let mut extra = FileParse {
        nodes: fp.nodes.split_off(first_new),
        imports: fp.imports.clone(),
        ..Default::default()
    };
    attach_imports_cell(&mut extra, lang);
    let added = extra.nodes.len();
    fp.nodes.extend(extra.nodes);
    added
}

/// LA.17: graft one Connect / Twirp batch onto its file's parse. The nodes
/// arrive finished (POSITION, and HANDLED_BY / USES / module CONTAINS), so no
/// anchor pass runs; like [`graft_rpc_markers`] they then take the router's raw
/// G15 IMPORTS cell, which `filter_imports_cells` rewrites with the rest of the
/// file's.
fn graft_proto_rpc(fp: &mut FileParse, out: grpc::ProtoRpcNodes, lang: &str) {
    let grpc::ProtoRpcNodes {
        nodes,
        mut edges,
        nav,
        ..
    } = out;
    if nodes.is_empty() {
        return;
    }
    // LC.3a: the procedure / call edges land post-cache, unstamped.
    let ev = Evidence::emitter("extractor:rpc_needles").rule("proto_rpc");
    evidence::stamp_missing_with(&mut edges, &ev);
    let mut extra = FileParse {
        nodes,
        imports: fp.imports.clone(),
        ..Default::default()
    };
    attach_imports_cell(&mut extra, lang);
    fp.nodes.extend(extra.nodes);
    fp.edges.extend(edges);
    merge_nav(&mut fp.nav, nav);
}

#[cfg(test)]
mod rpc_needle_tests {
    use super::*;
    use std::path::Path;

    use repo_graph_code_domain::walk_gating::repo_identity;
    use repo_graph_code_domain::{cell_type, edge_category};
    use repo_graph_core::{Cell, EdgeCategoryId as CategoryId};
    use repo_graph_graph::MergedGraph;

    use crate::build::{generate_many, generate_one, generate_one_with_cache};
    use crate::cache::ParseCache;

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// `(qname, node id)` of every GRPC_CLIENT in `repo`, sorted by qname.
    fn clients(m: &MergedGraph, repo: RepoId) -> Vec<(String, NodeId)> {
        let mut out: Vec<(String, NodeId)> = m
            .graphs
            .iter()
            .filter(|g| g.repo == repo)
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRPC_CLIENT))
                    .map(move |n| (g.nav.qname_by_id[&n.id].clone(), n.id))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn node_id_by_qname(m: &MergedGraph, qname: &str) -> NodeId {
        m.graphs
            .iter()
            .find_map(|g| {
                g.nav
                    .qname_by_id
                    .iter()
                    .find_map(|(id, q)| (q == qname).then_some(*id))
            })
            .unwrap_or_else(|| panic!("no node {qname}"))
    }

    fn cells_of(m: &MergedGraph, id: NodeId) -> Vec<Cell> {
        m.graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .find(|n| n.id == id)
            .map(|n| n.cells.clone())
            .unwrap_or_default()
    }

    fn incoming(m: &MergedGraph, id: NodeId) -> Vec<CategoryId> {
        let mut cats: Vec<CategoryId> =
            m.all_edges().filter(|e| e.to == id).map(|e| e.category).collect();
        cats.sort_by_key(|c| c.0);
        cats
    }

    fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: CategoryId) -> bool {
        m.all_edges()
            .any(|e| e.from == from && e.to == to && e.category == cat)
    }

    const PROTO: &str = "syntax = \"proto3\";\npackage shop;\noption go_package = \"example.com/shop/pb\";\n\nservice Greeter {\n  rpc SayHello (HelloRequest) returns (HelloReply);\n}\n\nservice OrderService {\n  rpc Place (PlaceRequest) returns (PlaceReply);\n}\n";

    const GO_CLIENT: &str = "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/shop/pb\"\n)\n\nfunc main() {\n\tconn, _ := grpc.Dial(\"server:50051\")\n\tgreeter := pb.NewGreeterClient(conn)\n\torders := pb.NewOrderServiceClient(conn)\n\t_, _ = greeter, orders\n}\n";

    #[test]
    fn generate_many_mints_clients_from_the_union_of_proto_services() {
        let tmp = tempfile::tempdir().unwrap();
        let server = tmp.path().join("server");
        let client = tmp.path().join("client");
        write(&server, "api.proto", PROTO);
        write(&client, "main.go", GO_CLIENT);
        let (server_s, client_s) = (
            server.to_str().unwrap().to_string(),
            client.to_str().unwrap().to_string(),
        );
        let client_repo = RepoId::from_canonical(&repo_identity(&client).key);

        // Alone, the client repo knows no proto: only the suffix fallback fires.
        let alone = generate_one(&client_s).unwrap();
        let names: Vec<String> = clients(&alone.merged, client_repo)
            .into_iter()
            .map(|(q, _)| q)
            .collect();
        assert_eq!(names, vec!["grpc_client:OrderService".to_string()]);

        // Merged, the server's .proto names `Greeter` for the client repo too.
        let merged = generate_many(&[server_s.clone(), client_s.clone()]).unwrap().merged;
        let found = clients(&merged, client_repo);
        let names: Vec<&str> = found.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(names, vec!["grpc_client:Greeter", "grpc_client:OrderService"]);
        let (greeter, orders) = (found[0].1, found[1].1);
        assert!(has_edge(
            &merged,
            greeter,
            node_id_by_qname(&merged, "grpc:shop.Greeter"),
            edge_category::GRPC_CALLS
        ));
        assert!(has_edge(
            &merged,
            orders,
            node_id_by_qname(&merged, "grpc:shop.OrderService"),
            edge_category::GRPC_CALLS
        ));

        // A data-driven client is shaped exactly like a fallback one: same
        // cells in the same order (the file's IMPORTS), same incoming
        // structural edges. POSITION is the one per-client cell (A5.8): each
        // stub is located at its own construction line.
        let greeter_cells = cells_of(&merged, greeter);
        let orders_cells = cells_of(&merged, orders);
        assert!(greeter_cells.iter().any(|c| c.kind == cell_type::IMPORTS));
        let kinds = |cells: &[Cell]| cells.iter().map(|c| c.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&greeter_cells), kinds(&orders_cells));
        let without_position = |cells: &[Cell]| {
            cells
                .iter()
                .filter(|c| c.kind != cell_type::POSITION)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(without_position(&greeter_cells), without_position(&orders_cells));
        let position = |cells: &[Cell]| {
            cells
                .iter()
                .find(|c| c.kind == cell_type::POSITION)
                .map(|c| c.payload.clone())
        };
        assert_eq!(
            position(&greeter_cells),
            Some(repo_graph_core::CellPayload::Json(
                r#"{"file":"main.go","start_line":9,"end_line":9}"#.to_string()
            ))
        );
        assert_eq!(
            position(&orders_cells),
            Some(repo_graph_core::CellPayload::Json(
                r#"{"file":"main.go","start_line":10,"end_line":10}"#.to_string()
            ))
        );
        let mut in_greeter = incoming(&merged, greeter);
        let mut in_orders = incoming(&merged, orders);
        in_greeter.retain(|c| *c != edge_category::GRPC_CALLS);
        in_orders.retain(|c| *c != edge_category::GRPC_CALLS);
        assert_eq!(in_greeter, in_orders);

        // Repo order does not change what the client repo gets.
        let reversed = generate_many(&[client_s, server_s]).unwrap().merged;
        let rev_names: Vec<String> = clients(&reversed, client_repo)
            .into_iter()
            .map(|(q, _)| q)
            .collect();
        assert_eq!(rev_names, names);
    }

    fn write_store(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
        repo_graph_store::write_merged_sharded(m, dir).unwrap();
        let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().to_string(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    /// `(qname, node id)` of every GRPC_SERVER in `m`, sorted by qname and
    /// deduplicated by id (one marker id can sit in several language graphs).
    fn servers(m: &MergedGraph) -> Vec<(String, NodeId)> {
        let mut out: Vec<(String, NodeId)> = m
            .graphs
            .iter()
            .flat_map(|g| {
                g.nodes
                    .iter()
                    .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::GRPC_SERVER))
                    .map(move |n| (g.nav.qname_by_id[&n.id].clone(), n.id))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        out.dedup();
        out
    }

    const GO_SERVER: &str = "package main\n\nimport (\n\t\"context\"\n\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/shop/pb\"\n)\n\ntype server struct {\n\tpb.UnimplementedGreeterServer\n}\n\nfunc (s *server) SayHello(ctx context.Context, in *pb.HelloRequest) (*pb.HelloReply, error) {\n\treturn &pb.HelloReply{}, nil\n}\n\nfunc main() {\n\ts := grpc.NewServer()\n\tpb.RegisterGreeterServer(s, &server{})\n}\n";

    #[test]
    fn server_marker_pairs_to_its_service_and_anchors_to_the_rpc_method() {
        let tmp = tempfile::tempdir().unwrap();
        let server = tmp.path().join("server");
        write(&server, "api.proto", PROTO);
        write(&server, "main.go", GO_SERVER);
        let merged = generate_one(server.to_str().unwrap()).unwrap().merged;

        let found = servers(&merged);
        let names: Vec<&str> = found.iter().map(|(q, _)| q.as_str()).collect();
        assert_eq!(names, vec!["grpc_server:Greeter"], "OrderService has no impl here");
        let marker = found[0].1;
        let service = node_id_by_qname(&merged, "grpc:shop.Greeter");
        let say_hello = node_id_by_qname(&merged, "main::server::SayHello");
        let main_fn = node_id_by_qname(&merged, "main::main");
        assert!(has_edge(&merged, service, marker, edge_category::HANDLED_BY));
        assert!(has_edge(&merged, marker, say_hello, edge_category::HANDLED_BY));
        // The receiver method serves the rpc; `main` only registers it.
        assert!(!has_edge(&merged, marker, main_fn, edge_category::HANDLED_BY));
        // Located at the embedded base, the first needle line.
        let position = cells_of(&merged, marker)
            .into_iter()
            .find(|c| c.kind == cell_type::POSITION)
            .map(|c| c.payload);
        assert_eq!(
            position,
            Some(repo_graph_core::CellPayload::Json(
                r#"{"file":"main.go","start_line":10,"end_line":10}"#.to_string()
            ))
        );
        // Package evidence and the file's IMPORTS cell, like a client.
        let kinds: Vec<_> = cells_of(&merged, marker).iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&cell_type::RPC_PACKAGE) && kinds.contains(&cell_type::IMPORTS));
    }

    #[test]
    fn server_markers_follow_the_proto_under_a_warm_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(
            &repo,
            "server.py",
            "import grpc\nimport hello_pb2_grpc\n\n\nclass Greeter(hello_pb2_grpc.GreeterServicer):\n    def Hi(self, request, context):\n        return None\n",
        );
        write(&repo, "api.proto", GREETER_PROTO);
        let repo_s = repo.to_str().unwrap();
        let names = |m: &MergedGraph| -> Vec<String> { servers(m).into_iter().map(|(q, _)| q).collect() };

        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(names(&cold.merged), vec!["grpc_server:Greeter".to_string()]);
        let marker = servers(&cold.merged)[0].1;
        let hi = node_id_by_qname(&cold.merged, "server::Greeter::Hi");
        assert!(has_edge(&cold.merged, marker, hi, edge_category::HANDLED_BY));

        // A proto-only edit: server.py comes from the cache and loses its marker.
        write(&repo, "api.proto", FAREWELL_PROTO);
        let warm = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "server.py must come from the cache");
        assert!(names(&warm.merged).is_empty(), "stale server marker replayed from cache");
        let clean = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm.merged, &tmp.path().join("warm")),
            write_store(&clean.merged, &tmp.path().join("clean")),
            "incremental vs clean after a proto-only edit"
        );

        write(&repo, "api.proto", GREETER_PROTO);
        let warm2 = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(names(&warm2.merged), vec!["grpc_server:Greeter".to_string()]);
        let clean2 = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm2.merged, &tmp.path().join("warm2")),
            write_store(&clean2.merged, &tmp.path().join("clean2")),
            "incremental vs clean with a server marker present"
        );
    }

    const GREETER_PROTO: &str = "package hello;\nservice Greeter {\n  rpc Hi (A) returns (B);\n}\n";
    const FAREWELL_PROTO: &str = "package hello;\nservice Farewell {\n  rpc Bye (A) returns (B);\n}\n";

    #[test]
    fn rpc_needles_follow_the_proto_under_a_warm_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        write(
            &repo,
            "client.py",
            "import grpc\nimport hello_pb2_grpc\n\n\ndef call(channel):\n    return hello_pb2_grpc.GreeterStub(channel)\n",
        );
        write(&repo, "api.proto", GREETER_PROTO);
        let repo_s = repo.to_str().unwrap();
        let rid = RepoId::from_canonical(&repo_identity(&repo).key);
        let names = |m: &MergedGraph| -> Vec<String> {
            clients(m, rid).into_iter().map(|(q, _)| q).collect()
        };

        let mut cache = ParseCache::new();
        let cold = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(names(&cold.merged), vec!["grpc_client:Greeter".to_string()]);

        // Only the .proto changes: client.py is replayed from the cache, and
        // must still lose the client its needle no longer names.
        write(&repo, "api.proto", FAREWELL_PROTO);
        let warm = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1, "client.py must come from the cache");
        assert!(names(&warm.merged).is_empty(), "stale client replayed from cache");
        let clean = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm.merged, &tmp.path().join("warm")),
            write_store(&clean.merged, &tmp.path().join("clean")),
            "incremental vs clean after a proto-only edit"
        );

        // And back: the cached parse picks the client up again.
        write(&repo, "api.proto", GREETER_PROTO);
        let warm2 = generate_one_with_cache(repo_s, &mut cache).unwrap();
        assert_eq!(cache.stats.reused, 1);
        assert_eq!(names(&warm2.merged), vec!["grpc_client:Greeter".to_string()]);
        let clean2 = generate_one(repo_s).unwrap();
        assert_eq!(
            write_store(&warm2.merged, &tmp.path().join("warm2")),
            write_store(&clean2.merged, &tmp.path().join("clean2")),
            "incremental vs clean with a data-driven client present"
        );
    }
}
