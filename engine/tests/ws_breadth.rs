//! LA.18a acceptance (programme A5.9): the WebSocket extractor reads every
//! occurrence of every gated needle, reads real server paths for Python /
//! Java / C# (and nhooyr), and reads client paths out of concatenations and
//! template literals, on REAL builds of the committed substrate-gap fixtures.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild; these tests grade the working tree directly.
//! Run with `-- --nocapture` to see the `[ws] handlers|clients framework=...`
//! fired_on lines.

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::{generate_many, generate_one};
use repo_graph_graph::MergedGraph;

fn bench(rel: &str) -> String {
    format!(
        "{}/../bench/substrate-gap/{rel}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn build(base: &str) -> MergedGraph {
    let paths = vec![
        bench(&format!("{base}/client")),
        bench(&format!("{base}/server")),
    ];
    generate_many(&paths).expect("fixture builds").merged
}

/// Qnames of every node of `kind`, sorted.
fn qnames(m: &MergedGraph, kind: NodeKindId) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .filter(move |(id, _)| g.nav.kind_by_id.get(id) == Some(&kind))
                .map(|(_, q)| q.clone())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn id_of(m: &MergedGraph, kind: NodeKindId, qname: &str) -> Option<NodeId> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .iter()
            .find(|(id, q)| q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind))
            .map(|(id, _)| *id)
    })
}

fn has_edge(m: &MergedGraph, from: NodeId, to: NodeId, cat: EdgeCategoryId) -> bool {
    m.all_edges()
        .any(|e| e.from == from && e.to == to && e.category == cat)
}

fn connects(m: &MergedGraph, client: &str, handler: &str) -> bool {
    let (Some(c), Some(h)) = (
        id_of(m, node_kind::WS_CLIENT, client),
        id_of(m, node_kind::WS_HANDLER, handler),
    ) else {
        return false;
    };
    m.all_edges()
        .any(|e| e.from == c && e.to == h && e.category == edge_category::WS_CONNECTS)
}

/// No WS qname is ever multi-line or spaced (the dropped `gorilla/websocket`
/// import needle once minted `ws:\n)\n\nvar upgrader = ...`).
fn assert_single_token_names(m: &MergedGraph, context: &str) {
    for kind in [node_kind::WS_HANDLER, node_kind::WS_CLIENT] {
        for q in qnames(m, kind) {
            assert!(
                !q.chars().any(|c| c.is_whitespace() || c.is_control()),
                "{context}: WS qname {q:?} is not a single token"
            );
        }
    }
}

#[test]
fn ws_breadth_probes_pair() {
    // Python / FastAPI: both decorators are read, and the chat client pairs
    // with its own endpoint only.
    let m = build("matrix/python/ws");
    let handlers = qnames(&m, node_kind::WS_HANDLER);
    assert!(
        handlers.contains(&"ws:/ws/chat".to_string()),
        "{handlers:?}"
    );
    assert!(
        handlers.contains(&"ws:/ws/admin".to_string()),
        "{handlers:?}"
    );
    assert!(connects(&m, "ws_client:/ws/chat", "ws:/ws/chat"));
    assert!(!connects(&m, "ws_client:/ws/chat", "ws:/ws/admin"));
    assert!(!connects(&m, "ws_client:/ws/chat", "ws:ws"));
    // Each decorator anchors at the `async def` below it, so each endpoint is
    // HANDLED_BY its own function (a Python FUNCTION span starts at `def`).
    for (handler, func) in [("ws:/ws/chat", "app::chat"), ("ws:/ws/admin", "app::admin")] {
        let h = id_of(&m, node_kind::WS_HANDLER, handler).expect("handler");
        let f = id_of(&m, node_kind::FUNCTION, func).expect("function");
        assert!(
            has_edge(&m, h, f, edge_category::HANDLED_BY),
            "{handler} HANDLED_BY {func}"
        );
    }
    assert_single_token_names(&m, "python/ws");

    // Java: JSR-356 @ServerEndpoint and Spring addHandler (path in arg #1);
    // the second, template-literal client is read too.
    let m = build("matrix/java/ws");
    let handlers = qnames(&m, node_kind::WS_HANDLER);
    assert!(
        handlers.contains(&"ws:/notifications".to_string()),
        "{handlers:?}"
    );
    assert!(handlers.contains(&"ws:/echo".to_string()), "{handlers:?}");
    assert!(!handlers.contains(&"ws:*".to_string()), "{handlers:?}");
    assert!(connects(&m, "ws_client:/echo", "ws:/echo"));
    assert!(connects(
        &m,
        "ws_client:/notifications",
        "ws:/notifications"
    ));
    assert!(!connects(&m, "ws_client:/echo", "ws:/notifications"));
    assert_single_token_names(&m, "java/ws");

    // C# SignalR: MapHub<T>(path) and the browser HubConnectionBuilder.
    let m = build("matrix/csharp/ws");
    let handlers = qnames(&m, node_kind::WS_HANDLER);
    assert_eq!(handlers, vec!["ws:/hubs/chat".to_string()]);
    assert_eq!(
        qnames(&m, node_kind::WS_CLIENT),
        vec!["ws_client:/hubs/chat".to_string()]
    );
    assert!(connects(&m, "ws_client:/hubs/chat", "ws:/hubs/chat"));
    assert_single_token_names(&m, "csharp/ws");

    // Browser client URL forms: static tail of a concatenation, template
    // literal after a dynamic head, a bare variable (unreadable).
    let m = build("fixtures/xcut-websocket-client-forms");
    let clients = qnames(&m, node_kind::WS_CLIENT);
    assert!(
        clients.contains(&"ws_client:/ws/chat".to_string()),
        "{clients:?}"
    );
    assert!(
        clients.contains(&"ws_client:/ws/admin".to_string()),
        "{clients:?}"
    );
    assert!(
        !clients.contains(&"ws_client:ws://".to_string()),
        "{clients:?}"
    );
    assert!(
        !clients.contains(&"ws_client:url".to_string()),
        "{clients:?}"
    );
    assert!(connects(&m, "ws_client:/ws/chat", "ws:/ws/chat"));
    assert!(connects(&m, "ws_client:/ws/admin", "ws:/ws/admin"));
    assert!(!connects(&m, "ws_client:/ws/chat", "ws:/ws/admin"));
    assert_single_token_names(&m, "xcut-websocket-client-forms");

    // Gorilla: one generic handler at the upgrade call, never a multi-line
    // name read off the import line.
    let m = build("fixtures/xcut-websocket-ws_connects");
    assert_eq!(qnames(&m, node_kind::WS_HANDLER), vec!["ws:ws".to_string()]);
    assert!(connects(&m, "ws_client:/ws", "ws:ws"));
    let handler = id_of(&m, node_kind::WS_HANDLER, "ws:ws").expect("handler");
    let serve = id_of(&m, node_kind::FUNCTION, "hub::ServeWs").expect("ServeWs");
    assert!(has_edge(&m, handler, serve, edge_category::HANDLED_BY));
    assert!(!has_edge(&m, serve, handler, edge_category::CONTAINS));
    assert_single_token_names(&m, "xcut-websocket-ws_connects");
    // Precision twin: the generic handler still never pairs with a real path.
    let m = build("fixtures/xcut-websocket-precision");
    assert_eq!(qnames(&m, node_kind::WS_HANDLER), vec!["ws:ws".to_string()]);
    assert!(id_of(&m, node_kind::WS_CLIENT, "ws_client:/notifications").is_some());
    assert!(!connects(&m, "ws_client:/notifications", "ws:ws"));
    let r = generate_one(&bench("fixtures/arch-monorepo-flows")).expect("fixture builds");
    assert_eq!(
        qnames(&r.merged, node_kind::WS_HANDLER),
        vec!["ws:ws".to_string()]
    );
    assert_single_token_names(&r.merged, "arch-monorepo-flows");
}

#[test]
fn dart_dio_is_not_a_ws_client() {
    let r = generate_one(&bench("fixtures/http-absolute-url-client/client-dart"))
        .expect("fixture builds");
    assert_eq!(
        qnames(&r.merged, node_kind::WS_CLIENT),
        Vec::<String>::new(),
        "`Dio()` is not socket.io's `io(`"
    );
}
