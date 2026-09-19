//! LC.3c: a cross edge from a tiered resolver names the rule that paired it.
//!
//! LC.3a stamps every cross edge `resolver:<name>` in the engine's Resolve
//! stage (`CODE_PASSES`, LD.13); the six resolvers with more than one matching rule
//! attach that evidence themselves, with the rule, and the stamp never
//! overrides it. Each test runs one resolver over hand-built graphs and reads
//! the EVIDENCE cell off each edge: the emitter must equal the engine's stamp
//! exactly, and the rule must name the tier, pass or branch that paired it.
//!
//! Builders are copied from `stack_resolvers.rs` rather than shared.

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CodeNav, GRAPH_TYPE, cell_type, edge_category, node_kind};
use glia_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};
use glia_graph::*;

fn repo(tag: &str) -> RepoId {
    RepoId::from_canonical(&format!("test://resolver_evidence/{tag}"))
}

/// One RepoGraph under construction: nodes by (kind, qname), plus the few
/// in-repo edges the WebSocket route join reads.
struct G {
    repo: RepoId,
    nodes: Vec<Node>,
    nav: CodeNav,
    edges: Vec<Edge>,
}

impl G {
    fn new(tag: &str) -> Self {
        Self {
            repo: repo(tag),
            nodes: vec![],
            nav: CodeNav::default(),
            edges: vec![],
        }
    }

    fn node(&mut self, kind: NodeKindId, qname: &str) -> NodeId {
        self.node_with(kind, qname, vec![])
    }

    fn node_with(&mut self, kind: NodeKindId, qname: &str, cells: Vec<Cell>) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
        self.nodes.push(Node {
            id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells,
        });
        let name = qname.rsplit([':', '.', '/']).next().unwrap_or(qname);
        self.nav.record(id, name, qname, kind, None);
        id
    }

    fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
        self.edges
            .push(Edge::new(from, to, category, Confidence::Strong));
    }

    fn build(self) -> RepoGraph {
        RepoGraph {
            repo: self.repo,
            nodes: self.nodes,
            edges: self.edges,
            nav: self.nav,
            symbols: Default::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        }
    }
}

fn json_cell(kind: glia_core::CellTypeId, json: &str) -> Cell {
    Cell {
        kind,
        payload: CellPayload::Json(json.to_string()),
    }
}

/// Run `resolver` over `graphs`.
fn resolve(graphs: Vec<RepoGraph>, resolver: &dyn CrossGraphResolver) -> MergedGraph {
    let mut merged = MergedGraph::new(graphs);
    resolver.resolve(&mut merged);
    merged
}

/// The one `category` cross edge `from -> to`.
fn edge(m: &MergedGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) -> &Edge {
    m.cross_edges
        .iter()
        .find(|e| e.from == from && e.to == to && e.category == category)
        .unwrap_or_else(|| {
            panic!(
                "no {category:?} edge {from:?} -> {to:?}: {:?}",
                m.cross_edges
            )
        })
}

/// `(emitter, rule)` of the edge's evidence, asserting it carries exactly one
/// EVIDENCE cell and no location (the engine's fill pass places it).
fn rule_of(e: &Edge) -> (String, Option<String>) {
    let n = e
        .cells
        .iter()
        .filter(|c| c.kind == cell_type::EVIDENCE)
        .count();
    assert_eq!(n, 1, "exactly one EVIDENCE cell: {e:?}");
    let ev = Evidence::of(e).unwrap_or_else(|| panic!("unreadable evidence: {e:?}"));
    assert_eq!((ev.file.as_deref(), ev.line), (None, None), "{ev:?}");
    (ev.emitter, ev.rule)
}

fn assert_rule(e: &Edge, emitter: &str, rule: &str) {
    assert_eq!(
        rule_of(e),
        (emitter.to_string(), Some(rule.to_string())),
        "{e:?}"
    );
}

// ============================================================================
// HTTP — the six MatchTiers
// ============================================================================

fn route(g: &mut G, method: &str, path: &str) -> NodeId {
    let qname = format!("{method} {path}");
    let cell = Cell {
        kind: cell_type::ROUTE_METHOD,
        payload: CellPayload::Text(method.to_string()),
    };
    g.node_with(node_kind::ROUTE, &qname, vec![cell])
}

fn endpoint(g: &mut G, method: &str, path: &str) -> NodeId {
    g.node(node_kind::ENDPOINT, &format!("endpoint:{method}:{path}"))
}

#[test]
fn http_edge_names_its_tier() {
    let mut s = G::new("http-server");
    let users = route(&mut s, "GET", "/users");
    let orders = route(&mut s, "GET", "/orders");
    let posts = route(&mut s, "ANY", "/posts");
    let items = route(&mut s, "GET", "/api/items");
    let carts = route(&mut s, "GET", "/carts");
    let mut c = G::new("http-client");
    let exact = endpoint(&mut c, "GET", "/users");
    let eprefix = endpoint(&mut c, "GET", "/api/orders");
    let any = endpoint(&mut c, "GET", "/posts");
    let rprefix = endpoint(&mut c, "GET", "/items");
    let base = endpoint(&mut c, "GET", "${…}/carts");
    // Only the suffix tier reaches `/users` from here: a base URL, then a
    // segment no route and no API prefix names.
    let suffix = endpoint(&mut c, "GET", "${…}/tenant/users");
    let m = resolve(vec![s.build(), c.build()], &HttpStackResolver);

    let cat = edge_category::HTTP_CALLS;
    assert_rule(edge(&m, exact, users, cat), "resolver:http", "exact");
    assert_rule(
        edge(&m, eprefix, orders, cat),
        "resolver:http",
        "endpoint_prefix",
    );
    assert_rule(edge(&m, any, posts, cat), "resolver:http", "any");
    assert_rule(
        edge(&m, rprefix, items, cat),
        "resolver:http",
        "route_prefix",
    );
    assert_rule(edge(&m, base, carts, cat), "resolver:http", "base_fold");
    let e = edge(&m, suffix, users, cat);
    assert_rule(e, "resolver:http", "suffix");
    assert_eq!(
        e.confidence,
        Confidence::Weak,
        "the tier still drives confidence"
    );
    assert_eq!(m.cross_edges.len(), 6, "{:?}", m.cross_edges);
}

// ============================================================================
// gRPC — package narrowing, client and server halves
// ============================================================================

fn rpc_package(json: &str) -> Cell {
    json_cell(cell_type::RPC_PACKAGE, json)
}

#[test]
fn grpc_edge_names_narrowing() {
    // One service with a unique name (all), and `PaymentsService` declared in
    // two packages that the client's go_package import narrows (narrowed).
    let mut s = G::new("grpc-server");
    let greeter = s.node_with(
        node_kind::GRPC_SERVICE,
        "grpc:helloworld.Greeter",
        vec![rpc_package(r#"{"package":"helloworld"}"#)],
    );
    let billing = s.node_with(
        node_kind::GRPC_SERVICE,
        "grpc:billing.PaymentsService",
        vec![rpc_package(
            r#"{"package":"billing","go_package":"example.com/gen/billing"}"#,
        )],
    );
    s.node_with(
        node_kind::GRPC_SERVICE,
        "grpc:legacy.PaymentsService",
        vec![rpc_package(
            r#"{"package":"legacy","go_package":"example.com/gen/legacy"}"#,
        )],
    );
    let greeter_impl = s.node(node_kind::GRPC_SERVER, "grpc_server:Greeter");
    let payments_impl = s.node_with(
        node_kind::GRPC_SERVER,
        "grpc_server:PaymentsService",
        vec![rpc_package(r#"{"imports":["example.com/gen/legacy"]}"#)],
    );
    let mut c = G::new("grpc-client");
    let greeter_client = c.node(node_kind::GRPC_CLIENT, "grpc_client:Greeter");
    let payments_client = c.node_with(
        node_kind::GRPC_CLIENT,
        "grpc_client:PaymentsService",
        vec![rpc_package(
            r#"{"imports":["context","google.golang.org/grpc","example.com/gen/billing"]}"#,
        )],
    );
    let m = resolve(vec![s.build(), c.build()], &GrpcStackResolver);
    let legacy = NodeId::from_parts(
        GRAPH_TYPE,
        repo("grpc-server"),
        node_kind::GRPC_SERVICE,
        "grpc:legacy.PaymentsService",
    );

    let calls = edge_category::GRPC_CALLS;
    assert_rule(
        edge(&m, greeter_client, greeter, calls),
        "resolver:grpc",
        "all",
    );
    assert_rule(
        edge(&m, payments_client, billing, calls),
        "resolver:grpc",
        "narrowed",
    );
    let handled = edge_category::HANDLED_BY;
    assert_rule(
        edge(&m, greeter, greeter_impl, handled),
        "resolver:grpc",
        "server_all",
    );
    assert_rule(
        edge(&m, legacy, payments_impl, handled),
        "resolver:grpc",
        "server_narrowed",
    );
    assert_eq!(m.cross_edges.len(), 4, "{:?}", m.cross_edges);
}

// ============================================================================
// Queue — exact topic vs compiled pattern
// ============================================================================

/// The A2.8 provenance cell a queue side carries, in `queues::finish`'s shape.
fn family(family: &str) -> Cell {
    json_cell(
        cell_type::CODE,
        &format!(
            r#"{{"framework":"X","family":"{family}","sites":[{{"file":"src/a.go","line":3}}]}}"#
        ),
    )
}

#[test]
fn queue_edge_names_exact_or_pattern() {
    let mut s = G::new("queue-consumer");
    let wild = s.node_with(
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:orders.*",
        vec![family("nats")],
    );
    let lit = s.node_with(
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:orders.created",
        vec![family("nats")],
    );
    let mut c = G::new("queue-producer");
    let producer = c.node_with(
        node_kind::QUEUE_PRODUCER,
        "queue_producer:orders.created",
        vec![family("nats")],
    );
    let m = resolve(vec![s.build(), c.build()], &QueueStackResolver);

    let cat = edge_category::QUEUE_FLOWS;
    assert_rule(edge(&m, producer, lit, cat), "resolver:queue", "exact");
    let e = edge(&m, producer, wild, cat);
    assert_rule(e, "resolver:queue", "pattern");
    assert_eq!(e.confidence, Confidence::Weak);
    assert_eq!(m.cross_edges.len(), 2, "{:?}", m.cross_edges);
}

// ============================================================================
// WebSocket — exact / suffix / param / inherited / wildcard
// ============================================================================

#[test]
fn ws_edge_names_exact_or_suffix() {
    let mut s = G::new("ws-server");
    let chat = s.node(node_kind::WS_HANDLER, "ws:/chat");
    let room = s.node(node_kind::WS_HANDLER, "ws:/rooms/{room}");
    let topic = s.node(node_kind::WS_HANDLER, "ws:room:*");
    // A generic upgrade (`ws:ws`) whose owning function serves `route:/live`.
    let serve = s.node(node_kind::FUNCTION, "hub::ServeWs");
    let live = s.node(node_kind::ROUTE, "route:/live");
    let generic = s.node(node_kind::WS_HANDLER, "ws:ws");
    s.edge(live, serve, edge_category::HANDLED_BY);
    s.edge(generic, serve, edge_category::HANDLED_BY);
    let mut c = G::new("ws-client");
    let exact = c.node(node_kind::WS_CLIENT, "ws_client:/chat");
    let suffix = c.node(node_kind::WS_CLIENT, "ws_client:/api/v1/chat");
    let param = c.node(node_kind::WS_CLIENT, "ws_client:/rooms/lobby");
    let wildcard = c.node(node_kind::WS_CLIENT, "ws_client:room:lobby");
    let inherited = c.node(node_kind::WS_CLIENT, "ws_client:/live");
    let m = resolve(vec![s.build(), c.build()], &WebSocketStackResolver);

    let cat = edge_category::WS_CONNECTS;
    assert_rule(edge(&m, exact, chat, cat), "resolver:websocket", "exact");
    assert_rule(edge(&m, suffix, chat, cat), "resolver:websocket", "suffix");
    assert_rule(edge(&m, param, room, cat), "resolver:websocket", "param");
    assert_rule(
        edge(&m, wildcard, topic, cat),
        "resolver:websocket",
        "wildcard",
    );
    assert_rule(
        edge(&m, inherited, generic, cat),
        "resolver:websocket",
        "inherited",
    );
    assert_eq!(m.cross_edges.len(), 5, "{:?}", m.cross_edges);
}

// ============================================================================
// EventBus — exact key vs type-folded key
// ============================================================================

#[test]
fn eventbus_edge_names_exact_or_folded() {
    let mut s = G::new("event-handler");
    let created = s.node(node_kind::EVENT_HANDLER, "event_handle:user.created");
    let placed = s.node(node_kind::EVENT_HANDLER, "event_handle:OrderPlaced");
    let mut c = G::new("event-emitter");
    let exact = c.node(node_kind::EVENT_EMITTER, "event_emit:user.created");
    let folded = c.node(node_kind::EVENT_EMITTER, "event_emit:OrderPlacedEvent");
    let m = resolve(vec![s.build(), c.build()], &EventBusResolver);

    let cat = edge_category::EVENT_FLOWS;
    assert_rule(edge(&m, exact, created, cat), "resolver:eventbus", "exact");
    assert_rule(edge(&m, folded, placed, cat), "resolver:eventbus", "folded");
    assert_eq!(m.cross_edges.len(), 2, "{:?}", m.cross_edges);
}

// ============================================================================
// DB — entity (exact qname) / entity_fold (canonical name) / provider
// ============================================================================

#[test]
fn db_edge_names_entity_or_provider() {
    let mut a = G::new("db-a");
    let users_a = a.node(node_kind::DATA_ENTITY, "data_entity:sql:users");
    let orders_a = a.node(node_kind::DATA_ENTITY, "data_entity:sql:orders");
    let redis_a = a.node(node_kind::CACHE, "data_source:redis");
    let mut b = G::new("db-b");
    let users_b = b.node(node_kind::DATA_ENTITY, "data_entity:sql:users");
    let order_b = b.node(node_kind::DATA_ENTITY, "data_entity:sql:Order");
    let redis_b = b.node(node_kind::CACHE, "data_source:redis");
    let m = resolve(vec![a.build(), b.build()], &DbResolver);

    let entity = edge_category::SHARES_DATA_ENTITY;
    assert_rule(edge(&m, users_a, users_b, entity), "resolver:db", "entity");
    let fold = m
        .cross_edges
        .iter()
        .find(|e| {
            e.category == entity
                && ((e.from, e.to) == (orders_a, order_b) || (e.from, e.to) == (order_b, orders_a))
        })
        .unwrap_or_else(|| panic!("no fold edge: {:?}", m.cross_edges));
    assert_rule(fold, "resolver:db", "entity_fold");
    assert_eq!(fold.confidence, Confidence::Weak);
    assert_rule(
        edge(&m, redis_a, redis_b, edge_category::SHARES_DATA_SOURCE),
        "resolver:db",
        "provider",
    );
    assert_eq!(m.cross_edges.len(), 3, "{:?}", m.cross_edges);
}

/// The single-rule pairwise resolver sharing DB's pair emitter attaches no
/// evidence of its own: LC.3a's emitter-only stamp is its whole story.
#[test]
fn message_schema_edge_carries_no_rule_of_its_own() {
    let mut a = G::new("msg-a");
    let ua = a.node(node_kind::MESSAGE_TYPE, "message:proto:shop.User");
    let mut b = G::new("msg-b");
    let ub = b.node(node_kind::MESSAGE_TYPE, "message:proto:shop.User");
    let m = resolve(vec![a.build(), b.build()], &MessageSchemaResolver);
    let e = edge(&m, ua, ub, edge_category::SHARES_SCHEMA);
    assert!(Evidence::of(e).is_none(), "{e:?}");
}
