//! A13.7 acceptance gate: a define-side env VALUE reaches a consumer.
//!
//! The unit tests in `parsers/code/extractors/src/config.rs` prove the scanners
//! parse the value. They do NOT prove it survives the three hops between the
//! scanner and area A11: `merge_parses` (which appends cells when a read site
//! and a define site produce the same `NodeId`), the rkyv/mmap store round
//! trip, and the cell lookup `PyGraph.node_cells` performs. This test walks all
//! three against a real on-disk repo, so the ENV-cell contract is gated in-tree
//! and does not wait on a rebuilt wheel.

use std::path::Path;

use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::CellPayload;
use repo_graph_engine::generate_one;
use repo_graph_store::{read_merged_sharded, write_merged_sharded};

fn write_fixture(dir: &Path) {
    // `.env.example` (not `.env`: glia's own repo gitignores that name).
    std::fs::write(
        dir.join(".env.example"),
        "# service wiring\n\
         API_URL=http://users-svc:8080\n\
         DATABASE_URL=postgres://appuser:s3cr3t@db.internal:5432/app\n\
         STRIPE_SECRET_KEY=sk_live_4eC39HqLyjWDarjtT1zdp7dc\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("Dockerfile"),
        "FROM python:3.11-slim\nENV QUEUE_URL=amqp://rabbit:5672/ LOG_LEVEL=debug\n",
    )
    .unwrap();
    // Read site for a key that is ALSO defined: both graphs emit the same
    // NodeId, so this exercises the cell-append path in merge_parses.
    std::fs::write(
        dir.join("app.py"),
        "import os\n\nurl = os.environ['API_URL']\n",
    )
    .unwrap();
}

/// Every ENV-cell payload in the graph for the CONFIG_KEY named `qname`.
/// Mirrors what `PyGraph.node_cells` does, except that it scans EVERY graph
/// rather than stopping at the first one holding that id.
fn env_payloads(merged: &repo_graph_graph::MergedGraph, qname: &str) -> Vec<String> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CONFIG_KEY)
                || g.nav.qname_by_id.get(&n.id).map(String::as_str) != Some(qname)
            {
                continue;
            }
            for c in &n.cells {
                if c.kind == cell_type::ENV
                    && let CellPayload::Text(s) | CellPayload::Json(s) = &c.payload
                {
                    out.push(s.clone());
                }
            }
        }
    }
    out
}

#[test]
fn env_values_survive_build_and_store_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_fixture(&repo);

    let built = generate_one(repo.to_str().unwrap()).unwrap().merged;
    let out = tmp.path().join("gmap");
    write_merged_sharded(&built, &out).unwrap();
    let loaded = read_merged_sharded(&out).unwrap();

    for (label, merged) in [("built", &built), ("round-tripped", &loaded)] {
        // Plain value, define side, read ALSO present for this key.
        let api = env_payloads(merged, "config:env:API_URL");
        assert_eq!(api.len(), 1, "{label}: API_URL env cells: {api:?}");
        assert!(
            api[0].contains(r#""value":"http://users-svc:8080""#),
            "{label}: {}",
            api[0]
        );
        assert!(api[0].contains(r#""source":"dotenv""#), "{label}: {}", api[0]);

        // Userinfo masked, HOST kept — A11 pairs on the host.
        let db = env_payloads(merged, "config:env:DATABASE_URL");
        assert_eq!(db.len(), 1, "{label}: DATABASE_URL env cells: {db:?}");
        assert!(
            db[0].contains(r#""value":"postgres://***@db.internal:5432/app""#),
            "{label}: {}",
            db[0]
        );

        // Second pair of a multi-pair Dockerfile ENV.
        let log = env_payloads(merged, "config:env:LOG_LEVEL");
        assert_eq!(log.len(), 1, "{label}: LOG_LEVEL env cells: {log:?}");
        assert!(log[0].contains(r#""source":"dockerfile""#), "{label}: {}", log[0]);

        // Secret-named key: the cell says a value exists and nothing else.
        let secret = env_payloads(merged, "config:env:STRIPE_SECRET_KEY");
        assert_eq!(
            secret,
            vec![r#"{"source":"dotenv","redacted":true}"#.to_string()],
            "{label}"
        );

        // Nothing anywhere in the graph carries the secret.
        let all: String = merged
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter())
            .flat_map(|n| n.cells.iter())
            .filter_map(|c| match &c.payload {
                CellPayload::Text(s) | CellPayload::Json(s) => Some(s.as_str()),
                CellPayload::Bytes(_) => None,
            })
            .collect();
        assert!(!all.contains("sk_live_"), "{label}: secret leaked into a cell");
        assert!(!all.contains("s3cr3t"), "{label}: db password leaked into a cell");
    }
}
