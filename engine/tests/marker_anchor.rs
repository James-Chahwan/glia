//! A5.8 acceptance: RPC-family marker nodes are located and owned on a REAL
//! build of the committed substrate-gap fixtures.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild. These tests grade the working tree directly: every
//! GRPC_CLIENT / WS_* / EVENT_* / GRAPHQL_* marker carries a POSITION at its
//! needle and an owner edge to the enclosing METHOD / FUNCTION (USES outbound,
//! HANDLED_BY inbound), or a CONTAINS from its module when no function
//! encloses it. Run with `-- --nocapture` to see the `[marker-anchor]` marker.

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::{
    ParseCache, cross_stack_trace, generate_many, generate_one, generate_one_with_cache,
    locate_node, node_file, service_map,
};
use repo_graph_graph::MergedGraph;
use std::path::Path;

fn fixture(rel: &str) -> String {
    format!(
        "{}/../bench/substrate-gap/fixtures/{rel}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn build(dirs: &[&str]) -> MergedGraph {
    let paths: Vec<String> = dirs.iter().map(|d| fixture(d)).collect();
    generate_many(&paths).expect("fixture builds").merged
}

/// The id of the node of `kind` whose qname is exactly `qname`.
fn node_id(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(move |(id, q)| {
                    q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind)
                })
                .map(|(id, _)| *id)
        })
        .next()
        .unwrap_or_else(|| panic!("no {qname} node of kind {kind:?}"))
}

/// Every POSITION payload on `id`, across graphs.
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

fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
    m.all_edges()
        .any(|e| e.from == from && e.to == to && e.category == cat)
}

fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    locate_node(m, id).qname
}

#[test]
fn go_grpc_client_is_located_owned_and_traceable() {
    let m = build(&["xcut-grpc-grpc_calls/client", "xcut-grpc-grpc_calls/server"]);
    let client = node_id(&m, node_kind::GRPC_CLIENT, "grpc_client:UserService");
    let fetch = node_id(&m, node_kind::FUNCTION, "main::FetchUser");
    let module = node_id(&m, node_kind::MODULE, "main");

    assert_eq!(
        positions(&m, client),
        vec![r#"{"file":"main.go","start_line":17,"end_line":17}"#.to_string()],
        "one-line span at the 0-indexed pb.NewUserServiceClient( row (storage is 0-based)"
    );
    // LD.1: the stored row 17 above and the reported line 18 here are the
    // convention's regression guard — storage stays 0-based, every answer
    // record is 1-based.
    let at = locate_node(&m, client);
    assert_eq!(
        (at.kind, at.file.as_deref(), at.line),
        ("GRPC_CLIENT", Some("main.go"), Some(18))
    );

    assert!(has_edge(&m, fetch, client, edge_category::USES));
    assert!(
        !has_edge(&m, module, client, edge_category::CONTAINS),
        "an owned marker takes no module fallback edge"
    );

    // The point of the packet: a trace from the function crosses into the
    // gRPC hop instead of dying at the file boundary.
    let hops = cross_stack_trace(&m, "main::FetchUser", 4).expect("seed resolves");
    let path: Vec<(&str, &str, bool)> = hops
        .iter()
        .map(|h| (h.mechanism, h.to_qname.as_str(), h.cross_service))
        .collect();
    assert!(
        path.contains(&("USES", "grpc_client:UserService", false)),
        "{path:?}"
    );
    assert!(
        path.contains(&("GRPC_CALLS", "grpc:user.UserService", true)),
        "{path:?}"
    );
}

#[test]
fn csharp_data_driven_client_is_anchored_post_cache() {
    // `grpc_client:Greeter` comes from the build-level proto-needle pass
    // (A5.2), which runs after the per-file extractors.
    let m = build(&["xcut-grpc-csharp/server", "xcut-grpc-csharp/client"]);
    let client = node_id(&m, node_kind::GRPC_CLIENT, "grpc_client:Greeter");
    let fetch = node_id(
        &m,
        node_kind::METHOD,
        "Storefront::Clients::HelloClient::FetchGreeting",
    );
    assert!(has_edge(&m, fetch, client, edge_category::USES));
    assert_eq!(
        positions(&m, client),
        vec![r#"{"file":"HelloClient.cs","start_line":11,"end_line":11}"#.to_string()]
    );
}

#[test]
fn ws_eventbus_and_graphql_markers_are_owned_or_module_contained() {
    // websocket: client inside connectChat; LA.18a: the gorilla handler is the
    // `upgrader.Upgrade(` call inside ServeWs, not the package-level Upgrader.
    let m = build(&[
        "xcut-websocket-ws_connects/client",
        "xcut-websocket-ws_connects/server",
    ]);
    let ws_client = node_id(&m, node_kind::WS_CLIENT, "ws_client:/ws");
    let ws_handler = node_id(&m, node_kind::WS_HANDLER, "ws:ws");
    let connect = node_id(&m, node_kind::FUNCTION, "chat::connectChat");
    let serve = node_id(&m, node_kind::FUNCTION, "hub::ServeWs");
    assert!(has_edge(&m, connect, ws_client, edge_category::USES));
    assert!(has_edge(&m, ws_handler, serve, edge_category::HANDLED_BY));
    assert_eq!(
        positions(&m, ws_client),
        vec![r#"{"file":"chat.ts","start_line":2,"end_line":2}"#.to_string()]
    );
    assert_eq!(
        positions(&m, ws_handler),
        vec![r#"{"file":"hub.go","start_line":15,"end_line":15}"#.to_string()]
    );

    // eventbus: emitter USES, handler HANDLED_BY the enclosing function.
    let m = build(&["xcut-eventbus-event_flows"]);
    let emit = node_id(&m, node_kind::EVENT_EMITTER, "event_emit:userCreated");
    let handle = node_id(&m, node_kind::EVENT_HANDLER, "event_handle:userCreated");
    let create = node_id(&m, node_kind::FUNCTION, "publisher::createUser");
    let register = node_id(&m, node_kind::FUNCTION, "subscriber::registerHandlers");
    let welcome = node_id(&m, node_kind::FUNCTION, "subscriber::sendWelcomeEmail");
    assert!(has_edge(&m, create, emit, edge_category::USES));
    assert!(has_edge(&m, handle, register, edge_category::HANDLED_BY));
    assert!(!has_edge(&m, handle, welcome, edge_category::HANDLED_BY));
    assert_eq!(
        positions(&m, emit),
        vec![r#"{"file":"publisher.ts","start_line":6,"end_line":6}"#.to_string()]
    );
    assert_eq!(
        positions(&m, handle),
        vec![r#"{"file":"subscriber.ts","start_line":3,"end_line":3}"#.to_string()]
    );

    // graphql: operation USES from the component, resolver HANDLED_BY the method.
    let m = build(&[
        "xcut-graphql-graphql_calls/client",
        "xcut-graphql-graphql_calls/server",
    ]);
    let op = node_id(&m, node_kind::GRAPHQL_OPERATION, "graphql_op:getUser");
    let resolver = node_id(&m, node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser");
    let profile = node_id(&m, node_kind::FUNCTION, "query::UserProfile");
    let get_user = node_id(&m, node_kind::METHOD, "resolver::UserResolver::getUser");
    assert!(has_edge(&m, profile, op, edge_category::USES));
    assert!(has_edge(&m, resolver, get_user, edge_category::HANDLED_BY));
    assert_eq!(
        positions(&m, op),
        vec![r#"{"file":"query.ts","start_line":12,"end_line":12}"#.to_string()]
    );
    assert_eq!(
        positions(&m, resolver),
        vec![r#"{"file":"resolver.ts","start_line":8,"end_line":8}"#.to_string()]
    );
}

#[test]
fn arch_monorepo_flows_places_every_marker_and_links_services() {
    let r = generate_one(&fixture("arch-monorepo-flows")).expect("fixture builds");
    let m = &r.merged;
    for (kind, qname, dir) in [
        (node_kind::WS_CLIENT, "ws_client:/ws", "web/"),
        (node_kind::WS_HANDLER, "ws:ws", "api/"),
        (node_kind::GRAPHQL_OPERATION, "graphql_op:getUser", "web/"),
        (
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:getUser",
            "api/",
        ),
        (
            node_kind::GRPC_CLIENT,
            "grpc_client:UserService",
            "gateway/",
        ),
        (node_kind::EVENT_EMITTER, "event_emit:orderPlaced", "api/"),
        (
            node_kind::EVENT_HANDLER,
            "event_handle:orderPlaced",
            "worker/",
        ),
    ] {
        let id = node_id(m, kind, qname);
        let node = m
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .find(|n| n.id == id)
            .expect("node present");
        let file = node_file(node).unwrap_or_default();
        assert!(
            file.starts_with(dir),
            "{qname}: file {file:?} not under {dir}"
        );
    }

    let map = service_map(m, &r.repo_labels);
    let mut links: Vec<(String, String, &str)> = map
        .links
        .iter()
        .map(|l| (l.from.clone(), l.to.clone(), l.mechanism))
        .collect();
    links.sort();
    assert_eq!(map.services.len(), 4);
    assert_eq!(
        links.len(),
        4,
        "one link per flow mechanism, got {links:?} (unlocated={})",
        map.unlocated_nodes
    );
    for (from, to) in [("web", "api"), ("gateway", "api"), ("api", "worker")] {
        assert!(
            links.iter().any(|(f, t, _)| f == from && t == to),
            "missing {from} -> {to} in {links:?}"
        );
    }
}

#[test]
fn owner_edges_are_emitted_in_a_stable_order() {
    // Two builds use two independently seeded HashMaps; the anchor pass must
    // not let either seed reach the edge order.
    let dirs = ["arch-monorepo-flows"];
    let a = build(&dirs);
    let b = build(&dirs);
    // Every intra-repo edge that touches a marker node, in emission order.
    let listing = |m: &MergedGraph| -> Vec<(String, String, EdgeCategoryId)> {
        let is_marker = |id: &NodeId| {
            m.graphs.iter().any(|g| {
                g.nav
                    .kind_by_id
                    .get(id)
                    .is_some_and(|k| repo_graph_code_extractors::anchor::is_marker_kind(*k))
            })
        };
        m.graphs
            .iter()
            .flat_map(|g| g.edges.iter())
            .filter(|e| is_marker(&e.from) || is_marker(&e.to))
            .map(|e| (qname_of(m, e.from), qname_of(m, e.to), e.category))
            .collect()
    };
    let (la, lb) = (listing(&a), listing(&b));
    assert_eq!(la, lb);
    // 6 owner edges + 3 module fallbacks (the package-level ws Upgrader and
    // the two class-level GraphQL decorator nouns), as the marker line says.
    assert_eq!(la.len(), 9, "{la:?}");
    assert!(
        la.iter().any(|(f, t, c)| f.ends_with("::FetchUser")
            && t == "grpc_client:UserService"
            && *c == edge_category::USES),
        "{la:?}"
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn store_bytes(m: &MergedGraph, dir: &Path) -> Vec<(String, Vec<u8>)> {
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

#[test]
fn anchored_markers_are_byte_identical_clean_and_incremental() {
    // engine/tests/byte_identical.rs has no marker needles, so this is the
    // on-disk determinism gate for the anchor pass: clean == clean, and a warm
    // cache that mixes reused and reparsed marker files == clean.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    copy_tree(Path::new(&fixture("arch-monorepo-flows")), &repo);
    let repo_s = repo.to_str().unwrap();

    let a = generate_one(repo_s).unwrap().merged;
    let b = generate_one(repo_s).unwrap().merged;
    assert_eq!(
        store_bytes(&a, &tmp.path().join("a")),
        store_bytes(&b, &tmp.path().join("b")),
        "two clean builds"
    );

    let mut cache = ParseCache::new();
    generate_one_with_cache(repo_s, &mut cache).unwrap();
    // Move the emitter one line down: its file reparses, the rest replay.
    std::fs::write(
        repo.join("api/orders.ts"),
        "// In-process bus: api emits orderPlaced, worker handles it (EVENT_FLOWS).\nimport { EventEmitter } from \"events\";\n\nexport const bus = new EventEmitter();\n\nexport function placeOrder(id: string) {\n  const payload = { id };\n  bus.emit(\"orderPlaced\", payload);\n}\n",
    )
    .unwrap();
    let warm = generate_one_with_cache(repo_s, &mut cache).unwrap().merged;
    assert!(cache.stats.reused > 0 && cache.stats.reparsed > 0);
    let clean = generate_one(repo_s).unwrap().merged;
    assert_eq!(
        store_bytes(&warm, &tmp.path().join("warm")),
        store_bytes(&clean, &tmp.path().join("clean")),
        "incremental vs clean with anchored markers"
    );
    let emit = node_id(&clean, node_kind::EVENT_EMITTER, "event_emit:orderPlaced");
    assert_eq!(
        positions(&clean, emit),
        vec![r#"{"file":"api/orders.ts","start_line":7,"end_line":7}"#.to_string()]
    );
}
