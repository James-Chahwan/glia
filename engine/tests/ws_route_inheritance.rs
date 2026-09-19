//! LA.18b acceptance (programme A5.9, resolver half): a generic WebSocket
//! upgrade handler (`ws:ws`, the extractor's fallback name) pairs only through
//! the paths of the routes that reach its upgrading function, and a templated
//! handler path pairs one segment per parameter — on REAL builds of the
//! committed substrate-gap fixtures.
//!
//! `grade.py` reads the installed wheel, so it cannot see this change until
//! the end-of-wave rebuild; these tests grade the working tree directly.
//! Run with `-- --nocapture` to see the fired_on line:
//!   `[ws-resolve] 2 pairs (exact=0 suffix=0 param=0 inherited=2) dropped-generic=1`

use glia_code_domain::{edge_category, node_kind};
use glia_core::{NodeId, NodeKindId};
use glia_engine::generate_many;
use glia_graph::MergedGraph;

fn build(fixture: &str) -> MergedGraph {
    let base = format!(
        "{}/../bench/substrate-gap/fixtures/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let paths = vec![format!("{base}/client"), format!("{base}/server")];
    generate_many(&paths).expect("fixture builds").merged
}

fn id_of(m: &MergedGraph, kind: NodeKindId, qname: &str) -> NodeId {
    m.graphs
        .iter()
        .find_map(|g| {
            g.nav
                .qname_by_id
                .iter()
                .find(|(id, q)| q.as_str() == qname && g.nav.kind_by_id.get(id) == Some(&kind))
                .map(|(id, _)| *id)
        })
        .unwrap_or_else(|| panic!("no {qname}"))
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

#[test]
fn gorilla_and_nhooyr_inherit_their_route_paths() {
    // gorilla: `/ws` is a func-literal route in main.go calling serveWs
    // (LA.18d HANDLED_BY), the Upgrade is in client.go. nhooyr: `/echo` is
    // HANDLED_BY echo, which holds websocket.Accept(.
    let m = build("xcut-websocket-go-route");
    assert!(connects(&m, "ws_client:/echo", "ws:ws"));
    assert!(connects(&m, "ws_client:/ws", "ws:ws"));
    assert_eq!(connects_any(&m, "ws_client:/admin/live"), 0);
}

#[test]
fn param_segments_pair_one_segment() {
    let m = build("xcut-websocket-param-path");
    assert!(connects(&m, "ws_client:/chat/lobby", "ws:/chat/{room}"));
    assert_eq!(connects_any(&m, "ws_client:/chat/lobby/feed"), 0);
}

#[test]
fn generic_handler_pairs_only_through_its_route() {
    // The only route reaching the upgrade is /live. The /ws client paired by
    // name coincidence before LA.18b (`/ws` vs the fallback `ws`).
    let m = build("xcut-websocket-generic-route");
    assert!(connects(&m, "ws_client:/live", "ws:ws"));
    assert_eq!(connects_any(&m, "ws_client:/ws"), 0);
}

#[test]
fn ws_connects_control_now_pairs_by_inheritance() {
    // The committed control keeps its /ws pair, now through
    // `route:/ws -HANDLED_BY-> hub::ServeWs`; its precision twin (no route
    // reaches /notifications) still never pairs.
    let m = build("xcut-websocket-ws_connects");
    assert!(connects(&m, "ws_client:/ws", "ws:ws"));
    let m = build("xcut-websocket-precision");
    assert_eq!(connects_any(&m, "ws_client:/notifications"), 0);
}
