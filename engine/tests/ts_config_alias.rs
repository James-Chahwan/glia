//! DF.20 — endpoint bases held in a class member or a module const assigned
//! from config fold to their real paths, over a REAL two-repo build.
//!
//! `bench/substrate-gap/fixtures/ts-config-alias-endpoints` (Kina reduced): an
//! Angular `web` whose services reach their base URL through a one-`return`
//! getter (`get api() { return environment.apiUrl; }`), a readonly template
//! field (`` `${environment.apiUrl}/notifications` ``), a readonly
//! `environment.apiUrl.replace('/api', '') + '/admin'` field and an exported
//! `const ADMIN_BASE` of the same expression, and a gin `server` with
//! `/api/*` and `/admin/*` groups plus a public `GET /stats`. Before DF.20
//! every web ENDPOINT kept a `${…}` base, paired only through the HTTP
//! resolver's base tier, and `${…}/stats` paired the PUBLIC `GET /stats`.
//! `bench/` is outside the cargo workspace's crates, so the fixture is copied
//! into a tempdir and built there.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId, NodeKindId};
use glia_engine::generate_many;
use glia_graph::MergedGraph;

fn fixture() -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/ts-config-alias-endpoints",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy the fixture's sources (never its key.json or a stray cache).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            if e.file_name() != ".ai" && e.file_name() != ".glia" {
                copy_tree(&src, &dst);
            }
        } else if e.file_name() != "key.json" {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn build() -> (tempfile::TempDir, MergedGraph) {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&fixture(), td.path());
    let repos: Vec<String> = ["web", "server"]
        .iter()
        .map(|d| td.path().join(d).to_string_lossy().into_owned())
        .collect();
    let m = generate_many(&repos).expect("ts-config-alias-endpoints builds").merged;
    (td, m)
}

/// A node's qname without its ` @<project>` owner segment.
fn qname(m: &MergedGraph, id: NodeId) -> Option<String> {
    m.graphs.iter().find_map(|g| {
        g.nav
            .qname_by_id
            .get(&id)
            .map(|q| q.split(" @").next().unwrap_or(q).to_string())
    })
}

fn qnames_of(m: &MergedGraph, kind: NodeKindId) -> BTreeSet<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.kind_by_id.get(&n.id) == Some(&kind))
                .filter_map(move |n| g.nav.qname_by_id.get(&n.id))
                .map(|q| q.split(" @").next().unwrap_or(q).to_string())
        })
        .collect()
}

/// Every HTTP_CALLS edge as `(endpoint qname, route qname)`.
fn http_calls(m: &MergedGraph) -> BTreeSet<(String, String)> {
    m.all_edges()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
        .filter_map(|e| Some((qname(m, e.from)?, qname(m, e.to)?)))
        .collect()
}

/// The ENDPOINT_HIT payloads of the ENDPOINT `endpoint:<method>:<path>`.
fn hits(m: &MergedGraph, endpoint: &str) -> Vec<serde_json::Value> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT)
                    && g.nav
                        .qname_by_id
                        .get(&n.id)
                        .is_some_and(|q| q.split(" @").next() == Some(endpoint))
            })
        })
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ENDPOINT_HIT)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) => serde_json::from_str(s).ok(),
            _ => None,
        })
        .collect()
}

#[test]
fn config_alias_bases_fold_and_pair_exactly() {
    let (_td, m) = build();
    let endpoints = qnames_of(&m, node_kind::ENDPOINT);
    let want = [
        "endpoint:GET:/api/trades",
        "endpoint:GET:/api/notifications/unread",
        "endpoint:GET:/admin/stats",
        "endpoint:GET:/admin/users",
    ];
    assert_eq!(
        endpoints,
        want.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        "every base folds; no `${{…}}` ENDPOINT is left"
    );
    let calls = http_calls(&m);
    let pairs: BTreeSet<(String, String)> = [
        ("endpoint:GET:/api/trades", "GET /api/trades"),
        ("endpoint:GET:/api/notifications/unread", "GET /api/notifications/unread"),
        ("endpoint:GET:/admin/stats", "GET /admin/stats"),
        ("endpoint:GET:/admin/users", "GET /admin/users"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(calls, pairs, "each endpoint pairs its own route, and only it");
    assert!(
        !calls.contains(&("endpoint:GET:/admin/stats".into(), "GET /stats".into())),
        "the admin stats call never pairs the public route"
    );
}

#[test]
fn folded_hits_name_the_member_alias_and_the_host() {
    let (_td, m) = build();
    for (endpoint, alias, template, folded_from) in [
        (
            "endpoint:GET:/api/trades",
            Some("this.api"),
            "${environment.apiUrl}/trades",
            "${…}/trades",
        ),
        (
            "endpoint:GET:/api/notifications/unread",
            Some("this.base"),
            "${environment.apiUrl}/notifications/unread",
            "${…}/unread",
        ),
        (
            "endpoint:GET:/admin/stats",
            Some("this.base"),
            "${environment.apiUrl.replace('/api', '')}/admin/stats",
            "${…}/stats",
        ),
        // The module const folds through the const table, not the parser.
        ("endpoint:GET:/admin/users", None, "${ADMIN_BASE}/users", "${…}/users"),
    ] {
        let h = hits(&m, endpoint);
        assert_eq!(h.len(), 1, "{endpoint}: {h:?}");
        let h = &h[0];
        assert_eq!(h["template"], template, "{endpoint}");
        assert_eq!(h["folded_from"], folded_from, "{endpoint}");
        assert_eq!(h["host"], "localhost:3701", "{endpoint}");
        assert_eq!(h.get("alias").and_then(|a| a.as_str()), alias, "{endpoint}");
    }
}
