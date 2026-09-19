//! LA.18c acceptance (programme A5.9, channel-keyed frameworks): ActionCable
//! channel classes and Phoenix socket mounts / channel topics are named by
//! their join key (`ws:ChatChannel`, `ws:/socket`, `ws:room:*`), their JS
//! clients are extracted (`ws_client:ChatChannel`, `ws_client:/socket`,
//! `ws_client:room:lobby`), and the resolver pairs a Phoenix trailing-`*`
//! topic with the concrete topic a client joins — on REAL builds of the
//! committed matrix probes `matrix/ruby/ws` and `matrix/elixir/ws`.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild; these tests grade the working tree directly.
//! Run with `-- --nocapture` to see the fired_on lines:
//!   `[ws] handlers framework=phoenix n=1 in lib/app_web/channels/user_socket.ex`
//!   `[ws-resolve] 2 pairs (exact=1 suffix=0 param=0 inherited=0 wildcard=1) dropped-generic=0`

use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{NodeId, NodeKindId};
use repo_graph_engine::generate_many;
use repo_graph_graph::MergedGraph;

fn build(cell: &str) -> MergedGraph {
    let base = format!(
        "{}/../bench/substrate-gap/matrix/{cell}",
        env!("CARGO_MANIFEST_DIR")
    );
    let paths = vec![format!("{base}/client"), format!("{base}/server")];
    generate_many(&paths).expect("fixture builds").merged
}

fn find(m: &MergedGraph, kind: NodeKindId, qname: &str) -> Option<NodeId> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .iter()
            .find(|(id, q)| q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind))
            .map(|(id, _)| *id)
    })
}

fn id_of(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    find(m, kind, qname).unwrap_or_else(|| panic!("no {qname}"))
}

fn connects(m: &MergedGraph, client: &str, handler: &str) -> bool {
    let (c, h) = (
        id_of(m, node_kind::WS_CLIENT, client),
        id_of(m, node_kind::WS_HANDLER, handler),
    );
    m.all_edges()
        .any(|e| e.from == c && e.to == h && e.category == edge_category::WS_CONNECTS)
}

/// Every WS_CONNECTS edge leaving `client`.
fn connects_any(m: &MergedGraph, client: &str) -> usize {
    let c = id_of(m, node_kind::WS_CLIENT, client);
    m.all_edges()
        .filter(|e| e.from == c && e.category == edge_category::WS_CONNECTS)
        .count()
}

/// Every WS_HANDLER / WS_CLIENT qname of the build, sorted.
fn ws_qnames(m: &MergedGraph) -> Vec<String> {
    let mut out: Vec<String> = m
        .graphs
        .iter()
        .flat_map(|g| {
            g.nav.qname_by_id.iter().filter_map(|(id, q)| {
                matches!(
                    g.nav.kind_by_id.get(id),
                    Some(&k) if k == node_kind::WS_HANDLER || k == node_kind::WS_CLIENT
                )
                .then(|| q.clone())
            })
        })
        .collect();
    out.sort();
    out
}

#[test]
fn actioncable_channels_pair() {
    let m = build("ruby/ws");
    assert_eq!(
        ws_qnames(&m),
        vec![
            "ws:ChatChannel",
            "ws:PresenceChannel",
            "ws_client:ChatChannel"
        ],
        "channel classes and the consumer's channel key, nothing generic"
    );
    assert!(connects(&m, "ws_client:ChatChannel", "ws:ChatChannel"));
    assert!(!connects(&m, "ws_client:ChatChannel", "ws:PresenceChannel"));
    assert_eq!(connects_any(&m, "ws_client:ChatChannel"), 1);
    assert!(find(&m, node_kind::WS_HANDLER, "ws:default").is_none());
}

#[test]
fn phoenix_socket_and_topic_pair() {
    let m = build("elixir/ws");
    assert_eq!(
        ws_qnames(&m),
        vec![
            "ws:/socket",
            "ws:room:*",
            "ws_client:/socket",
            "ws_client:room:lobby"
        ],
        "endpoint mount, topic pattern, phoenix.js socket and joined topic; \
         `use Phoenix.Channel` mints nothing"
    );
    assert!(connects(&m, "ws_client:/socket", "ws:/socket"));
    assert!(connects(&m, "ws_client:room:lobby", "ws:room:*"));
    assert!(!connects(&m, "ws_client:room:lobby", "ws:/socket"));
    assert!(!connects(&m, "ws_client:/socket", "ws:room:*"));
    assert_eq!(connects_any(&m, "ws_client:/socket"), 1);
    assert_eq!(connects_any(&m, "ws_client:room:lobby"), 1);
    assert!(find(&m, node_kind::WS_HANDLER, "ws:default").is_none());
    assert!(find(&m, node_kind::WS_HANDLER, "ws:ws").is_none());
}
