//! LB.8 — the owner segment on channel SIDES, over REAL builds.
//!
//! QUEUE_PRODUCER / QUEUE_CONSUMER, WS_HANDLER / WS_CLIENT, GRAPHQL_RESOLVER /
//! GRAPHQL_OPERATION, GRPC_CLIENT and GRPC_SERVER nodes whose file lies under
//! a NESTED project root are qualified with ` @<project path>`, like LB.4a's
//! ROUTE / ENDPOINT nodes. The channel itself (topic, ws path, graphql field,
//! proto service) stays the pairing key, and the proto GRPC_SERVICE contract
//! stays one shared, unowned node.
//!
//! Before it, on the `bench/substrate-gap/fixtures/channel-monorepo-owner`
//! layout (eleven manifest-rooted services), two publishing services were ONE
//! `queue_producer:orders.created`, two consuming services ONE consumer, two
//! gateways ONE `grpc_client:UserService`, two NestJS apps ONE
//! `graphql_resolver:getUser` HANDLED_BY both apps' methods, two FastAPI apps
//! ONE `ws:/ws`; `glia arch` printed 4 links where the tree has 10. `bench/` is
//! outside the cargo workspace's crates, so the fixture is copied into a
//! tempdir (the `scope_filter.rs` way) and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::{
    GenerateResult, ParseCache, generate_one, generate_one_incremental, generate_one_with_cache,
    message_contracts, service_map,
};
use repo_graph_graph::MergedGraph;

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/channel-monorepo-owner",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray `.ai/` cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn build(dir: &Path) -> GenerateResult {
    generate_one(dir.to_str().unwrap()).expect("generate_one")
}

fn monorepo() -> (tempfile::TempDir, GenerateResult) {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let r = build(td.path());
    (td, r)
}

/// Every qname of `kind`, sorted and deduplicated across graphs.
fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let set: BTreeSet<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&kind))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    set.into_iter().collect()
}

fn qname(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// `(from qname, to qname)` for every edge of `category`, sorted, duplicates
/// kept (a count is part of what is asserted).
fn edges(m: &MergedGraph, category: EdgeCategoryId) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = m
        .all_edges()
        .filter(|e| e.category == category)
        .map(|e| (qname(m, e.from), qname(m, e.to)))
        .collect();
    out.sort();
    out
}

fn s(a: &str) -> String {
    a.to_string()
}

#[test]
fn monorepo_channel_sides_carry_their_owner() {
    let (_td, r) = monorepo();
    let m = &r.merged;
    assert_eq!(
        qnames_of(m, node_kind::QUEUE_PRODUCER),
        [
            "queue_producer:orders.created @services/orders",
            "queue_producer:orders.created @services/returns"
        ]
    );
    assert_eq!(
        qnames_of(m, node_kind::QUEUE_CONSUMER),
        [
            "queue_consumer:orders.created @services/audit",
            "queue_consumer:orders.created @services/billing"
        ]
    );
    assert_eq!(
        qnames_of(m, node_kind::GRPC_CLIENT),
        ["grpc_client:UserService @services/admin", "grpc_client:UserService @services/gateway"]
    );
    assert_eq!(
        qnames_of(m, node_kind::GRPC_SERVICE),
        ["grpc:user.UserService"],
        "the proto contract is shared and never owned"
    );
    assert_eq!(qnames_of(m, node_kind::WS_HANDLER), ["ws:/ws @services/chat", "ws:/ws @services/notify"]);
    assert_eq!(qnames_of(m, node_kind::WS_CLIENT), ["ws_client:/ws @web"]);
    assert_eq!(qnames_of(m, node_kind::GRAPHQL_OPERATION), ["graphql_op:getUser @web"]);

    // Each field resolver is HANDLED_BY exactly its own service's method: the
    // cross-project HANDLED_BY the collapsed node carried is gone.
    let handled: Vec<(String, String)> = edges(m, edge_category::HANDLED_BY)
        .into_iter()
        .filter(|(from, _)| from.starts_with("graphql_resolver:getUser"))
        .collect();
    assert_eq!(
        handled,
        [
            (
                s("graphql_resolver:getUser @services/catalog"),
                s("services::catalog::resolver::MeResolver::getUser")
            ),
            (
                s("graphql_resolver:getUser @services/users"),
                s("services::users::resolver::MeResolver::getUser")
            ),
        ]
    );

    // Display names are unchanged: the owner lives in the qname only.
    for g in &m.graphs {
        for (id, q) in &g.nav.qname_by_id {
            if q == "queue_producer:orders.created @services/orders" {
                assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some("orders.created"));
            }
        }
    }
}

#[test]
fn every_owner_pairs_by_the_shared_channel() {
    let (_td, r) = monorepo();
    let m = &r.merged;
    let q = |a: &str, b: &str| {
        (
            format!("queue_producer:orders.created @services/{a}"),
            format!("queue_consumer:orders.created @services/{b}"),
        )
    };
    assert_eq!(
        edges(m, edge_category::QUEUE_FLOWS),
        [q("orders", "audit"), q("orders", "billing"), q("returns", "audit"), q("returns", "billing")],
        "every producer pairs every consumer of the topic"
    );
    assert_eq!(
        edges(m, edge_category::GRPC_CALLS),
        [
            (s("grpc_client:UserService @services/admin"), s("grpc:user.UserService")),
            (s("grpc_client:UserService @services/gateway"), s("grpc:user.UserService")),
        ]
    );
    assert_eq!(
        edges(m, edge_category::GRAPHQL_CALLS),
        [
            (s("graphql_op:getUser @web"), s("graphql_resolver:getUser @services/catalog")),
            (s("graphql_op:getUser @web"), s("graphql_resolver:getUser @services/users")),
        ]
    );
    assert_eq!(
        edges(m, edge_category::WS_CONNECTS),
        [
            (s("ws_client:/ws @web"), s("ws:/ws @services/chat")),
            (s("ws_client:/ws @web"), s("ws:/ws @services/notify")),
        ]
    );
}

/// `glia arch` sees every link the collapse hid: 10, where HEAD printed 4.
/// Channels read owner-free.
#[test]
fn service_map_shows_every_side() {
    let (_td, r) = monorepo();
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "project_roots");
    let links: BTreeSet<(String, String, &str, String)> = map
        .links
        .iter()
        .map(|l| (l.from.clone(), l.to.clone(), l.mechanism, l.channel.clone()))
        .collect();
    let mut want: BTreeSet<(String, String, &str, String)> = BTreeSet::new();
    for c in ["services/admin", "services/gateway"] {
        want.insert((s(c), s("(outside projects)"), "GRPC_CALLS", s("UserService")));
    }
    for p in ["services/orders", "services/returns"] {
        for c in ["services/billing", "services/audit"] {
            want.insert((s(p), s(c), "QUEUE_FLOWS", s("orders.created")));
        }
    }
    for t in ["services/users", "services/catalog"] {
        want.insert((s("web"), s(t), "GRAPHQL_CALLS", s("getUser")));
    }
    for t in ["services/chat", "services/notify"] {
        want.insert((s("web"), s(t), "WS_CONNECTS", s("/ws")));
    }
    assert_eq!(links, want);
    assert_eq!(map.links.len(), 10);
}

/// One contract row per resolver-made pair, each on the bare topic; the side
/// qnames still say which project each side is.
#[test]
fn message_contracts_read_the_bare_topic() {
    let (_td, r) = monorepo();
    let rows = message_contracts(&r.merged);
    assert_eq!(rows.len(), 4, "2 producers x 2 consumers");
    assert!(rows.iter().all(|row| row.topic == "orders.created"), "topic is owner-free");
    let sides: BTreeSet<(String, String)> = rows
        .iter()
        .map(|row| {
            (
                row.producer.as_ref().map(|p| p.qname.clone()).unwrap_or_default(),
                row.consumer.as_ref().map(|c| c.qname.clone()).unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(sides.len(), 4);
    assert!(sides.contains(&(
        s("queue_producer:orders.created @services/returns"),
        s("queue_consumer:orders.created @services/billing")
    )));
    assert!(rows.iter().all(|row| row.producer.as_ref().is_some_and(|p| p.topic == "orders.created")));
}

const PUBLISH: &str = "from kafka import KafkaProducer\n\nproducer = KafkaProducer()\n\n\n\
                       def place_order(order):\n    producer.send(\"orders.created\", order)\n";
const CONSUME: &str = "from kafka import KafkaConsumer\n\n\n\
                       def run():\n    for m in KafkaConsumer(\"orders.created\"):\n        print(m)\n";
const PROTO: &str = "syntax = \"proto3\";\n\npackage user;\n\nservice UserService {\n  \
                     rpc GetUser (GetUserRequest) returns (User);\n}\n\n\
                     message GetUserRequest {\n  string id = 1;\n}\n\n\
                     message User {\n  string id = 1;\n}\n";
const GO_CLIENT: &str = "package main\n\nimport (\n\t\"google.golang.org/grpc\"\n\tpb \"example.com/proto/user\"\n)\n\n\
                         func FetchUser(conn *grpc.ClientConn) {\n\tclient := pb.NewUserServiceClient(conn)\n\t_ = client\n}\n";

/// A tree whose only manifests sit at the root keeps today's qnames exactly.
#[test]
fn single_project_channels_unchanged() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    write(d, "pyproject.toml", "[project]\nname = \"shop\"\n");
    write(d, "go.mod", "module example.com/shop\n\ngo 1.21\n");
    write(d, "services/orders/publish.py", PUBLISH);
    write(d, "services/billing/consume.py", CONSUME);
    write(d, "services/gateway/main.go", GO_CLIENT);
    write(d, "proto/user.proto", PROTO);
    let m = build(d).merged;
    assert_eq!(qnames_of(&m, node_kind::QUEUE_PRODUCER), ["queue_producer:orders.created"]);
    assert_eq!(qnames_of(&m, node_kind::QUEUE_CONSUMER), ["queue_consumer:orders.created"]);
    assert_eq!(qnames_of(&m, node_kind::GRPC_CLIENT), ["grpc_client:UserService"]);
    assert_eq!(
        edges(&m, edge_category::QUEUE_FLOWS),
        [(s("queue_producer:orders.created"), s("queue_consumer:orders.created"))]
    );
    assert_eq!(
        edges(&m, edge_category::GRPC_CALLS),
        [(s("grpc_client:UserService"), s("grpc:user.UserService"))]
    );
}

/// Vendored copies of one `.proto` in two projects are ONE contract; each
/// project's client is its own side and calls it once. HEAD: one
/// GRPC_SERVICE and ONE GRPC_CLIENT.
#[test]
fn proto_contract_stays_shared() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    for svc in ["gateway", "admin"] {
        write(d, &format!("services/{svc}/go.mod"), &format!("module example.com/{svc}\n\ngo 1.21\n"));
        write(d, &format!("services/{svc}/proto/user.proto"), PROTO);
        write(d, &format!("services/{svc}/main.go"), GO_CLIENT);
    }
    let m = build(d).merged;
    let services = qnames_of(&m, node_kind::GRPC_SERVICE);
    assert_eq!(services, ["grpc:user.UserService"]);
    assert!(services.iter().all(|q| !q.contains(" @")), "a GRPC_SERVICE is never owned");
    assert_eq!(
        qnames_of(&m, node_kind::GRPC_CLIENT),
        ["grpc_client:UserService @services/admin", "grpc_client:UserService @services/gateway"]
    );
    assert_eq!(
        edges(&m, edge_category::GRPC_CALLS),
        [
            (s("grpc_client:UserService @services/admin"), s("grpc:user.UserService")),
            (s("grpc_client:UserService @services/gateway"), s("grpc:user.UserService")),
        ],
        "each client calls the one shared contract exactly once"
    );
}

/// The rule is by file: an SDL schema inside a project is that project's
/// server surface and takes its owner; one outside every project does not.
#[test]
fn sdl_fields_are_owned_by_their_file() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    write(d, "services/users/package.json", "{\"name\": \"users\"}\n");
    write(d, "services/users/schema.graphql", "type Query {\n  getUser(id: ID!): User\n}\n\ntype User {\n  id: ID!\n}\n");
    write(d, "schema/shared.graphql", "type Query {\n  listUsers: [String]\n}\n");
    let m = build(d).merged;
    let resolvers = qnames_of(&m, node_kind::GRAPHQL_RESOLVER);
    assert!(
        resolvers.contains(&s("graphql_resolver:getUser @services/users")),
        "an SDL field inside a project is owned: {resolvers:?}"
    );
    assert!(
        resolvers.contains(&s("graphql_resolver:listUsers")),
        "an SDL field outside every project stays unowned: {resolvers:?}"
    );
}

/// The owner pass runs post-cache, so an incremental build (cold, then warm
/// off the persisted parse cache) writes the same `.gmap` bytes as a clean one.
#[test]
fn incremental_matches_clean() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("mono");
    copy_tree(&fixture(), &repo);
    let path = repo.to_str().unwrap();

    let write_gmap = |r: &GenerateResult, name: &str| {
        let out = td.path().join(name);
        repo_graph_store::write_merged_sharded(&r.merged, &out).expect("write .gmap");
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&out)
            .unwrap()
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
            .collect();
        files.sort();
        files
    };
    let cold = generate_one_incremental(path).expect("cold incremental");
    let warm = generate_one_incremental(path).expect("warm incremental");
    let persisted = ParseCache::load(path);
    assert!(persisted.len() >= 10, "every parsed file cached: {}", persisted.len());
    let mut mem = ParseCache::new();
    generate_one_with_cache(path, &mut mem).expect("cold in-memory");
    let warm_mem = generate_one_with_cache(path, &mut mem).expect("warm in-memory");
    assert_eq!(mem.stats.reparsed, 0, "the second in-memory build reparses nothing");
    let clean = generate_one(path).expect("clean");
    assert!(
        qnames_of(&warm.merged, node_kind::QUEUE_PRODUCER)
            .contains(&s("queue_producer:orders.created @services/orders")),
        "the warm build qualifies cache-served parses too"
    );
    let clean_bytes = write_gmap(&clean, "clean");
    assert_eq!(write_gmap(&cold, "cold"), clean_bytes, "cold incremental == clean");
    assert_eq!(write_gmap(&warm, "warm"), clean_bytes, "warm incremental == clean");
    assert_eq!(write_gmap(&warm_mem, "warm_mem"), clean_bytes, "cache-served == clean");
}
