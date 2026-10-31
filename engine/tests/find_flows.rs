//! GB.2 — `find::find_flows` / `find::flows_through`: the entry flows that
//! pass through find's hits, each with the hit's depth in the flow (the node
//! side of GB.1's concept search), on GB.1's fixtures (`concept_flows::shop`,
//! `concept_flows::cached`).
//!
//! The parity test reads `trace::entry_flows` directly and finds, per hit,
//! the flows whose entry is the hit (depth 0) or whose hops show it (the
//! shallowest such hop's depth): `flows_through` must give exactly those, in
//! (depth, reach desc, key, entry qname) order. Before GB.2, `glia find`
//! had no `--flows` (`error: unexpected argument '--flows' found`, exit 2).

use std::collections::BTreeMap;

use glia_engine::find::{
    DEFAULT_FLOWS_PER_HIT, FindFlows, FindOptions, FlowsThrough, find_flows, flows_through,
};
use glia_engine::trace::{DEFAULT_DEPTH, EntryFlow, entry_flows};
use glia_engine::{GenerateResult, entrypoint_reachable};

use crate::concept_flows::{cached, shop};

fn ask(r: &GenerateResult, query: &str) -> FindFlows {
    find_flows(&r.merged, query, &FindOptions::default(), DEFAULT_DEPTH)
}

fn through<'a>(f: &'a FindFlows, qname: &str) -> &'a FlowsThrough {
    f.flows
        .iter()
        .find(|t| t.qname == qname)
        .unwrap_or_else(|| panic!("no flows entry for {qname}: {:?}", f.flows))
}

/// `(key, depth)` per flow, in answer order.
fn keyed(t: &FlowsThrough) -> Vec<(String, usize)> {
    t.flows.iter().map(|f| (f.key.clone(), f.depth)).collect()
}

#[test]
fn a_hit_lists_the_flows_through_it_with_its_depth() {
    let (_td, r) = shop();
    let f = ask(&r, "place_order");
    assert_eq!(f.answer.results[0].qname, "app::place_order");
    assert_eq!(f.flows.len(), f.answer.results.len(), "one entry per row");
    let t = through(&f, "app::place_order");
    assert_eq!(t.kind, "FUNCTION");
    assert_eq!(t.id, f.answer.results[0].id);
    // POST /orders -HANDLED_BY-> create_order -CALLS-> place_order.
    assert_eq!(keyed(t), vec![("post_/orders".to_string(), 2)]);
    assert_eq!((t.total, t.more), (1, 0));
    let flow = &t.flows[0];
    assert_eq!(flow.rank, 1);
    assert_eq!(flow.entry.qname, "POST /orders");
    assert_eq!(flow.entry.kind, "ROUTE");
    assert_eq!(flow.reach, 5);
    assert!(flow.mechanisms.contains(&"CALLS"), "{:?}", flow.mechanisms);
}

#[test]
fn an_entry_hit_is_depth_zero() {
    let (_td, r) = shop();
    let f = ask(&r, "POST /orders");
    let t = through(&f, "POST /orders");
    assert_eq!(keyed(t)[0], ("post_/orders".to_string(), 0));
}

#[test]
fn a_data_entity_hit_is_reached_at_its_hop_depth() {
    let (_td, r) = shop();
    let f = ask(&r, "issue_comments");
    let t = through(&f, "data_entity:sql:issue_comments");
    assert_eq!(t.kind, "DATA_ENTITY");
    assert_eq!(
        keyed(t),
        vec![("get_/orders/<oid>/comments".to_string(), 3)]
    );
}

#[test]
fn a_hit_no_flow_reaches_has_an_empty_entry() {
    let (_td, r) = shop();
    let f = ask(&r, "Caching");
    assert!(!f.answer.results.is_empty(), "the doc section is found");
    for t in &f.flows {
        if t.kind == "DOC_SECTION" {
            assert_eq!((t.total, t.flows.len(), t.more), (0, 0, 0), "{t:?}");
        }
    }
}

#[test]
fn a_cache_hop_three_deep_in_the_hand_built_graph() {
    let m = cached();
    let f = find_flows(&m, "redis", &FindOptions::default(), DEFAULT_DEPTH);
    let t = f
        .flows
        .iter()
        .find(|t| t.qname == "data_source:redis")
        .expect("redis is found");
    assert_eq!(keyed(t), vec![("post_/orders".to_string(), 3)]);
    // Depth 2 stops the flow one hop short of the cache.
    let short = find_flows(&m, "redis", &FindOptions::default(), 2);
    assert_eq!(short.flows[0].total, 0, "{:?}", short.flows);
}

/// Per hit, the flows a direct read of `entry_flows` says pass through it.
fn expected(qname: &str, flows: &[EntryFlow]) -> Vec<(String, usize, usize)> {
    let mut rows: Vec<(usize, usize, String, String)> = flows
        .iter()
        .filter_map(|f| {
            let depth = if f.entry.qname == qname {
                Some(0)
            } else {
                f.hops
                    .iter()
                    .filter(|h| h.to_qname == qname)
                    .map(|h| h.depth)
                    .min()
            }?;
            Some((depth, f.reach, f.key.clone(), f.entry.qname.clone()))
        })
        .collect();
    rows.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    rows.into_iter().map(|(d, r, k, _)| (k, d, r)).collect()
}

#[test]
fn flows_through_equals_a_scan_of_entry_flows() {
    let (_td, r) = shop();
    let flows = entry_flows(&r.merged, &r.repo_labels, DEFAULT_DEPTH);
    let mut checked = 0;
    for q in ["orders", "order", "notify", "comments", "health", "app"] {
        let f = ask(&r, q);
        assert_eq!(f.flows.len(), f.answer.results.len(), "{q}");
        for (row, t) in f.answer.results.iter().zip(&f.flows) {
            assert_eq!((t.id, t.qname.as_str()), (row.id, row.qname.as_str()));
            let want = expected(&row.qname, &flows);
            let got: Vec<(String, usize, usize)> = t
                .flows
                .iter()
                .map(|x| (x.key.clone(), x.depth, x.reach))
                .collect();
            assert_eq!(got, want, "query {q}, hit {}", row.qname);
            assert_eq!(t.total, want.len());
            checked += usize::from(t.total > 0);
        }
    }
    assert!(checked >= 6, "the scan compared {checked} hits with flows");
}

#[test]
fn the_cap_keeps_the_first_flows_and_counts_the_rest() {
    let (_td, r) = shop();
    let hits = ask(&r, "place_order").answer.results;
    let flows = entry_flows(&r.merged, &r.repo_labels, DEFAULT_DEPTH);
    // Three copies of every flow: the hit is in three flows.
    let tripled: Vec<EntryFlow> = flows.iter().chain(&flows).chain(&flows).cloned().collect();
    let all = flows_through(&hits, &tripled, 0);
    assert_eq!((all[0].total, all[0].flows.len(), all[0].more), (3, 3, 0));
    let cut = flows_through(&hits, &tripled, 2);
    assert_eq!((cut[0].total, cut[0].flows.len(), cut[0].more), (3, 2, 1));
    let ranks: Vec<usize> = cut[0].flows.iter().map(|f| f.rank).collect();
    assert_eq!(ranks, vec![1, 2]);
    assert_eq!(DEFAULT_FLOWS_PER_HIT, 20);
}

#[test]
fn no_match_is_find_s_absence_with_no_flows() {
    let (_td, r) = shop();
    let f = ask(&r, "zzqqxx");
    assert!(f.answer.results.is_empty());
    assert!(f.flows.is_empty());
    let a = f.answer.absence.as_ref().expect("absence");
    assert_eq!(a.reason, "no_match");
}

#[test]
fn the_json_is_find_s_envelope_plus_flows() {
    let (_td, r) = shop();
    let f = ask(&r, "place_order");
    // Key order is checked on the text: a `serde_json::Value` sorts keys.
    let text = serde_json::to_string(&f).expect("json");
    assert!(text.starts_with("{\"results\":[{"), "{text}");
    let absence = text.find(",\"absence\":null,").expect("absence key");
    let flows = text.find(",\"flows\":[{\"id\":").expect("flows key");
    assert!(absence < flows, "{text}");
    let entry = format!(
        "\"flows\":[{{\"id\":{},\"qname\":\"app::place_order\",\"kind\":\"FUNCTION\",\"total\":1,\"flows\":[{{\"rank\":1,\"key\":\"post_/orders\",\"entry\":{{",
        f.answer.results[0].id
    );
    assert!(text.contains(&entry), "{text}");
    assert!(
        text.contains("},\"depth\":2,\"reach\":5,\"cross_service\":false,\"mechanisms\":["),
        "{text}"
    );
    assert!(text.ends_with("]}],\"more\":0}]}"), "{text}");
    // find's own rows are untouched by the flows.
    let plain = glia_engine::find::find_nodes(&r.merged, "place_order", &FindOptions::default());
    let v = serde_json::to_value(&f).expect("json");
    assert_eq!(
        serde_json::to_value(&plain).expect("json")["results"],
        v["results"]
    );
}

#[test]
fn the_answer_is_deterministic() {
    let (_td, r) = shop();
    let live = entrypoint_reachable(&r.merged);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..3 {
        let f = glia_engine::find::find_flows_with_live(
            &r.merged,
            &live,
            "o",
            &FindOptions::default(),
            DEFAULT_DEPTH,
            "lib",
        );
        *seen
            .entry(serde_json::to_string(&f).expect("json"))
            .or_default() += 1;
    }
    assert_eq!(seen.len(), 1, "three runs, one answer");
}
