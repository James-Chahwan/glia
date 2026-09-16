use repo_graph_code_domain::{CodeNav, edge_category, node_kind, GRAPH_TYPE};
use repo_graph_core::{Confidence, Node, NodeId, RepoId};
use repo_graph_graph::*;

fn repo_a() -> RepoId {
    RepoId::from_canonical("test://resolver/a")
}
fn repo_b() -> RepoId {
    RepoId::from_canonical("test://resolver/b")
}

fn make_graph(repo: RepoId, nodes: Vec<Node>, nav: CodeNav) -> RepoGraph {
    RepoGraph {
        repo,
        nodes,
        edges: vec![],
        nav,
        symbols: Default::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    }
}

fn make_node(repo: RepoId, kind: repo_graph_core::NodeKindId, qname: &str, confidence: Confidence) -> (Node, NodeId) {
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, qname);
    let node = Node {
        id,
        repo,
        confidence,
        cells: vec![],
    };
    (node, id)
}

fn record(nav: &mut CodeNav, id: NodeId, name: &str, qname: &str, kind: repo_graph_core::NodeKindId) {
    nav.record(id, name, qname, kind, None);
}

// ============================================================================
// GrpcStackResolver
// ============================================================================

#[test]
fn grpc_resolver_links_client_to_service() {
    let mut nav_a = CodeNav::default();
    let (svc_node, svc_id) = make_node(repo_a(), node_kind::GRPC_SERVICE, "grpc:UserService", Confidence::Strong);
    record(&mut nav_a, svc_id, "UserService", "grpc:UserService", node_kind::GRPC_SERVICE);
    let ga = make_graph(repo_a(), vec![svc_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::GRPC_CLIENT, "grpc_client:UserService", Confidence::Medium);
    record(&mut nav_b, client_id, "UserService", "grpc_client:UserService", node_kind::GRPC_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    GrpcStackResolver.resolve(&mut merged);

    let grpc_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::GRPC_CALLS).collect();
    assert_eq!(grpc_edges.len(), 1);
    assert_eq!(grpc_edges[0].from, client_id);
    assert_eq!(grpc_edges[0].to, svc_id);
    assert_eq!(grpc_edges[0].confidence, Confidence::Medium);
}

#[test]
fn grpc_resolver_pairs_bare_client_to_package_qualified_service() {
    // A5.1: the proto service qname is now package-qualified, but a client
    // stub reconstructed from generated code only knows the bare name. The
    // index's bare-last-segment fallback keeps the pairing alive.
    let mut nav_a = CodeNav::default();
    let (svc_node, svc_id) = make_node(repo_a(), node_kind::GRPC_SERVICE, "grpc:user.UserService", Confidence::Strong);
    record(&mut nav_a, svc_id, "UserService", "grpc:user.UserService", node_kind::GRPC_SERVICE);
    let ga = make_graph(repo_a(), vec![svc_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::GRPC_CLIENT, "grpc_client:UserService", Confidence::Medium);
    record(&mut nav_b, client_id, "UserService", "grpc_client:UserService", node_kind::GRPC_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    GrpcStackResolver.resolve(&mut merged);

    let grpc_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::GRPC_CALLS).collect();
    assert_eq!(grpc_edges.len(), 1, "exactly one edge — the dual key must not double-emit");
    assert_eq!(grpc_edges[0].from, client_id);
    assert_eq!(grpc_edges[0].to, svc_id);
}

#[test]
fn grpc_resolver_method_level_matches_service() {
    let mut nav_a = CodeNav::default();
    let (svc_node, svc_id) = make_node(repo_a(), node_kind::GRPC_SERVICE, "grpc:OrderService", Confidence::Strong);
    record(&mut nav_a, svc_id, "OrderService", "grpc:OrderService", node_kind::GRPC_SERVICE);
    let ga = make_graph(repo_a(), vec![svc_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::GRPC_CLIENT, "grpc_client:OrderService.PlaceOrder", Confidence::Medium);
    record(&mut nav_b, client_id, "OrderService", "grpc_client:OrderService.PlaceOrder", node_kind::GRPC_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    GrpcStackResolver.resolve(&mut merged);

    assert_eq!(merged.cross_edges.len(), 1);
}

// ============================================================================
// QueueStackResolver
// ============================================================================

#[test]
fn queue_resolver_links_producer_to_consumer() {
    let mut nav_a = CodeNav::default();
    let (consumer_node, consumer_id) = make_node(repo_a(), node_kind::QUEUE_CONSUMER, "queue_consumer:emails", Confidence::Medium);
    record(&mut nav_a, consumer_id, "emails", "queue_consumer:emails", node_kind::QUEUE_CONSUMER);
    let ga = make_graph(repo_a(), vec![consumer_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (producer_node, producer_id) = make_node(repo_b(), node_kind::QUEUE_PRODUCER, "queue_producer:emails", Confidence::Medium);
    record(&mut nav_b, producer_id, "emails", "queue_producer:emails", node_kind::QUEUE_PRODUCER);
    let gb = make_graph(repo_b(), vec![producer_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    QueueStackResolver.resolve(&mut merged);

    let queue_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::QUEUE_FLOWS).collect();
    assert_eq!(queue_edges.len(), 1);
    assert_eq!(queue_edges[0].from, producer_id);
    assert_eq!(queue_edges[0].to, consumer_id);
}

#[test]
fn queue_resolver_no_match_on_different_topics() {
    let mut nav_a = CodeNav::default();
    let (consumer_node, consumer_id) = make_node(repo_a(), node_kind::QUEUE_CONSUMER, "queue_consumer:emails", Confidence::Medium);
    record(&mut nav_a, consumer_id, "emails", "queue_consumer:emails", node_kind::QUEUE_CONSUMER);
    let ga = make_graph(repo_a(), vec![consumer_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (producer_node, producer_id) = make_node(repo_b(), node_kind::QUEUE_PRODUCER, "queue_producer:orders", Confidence::Medium);
    record(&mut nav_b, producer_id, "orders", "queue_producer:orders", node_kind::QUEUE_PRODUCER);
    let gb = make_graph(repo_b(), vec![producer_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    QueueStackResolver.resolve(&mut merged);

    assert!(merged.cross_edges.is_empty());
}

#[test]
fn queue_resolver_ignores_unresolved_tag_nodes() {
    // A2.3 — THE false-pairing case. Two unrelated services whose Kafka topics
    // were both unreadable fall back to the extractor's framework tag. The tag
    // is an identical string on both sides, so before this guard the resolver
    // joined them and manufactured a cross-service QUEUE_FLOWS edge that
    // blast_radius traverses and cross_stack_trace labels as a real mechanism.
    let mut nav_a = CodeNav::default();
    let (consumer_node, consumer_id) = make_node(
        repo_a(),
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:unresolved:kafka",
        Confidence::Weak,
    );
    record(
        &mut nav_a,
        consumer_id,
        "unresolved:kafka",
        "queue_consumer:unresolved:kafka",
        node_kind::QUEUE_CONSUMER,
    );
    let ga = make_graph(repo_a(), vec![consumer_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (producer_node, producer_id) = make_node(
        repo_b(),
        node_kind::QUEUE_PRODUCER,
        "queue_producer:unresolved:kafka",
        Confidence::Weak,
    );
    record(
        &mut nav_b,
        producer_id,
        "unresolved:kafka",
        "queue_producer:unresolved:kafka",
        node_kind::QUEUE_PRODUCER,
    );
    let gb = make_graph(repo_b(), vec![producer_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    QueueStackResolver.resolve(&mut merged);

    assert!(
        merged.cross_edges.is_empty(),
        "a topic-agnostic framework tag must never pair with anything, got {:?}",
        merged.cross_edges
    );
}

#[test]
fn queue_resolver_unresolved_producer_cannot_reach_a_real_consumer() {
    // The guard is per-SIDE: an unresolved producer must not fall through to a
    // consumer that DID name a topic, and a real producer must not be dragged
    // into the sentinel either. Both directions in one graph.
    let mut nav_a = CodeNav::default();
    let (real_consumer, real_consumer_id) = make_node(
        repo_a(),
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:orders",
        Confidence::Medium,
    );
    record(&mut nav_a, real_consumer_id, "orders", "queue_consumer:orders", node_kind::QUEUE_CONSUMER);
    let (tag_consumer, tag_consumer_id) = make_node(
        repo_a(),
        node_kind::QUEUE_CONSUMER,
        "queue_consumer:unresolved:kafka",
        Confidence::Weak,
    );
    record(
        &mut nav_a,
        tag_consumer_id,
        "unresolved:kafka",
        "queue_consumer:unresolved:kafka",
        node_kind::QUEUE_CONSUMER,
    );
    let ga = make_graph(repo_a(), vec![real_consumer, tag_consumer], nav_a);

    let mut nav_b = CodeNav::default();
    let (real_producer, real_producer_id) = make_node(
        repo_b(),
        node_kind::QUEUE_PRODUCER,
        "queue_producer:orders",
        Confidence::Medium,
    );
    record(&mut nav_b, real_producer_id, "orders", "queue_producer:orders", node_kind::QUEUE_PRODUCER);
    let (tag_producer, tag_producer_id) = make_node(
        repo_b(),
        node_kind::QUEUE_PRODUCER,
        "queue_producer:unresolved:kafka",
        Confidence::Weak,
    );
    record(
        &mut nav_b,
        tag_producer_id,
        "unresolved:kafka",
        "queue_producer:unresolved:kafka",
        node_kind::QUEUE_PRODUCER,
    );
    let gb = make_graph(repo_b(), vec![real_producer, tag_producer], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    QueueStackResolver.resolve(&mut merged);

    // Exactly the one real pairing survives — the named topic still resolves.
    let queue_edges: Vec<_> = merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::QUEUE_FLOWS)
        .collect();
    assert_eq!(queue_edges.len(), 1, "got {queue_edges:?}");
    assert_eq!(queue_edges[0].from, real_producer_id);
    assert_eq!(queue_edges[0].to, real_consumer_id);
}

// ============================================================================
// GraphQLStackResolver
// ============================================================================

#[test]
fn graphql_resolver_links_operation_to_resolver() {
    let mut nav_a = CodeNav::default();
    let (resolver_node, resolver_id) = make_node(repo_a(), node_kind::GRAPHQL_RESOLVER, "graphql_resolver:Mutation", Confidence::Strong);
    record(&mut nav_a, resolver_id, "Mutation", "graphql_resolver:Mutation", node_kind::GRAPHQL_RESOLVER);
    let ga = make_graph(repo_a(), vec![resolver_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (op_node, op_id) = make_node(repo_b(), node_kind::GRAPHQL_OPERATION, "graphql_op:CreateUserMutation", Confidence::Medium);
    record(&mut nav_b, op_id, "CreateUserMutation", "graphql_op:CreateUserMutation", node_kind::GRAPHQL_OPERATION);
    let gb = make_graph(repo_b(), vec![op_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    GraphQLStackResolver.resolve(&mut merged);

    let gql_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::GRAPHQL_CALLS).collect();
    assert_eq!(gql_edges.len(), 1);
    assert_eq!(gql_edges[0].from, op_id);
    assert_eq!(gql_edges[0].to, resolver_id);
}

// ============================================================================
// WebSocketStackResolver
// ============================================================================

#[test]
fn ws_resolver_links_client_to_handler() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::WS_HANDLER, "ws:/chat", Confidence::Strong);
    record(&mut nav_a, handler_id, "chat", "ws:/chat", node_kind::WS_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::WS_CLIENT, "ws_client:/chat", Confidence::Medium);
    record(&mut nav_b, client_id, "chat", "ws_client:/chat", node_kind::WS_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    WebSocketStackResolver.resolve(&mut merged);

    let ws_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::WS_CONNECTS).collect();
    assert_eq!(ws_edges.len(), 1);
    assert_eq!(ws_edges[0].from, client_id);
    assert_eq!(ws_edges[0].to, handler_id);
}

/// A5.6: the WS extractor falls back to the literal name `ws` for any handler
/// whose path it cannot read. That name used to pair with EVERY client in the
/// merge via an unconditional wildcard term in `ws_paths_match`.
#[test]
fn ws_resolver_does_not_wildcard_unparsed_handler() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::WS_HANDLER, "ws:ws", Confidence::Medium);
    record(&mut nav_a, handler_id, "ws", "ws:ws", node_kind::WS_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::WS_CLIENT, "ws_client:/notifications", Confidence::Medium);
    record(&mut nav_b, client_id, "/notifications", "ws_client:/notifications", node_kind::WS_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    WebSocketStackResolver.resolve(&mut merged);

    let ws_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::WS_CONNECTS).collect();
    assert!(ws_edges.is_empty(), "generic handler name must not fan out to unrelated clients");
}

/// `@WebSocketGateway` / `Phoenix.Channel` / `ActionCable` handlers are named
/// `default` because they carry no path; that is not a licence to pair either.
#[test]
fn ws_resolver_does_not_wildcard_default_handler() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::WS_HANDLER, "ws:default", Confidence::Medium);
    record(&mut nav_a, handler_id, "default", "ws:default", node_kind::WS_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::WS_CLIENT, "ws_client:/notifications", Confidence::Medium);
    record(&mut nav_b, client_id, "/notifications", "ws_client:/notifications", node_kind::WS_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    WebSocketStackResolver.resolve(&mut merged);

    assert!(
        merged.cross_edges.iter().all(|e| e.category != edge_category::WS_CONNECTS),
        "handler named `default` must not pair with an unrelated client path"
    );
}

/// Suffix matching is kept, but on segment boundaries: a client mounted under a
/// prefix still reaches the handler it actually talks to.
#[test]
fn ws_resolver_suffix_matches_on_segment_boundary() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::WS_HANDLER, "ws:/chat", Confidence::Strong);
    record(&mut nav_a, handler_id, "chat", "ws:/chat", node_kind::WS_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::WS_CLIENT, "ws_client:/api/v1/chat", Confidence::Medium);
    record(&mut nav_b, client_id, "/api/v1/chat", "ws_client:/api/v1/chat", node_kind::WS_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    WebSocketStackResolver.resolve(&mut merged);

    let ws_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::WS_CONNECTS).collect();
    assert_eq!(ws_edges.len(), 1);
    assert_eq!(ws_edges[0].from, client_id);
    assert_eq!(ws_edges[0].to, handler_id);
}

/// The old `ends_with` was byte-level, so `/news` "ended with" a handler named
/// `ws`. Segment slicing kills that class of pairing outright.
#[test]
fn ws_resolver_rejects_byte_suffix() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::WS_HANDLER, "ws:/socket", Confidence::Strong);
    record(&mut nav_a, handler_id, "socket", "ws:/socket", node_kind::WS_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, client_id) = make_node(repo_b(), node_kind::WS_CLIENT, "ws_client:/websocket", Confidence::Medium);
    record(&mut nav_b, client_id, "/websocket", "ws_client:/websocket", node_kind::WS_CLIENT);
    let gb = make_graph(repo_b(), vec![client_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    WebSocketStackResolver.resolve(&mut merged);

    assert!(
        merged.cross_edges.iter().all(|e| e.category != edge_category::WS_CONNECTS),
        "`/websocket` byte-ends-with `socket` but is a different mount point"
    );
}

// ============================================================================
// EventBusResolver
// ============================================================================

#[test]
fn event_resolver_links_emitter_to_handler() {
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::EVENT_HANDLER, "event_handle:user.created", Confidence::Weak);
    record(&mut nav_a, handler_id, "user.created", "event_handle:user.created", node_kind::EVENT_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (emitter_node, emitter_id) = make_node(repo_b(), node_kind::EVENT_EMITTER, "event_emit:user.created", Confidence::Weak);
    record(&mut nav_b, emitter_id, "user.created", "event_emit:user.created", node_kind::EVENT_EMITTER);
    let gb = make_graph(repo_b(), vec![emitter_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    EventBusResolver.resolve(&mut merged);

    let event_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::EVENT_FLOWS).collect();
    assert_eq!(event_edges.len(), 1);
    assert_eq!(event_edges[0].from, emitter_id);
    assert_eq!(event_edges[0].to, handler_id);
    assert_eq!(event_edges[0].confidence, Confidence::Weak);
}

#[test]
fn event_resolver_folds_event_suffix() {
    // Spring publishes `OrderPlacedEvent`; the listener's parameter is often
    // spelled `OrderPlaced`. Type-named keys fold to one event.
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::EVENT_HANDLER, "event_handle:OrderPlaced", Confidence::Medium);
    record(&mut nav_a, handler_id, "OrderPlaced", "event_handle:OrderPlaced", node_kind::EVENT_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (emitter_node, emitter_id) = make_node(repo_b(), node_kind::EVENT_EMITTER, "event_emit:OrderPlacedEvent", Confidence::Medium);
    record(&mut nav_b, emitter_id, "OrderPlacedEvent", "event_emit:OrderPlacedEvent", node_kind::EVENT_EMITTER);
    let gb = make_graph(repo_b(), vec![emitter_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    EventBusResolver.resolve(&mut merged);

    let event_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::EVENT_FLOWS).collect();
    assert_eq!(event_edges.len(), 1, "{:?}", merged.cross_edges);
    assert_eq!(event_edges[0].from, emitter_id);
    assert_eq!(event_edges[0].to, handler_id);
}

#[test]
fn event_resolver_indexes_bare_code_qnames() {
    // The Solidity parser emits EVENT_* nodes with real code qnames, not the
    // extractor's `event_handle:` prefix. Those used to be `continue`d out of
    // the index, which made every parser-emitted event a disconnected island.
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::EVENT_HANDLER, "notify::Listener::OrderPlaced", Confidence::Medium);
    record(&mut nav_a, handler_id, "OrderPlaced", "notify::Listener::OrderPlaced", node_kind::EVENT_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (emitter_node, emitter_id) = make_node(repo_b(), node_kind::EVENT_EMITTER, "event_emit:OrderPlaced", Confidence::Medium);
    record(&mut nav_b, emitter_id, "OrderPlaced", "event_emit:OrderPlaced", node_kind::EVENT_EMITTER);
    let gb = make_graph(repo_b(), vec![emitter_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    EventBusResolver.resolve(&mut merged);

    let event_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::EVENT_FLOWS).collect();
    assert_eq!(event_edges.len(), 1, "{:?}", merged.cross_edges);
    assert_eq!(event_edges[0].from, emitter_id);
    assert_eq!(event_edges[0].to, handler_id);
}

#[test]
fn event_resolver_keeps_string_topics_exact() {
    // The fold is guarded to type-shaped keys. A dotted lowercase topic is not
    // one, so `user.created` must never reach `user.updated` — no separator
    // folding, no lowercasing, no all-to-all.
    let mut nav_a = CodeNav::default();
    let (handler_node, handler_id) = make_node(repo_a(), node_kind::EVENT_HANDLER, "event_handle:user.updated", Confidence::Weak);
    record(&mut nav_a, handler_id, "user.updated", "event_handle:user.updated", node_kind::EVENT_HANDLER);
    let ga = make_graph(repo_a(), vec![handler_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (emitter_node, emitter_id) = make_node(repo_b(), node_kind::EVENT_EMITTER, "event_emit:user.created", Confidence::Weak);
    record(&mut nav_b, emitter_id, "user.created", "event_emit:user.created", node_kind::EVENT_EMITTER);
    let gb = make_graph(repo_b(), vec![emitter_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    EventBusResolver.resolve(&mut merged);

    assert!(
        merged.cross_edges.is_empty(),
        "two different string topics must not pair, got {:?}",
        merged.cross_edges
    );
}

// ============================================================================
// CliInvocationResolver
// ============================================================================

#[test]
fn cli_resolver_links_invocation_to_command() {
    let mut nav_a = CodeNav::default();
    let (cmd_node, cmd_id) = make_node(repo_a(), node_kind::CLI_COMMAND, "cli:migrate", Confidence::Strong);
    record(&mut nav_a, cmd_id, "migrate", "cli:migrate", node_kind::CLI_COMMAND);
    let ga = make_graph(repo_a(), vec![cmd_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (inv_node, inv_id) = make_node(repo_b(), node_kind::CLI_INVOCATION, "cli_invoke:migrate", Confidence::Medium);
    record(&mut nav_b, inv_id, "migrate", "cli_invoke:migrate", node_kind::CLI_INVOCATION);
    let gb = make_graph(repo_b(), vec![inv_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    CliInvocationResolver.resolve(&mut merged);

    let cli_edges: Vec<_> = merged.cross_edges.iter().filter(|e| e.category == edge_category::CLI_INVOKES).collect();
    assert_eq!(cli_edges.len(), 1);
    assert_eq!(cli_edges[0].from, inv_id);
    assert_eq!(cli_edges[0].to, cmd_id);
}

// ============================================================================
// All resolvers run together without interference
// ============================================================================

#[test]
fn all_resolvers_compose_cleanly() {
    let mut nav_a = CodeNav::default();
    let (svc_node, svc_id) = make_node(repo_a(), node_kind::GRPC_SERVICE, "grpc:Auth", Confidence::Strong);
    record(&mut nav_a, svc_id, "Auth", "grpc:Auth", node_kind::GRPC_SERVICE);
    let (consumer_node, consumer_id) = make_node(repo_a(), node_kind::QUEUE_CONSUMER, "queue_consumer:jobs", Confidence::Medium);
    record(&mut nav_a, consumer_id, "jobs", "queue_consumer:jobs", node_kind::QUEUE_CONSUMER);
    let (cmd_node, cmd_id) = make_node(repo_a(), node_kind::CLI_COMMAND, "cli:seed", Confidence::Strong);
    record(&mut nav_a, cmd_id, "seed", "cli:seed", node_kind::CLI_COMMAND);
    let ga = make_graph(repo_a(), vec![svc_node, consumer_node, cmd_node], nav_a);

    let mut nav_b = CodeNav::default();
    let (client_node, _) = make_node(repo_b(), node_kind::GRPC_CLIENT, "grpc_client:Auth", Confidence::Medium);
    record(&mut nav_b, client_node.id, "Auth", "grpc_client:Auth", node_kind::GRPC_CLIENT);
    let (producer_node, _) = make_node(repo_b(), node_kind::QUEUE_PRODUCER, "queue_producer:jobs", Confidence::Medium);
    record(&mut nav_b, producer_node.id, "jobs", "queue_producer:jobs", node_kind::QUEUE_PRODUCER);
    let (inv_node, _) = make_node(repo_b(), node_kind::CLI_INVOCATION, "cli_invoke:seed", Confidence::Medium);
    record(&mut nav_b, inv_node.id, "seed", "cli_invoke:seed", node_kind::CLI_INVOCATION);
    let gb = make_graph(repo_b(), vec![client_node, producer_node, inv_node], nav_b);

    let mut merged = MergedGraph::new(vec![ga, gb]);
    merged.run(&GrpcStackResolver);
    merged.run(&QueueStackResolver);
    merged.run(&CliInvocationResolver);

    assert_eq!(merged.cross_edges.len(), 3);
    assert!(merged.cross_edges.iter().any(|e| e.category == edge_category::GRPC_CALLS));
    assert!(merged.cross_edges.iter().any(|e| e.category == edge_category::QUEUE_FLOWS));
    assert!(merged.cross_edges.iter().any(|e| e.category == edge_category::CLI_INVOKES));
}

// ============================================================================
// HttpStackResolver — the tier ladder (A3.1)
// ============================================================================
//
// Before A3.1 the matcher was one exact HashMap hit plus an endpoint-side-only
// strip of up to two API prefixes. Three whole classes of real pairing could
// never form: a method-agnostic (`ANY`) server route, a server mounted behind a
// prefix the client does not know about, and a client whose path starts with an
// interpolated base URL. Each tier below is one of those, and the ladder returns
// the FIRST tier that hits — never a union, which is where fan-out would live.

/// A `ROUTE_METHOD` cell in the plain-Text shape parser-ruby / -java / -csharp /
/// -dart emit. Fully qualified rather than imported so this block does not touch
/// the file's shared `use` lines.
fn route_method_cell(method: &str) -> repo_graph_core::Cell {
    repo_graph_core::Cell {
        kind: repo_graph_code_domain::cell_type::ROUTE_METHOD,
        payload: repo_graph_core::CellPayload::Text(method.to_string()),
    }
}

/// The ORIGIN cell A3.4's client-router extractors stamp on a browser
/// navigation ROUTE. Spelled as a literal on purpose — see `http_nav_route.rs`.
fn nav_route_cell() -> repo_graph_core::Cell {
    repo_graph_core::Cell {
        kind: repo_graph_code_domain::cell_type::ORIGIN,
        payload: repo_graph_core::CellPayload::Json(
            r#"{"provenance":"nav_route"}"#.to_string(),
        ),
    }
}

fn svc_repo(n: u8) -> RepoId {
    RepoId::from_canonical(&format!("test://resolver/svc{n}"))
}

/// One server repo holding legacy-shape (`<METHOD> <path>`) ROUTE nodes.
fn route_repo(repo: RepoId, routes: &[(&str, &str, Confidence)]) -> RepoGraph {
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    for (method, path, conf) in routes {
        let qname = format!("{method} {path}");
        let (mut node, id) = make_node(repo, node_kind::ROUTE, &qname, *conf);
        node.cells.push(route_method_cell(method));
        record(&mut nav, id, &qname, &qname, node_kind::ROUTE);
        nodes.push(node);
    }
    make_graph(repo, nodes, nav)
}

/// One client repo holding ENDPOINT nodes.
fn endpoint_repo(repo: RepoId, endpoints: &[(&str, &str, Confidence)]) -> RepoGraph {
    let mut nav = CodeNav::default();
    let mut nodes = Vec::new();
    for (method, path, conf) in endpoints {
        let qname = format!("endpoint:{method}:{path}");
        let (node, id) = make_node(repo, node_kind::ENDPOINT, &qname, *conf);
        record(&mut nav, id, &qname, &qname, node_kind::ENDPOINT);
        nodes.push(node);
    }
    make_graph(repo, nodes, nav)
}

fn http_edges(merged: &MergedGraph) -> Vec<repo_graph_core::Edge> {
    merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .copied()
        .collect()
}

fn resolved(server: RepoGraph, client: RepoGraph) -> MergedGraph {
    let mut merged = MergedGraph::new(vec![server, client]);
    HttpStackResolver.resolve(&mut merged);
    merged
}

fn node_id(repo: RepoId, kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo, kind, qname)
}

#[test]
fn http_any_route_matches_every_verb() {
    // Rails `resources :posts`, Spring class-level `@RequestMapping`, Laravel
    // `Route::any`, Go `HandleFunc`, Clojure `ANY`: nine parsers emit the method
    // string "ANY" for a method-agnostic server route. Keyed on (METHOD, path),
    // every one of those declarations was an unreachable HTTP_CALLS target.
    let server = route_repo(repo_a(), &[("ANY", "/posts", Confidence::Strong)]);
    let client = endpoint_repo(
        repo_b(),
        &[
            ("GET", "/posts", Confidence::Strong),
            ("POST", "/posts", Confidence::Strong),
        ],
    );
    let merged = resolved(server, client);
    let edges = http_edges(&merged);

    let route = node_id(repo_a(), node_kind::ROUTE, "ANY /posts");
    assert_eq!(edges.len(), 2, "both verbs must reach the ANY route: {edges:?}");
    for verb in ["GET", "POST"] {
        let ep = node_id(
            repo_b(),
            node_kind::ENDPOINT,
            &format!("endpoint:{verb}:/posts"),
        );
        let e = edges
            .iter()
            .find(|e| e.from == ep && e.to == route)
            .unwrap_or_else(|| panic!("{verb} endpoint → ANY route edge"));
        // The server DECLARED the route method-agnostic, so this is a
        // principled pairing, not a guess: no confidence floor.
        assert_eq!(e.confidence, Confidence::Strong);
    }
}

#[test]
fn http_exact_and_endpoint_strip_still_win_over_any() {
    // TIER ORDER REGRESSION. With the ANY tier placed second, `GET /api/users`
    // would strip to `/users`, find the ANY route first and silently retarget an
    // edge that binds the TYPED route today. Exact → endpoint-strip → ANY keeps
    // every pre-A3.1 pairing exactly where it was, which is what makes this
    // packet additive.
    let server = route_repo(
        repo_a(),
        &[
            ("GET", "/users", Confidence::Strong),
            ("ANY", "/users", Confidence::Strong),
        ],
    );
    let client = endpoint_repo(repo_b(), &[("GET", "/api/users", Confidence::Strong)]);
    let merged = resolved(server, client);
    let edges = http_edges(&merged);

    assert_eq!(edges.len(), 1, "one tier only, never a union: {edges:?}");
    assert_eq!(
        edges[0].to,
        node_id(repo_a(), node_kind::ROUTE, "GET /users"),
        "the endpoint-side strip must still bind the typed route, not the ANY one"
    );
    assert_eq!(edges[0].confidence, Confidence::Strong);
}

#[test]
fn http_route_side_prefix_strip() {
    // The strip used to be endpoint-side ONLY, so a client calling `/orders`
    // could never reach a server mounted at `/api/orders` — the single most
    // common cross-service shape in a polyglot repo.
    let server = route_repo(repo_a(), &[("GET", "/api/orders", Confidence::Strong)]);
    let client = endpoint_repo(repo_b(), &[("GET", "/orders", Confidence::Strong)]);
    let merged = resolved(server, client);
    let edges = http_edges(&merged);

    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0].to, node_id(repo_a(), node_kind::ROUTE, "GET /api/orders"));
    assert_eq!(
        edges[0].confidence,
        Confidence::Medium,
        "a route-side strip is an inference — floored to Medium so a consumer \
         can tell it from an exact pairing"
    );
}

#[test]
fn http_base_url_fold() {
    // `fetch(`${environment.apiUrl}/users`)` normalises to `/{}/users`. The
    // leading segment came from an interpolation, so it is a base URL, not a
    // resource — fold it away before matching.
    let server = route_repo(repo_a(), &[("GET", "/users", Confidence::Strong)]);
    let client = endpoint_repo(repo_b(), &[("GET", "${…}/users", Confidence::Strong)]);
    let merged = resolved(server, client);
    let edges = http_edges(&merged);

    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(edges[0].to, node_id(repo_a(), node_kind::ROUTE, "GET /users"));
    assert_eq!(edges[0].confidence, Confidence::Medium);
}

#[test]
fn http_suffix_fallback_is_weak_and_bails_on_ambiguity() {
    // Positive: one base-URL client, one deep-prefixed server, one candidate.
    let server = route_repo(repo_a(), &[("GET", "/users", Confidence::Strong)]);
    let client = endpoint_repo(
        repo_b(),
        &[("GET", "${…}/tenant/users", Confidence::Strong)],
    );
    let merged = resolved(server, client);
    let edges = http_edges(&merged);
    assert_eq!(edges.len(), 1, "{edges:?}");
    assert_eq!(
        edges[0].confidence,
        Confidence::Weak,
        "the suffix tier is the weakest thing this resolver will emit"
    );

    // Ambiguous: FOUR services expose `/users`. Emitting four edges off a
    // suffix guess is exactly the multi-service fan-out this tier must not
    // cause, so above MAX_SUFFIX_TARGETS it emits nothing at all.
    let mut graphs: Vec<RepoGraph> = (1..=4)
        .map(|i| route_repo(svc_repo(i), &[("GET", "/users", Confidence::Strong)]))
        .collect();
    graphs.push(endpoint_repo(
        repo_b(),
        &[("GET", "${…}/tenant/users", Confidence::Strong)],
    ));
    let mut merged = MergedGraph::new(graphs);
    HttpStackResolver.resolve(&mut merged);
    assert!(
        http_edges(&merged).is_empty(),
        "an ambiguous suffix must emit NOTHING rather than fan out: {:?}",
        http_edges(&merged)
    );
}

#[test]
fn http_ordinary_path_never_suffix_matches() {
    // The base-fold and suffix tiers are gated on a LEADING `{}` segment. An
    // ordinary literal path must never reach them, or every `/tenant/users` in
    // a repo would bind every `/users` route in it.
    let server = route_repo(repo_a(), &[("GET", "/users", Confidence::Strong)]);
    let client = endpoint_repo(repo_b(), &[("GET", "/tenant/users", Confidence::Strong)]);
    let merged = resolved(server, client);
    assert!(
        http_edges(&merged).is_empty(),
        "no leading interpolation => no fuzzy tier: {:?}",
        http_edges(&merged)
    );
}

#[test]
fn http_any_nav_route_is_still_excluded() {
    // A3.1 x A3.4. go_router / react-router / Angular Router mint their
    // navigation entries with the method string "ANY", so the moment the ANY
    // tier went live every nav entry in an SPA would have become an HTTP_CALLS
    // target. A3.4's `provenance: nav_route` marking is what stops that, and it
    // has to keep working now that "ANY" is pairable.
    let mut nav = CodeNav::default();
    let (mut route, route_qid) =
        make_node(repo_a(), node_kind::ROUTE, "ANY /users", Confidence::Medium);
    route.cells.push(route_method_cell("ANY"));
    route.cells.push(nav_route_cell());
    record(&mut nav, route_qid, "ANY /users", "ANY /users", node_kind::ROUTE);
    let server = make_graph(repo_a(), vec![route], nav);

    let client = endpoint_repo(repo_b(), &[("GET", "/users", Confidence::Strong)]);
    let merged = resolved(server, client);
    assert!(
        http_edges(&merged).is_empty(),
        "a nav ROUTE must not become pairable just because ANY now is: {:?}",
        http_edges(&merged)
    );
    assert!(
        merged.graphs[0].nodes.iter().any(|n| n.id == route_qid),
        "the nav ROUTE node itself must survive"
    );
}
