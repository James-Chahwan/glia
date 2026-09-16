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
