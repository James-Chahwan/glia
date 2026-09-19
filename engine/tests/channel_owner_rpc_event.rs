//! LB.8b — the owner segment on the RPC and event sides, over REAL builds.
//!
//! RPC_PROCEDURE / RPC_CALL (tRPC, Connect, Twirp) and EVENT_EMITTER /
//! EVENT_HANDLER nodes whose file lies under a NESTED project root are
//! qualified with ` @<project path>`, like LB.4a's ROUTE / ENDPOINT and LB.8's
//! queue / ws / graphql / grpc sides. RPC is a network channel: every call
//! still pairs every same-path procedure. An IN-PROCESS event pairs only
//! inside one project or with an unowned side (a file under no nested
//! project); a TRANSPORT event (NestJS microservices, EventBridge) pairs
//! across projects.
//!
//! Before it, on `bench/substrate-gap/fixtures/rpc-event-monorepo-owner`
//! (seven manifest-rooted projects plus root-level `lib/`), both tRPC routers
//! were ONE `rpc:user.list`, both apps ONE `rpc_call:user.list`, and the two
//! services' private EventEmitters ONE `event_emit:orderPlaced` whose
//! EVENT_FLOWS reached the other service's handler. `bench/` is outside the
//! cargo workspace's crates, so the fixture is copied into a tempdir (the
//! `scope_filter.rs` way) and built there.
//!
//! The fired_on markers are read from a child process: the
//! `child_build_for_stderr` test re-runs this binary on one tree with
//! `--nocapture` and the parent reads its stderr.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, EdgeCategoryId, NodeId, NodeKindId};
use repo_graph_engine::{GenerateResult, generate_one, generate_one_incremental, service_map};
use repo_graph_graph::MergedGraph;

fn bench_fixture(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "{}/../bench/substrate-gap/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// Copy a fixture's sources (never its key.json or a stray `.ai/` cache).
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
    copy_tree(&bench_fixture("rpc-event-monorepo-owner"), td.path());
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

/// The ORIGIN payloads of the node named `q`.
fn origins(m: &MergedGraph, q: &str) -> Vec<String> {
    m.graphs
        .iter()
        .flat_map(|g| {
            g.nodes
                .iter()
                .filter(move |n| g.nav.qname_by_id.get(&n.id).is_some_and(|x| x == q))
        })
        .flat_map(|n| n.cells.iter())
        .filter(|c| c.kind == cell_type::ORIGIN)
        .map(|c| match &c.payload {
            CellPayload::Json(j) => j.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

const CHILD_ENV: &str = "GLIA_LB8B_CHILD_BUILD";

/// The child half of [`build_stderr`]: a no-op unless the parent set
/// [`CHILD_ENV`], in which case it builds that tree so its stderr (with
/// `--nocapture`) carries the build's markers.
#[test]
fn child_build_for_stderr() {
    if let Ok(dir) = std::env::var(CHILD_ENV) {
        build(Path::new(&dir));
    }
}

/// The stderr lines of one `generate_one` over `dir` that start with `prefix`.
fn build_stderr(dir: &Path, prefix: &str) -> Vec<String> {
    let exe = std::env::current_exe().expect("test binary path");
    let out = Command::new(exe)
        .args(["--exact", "child_build_for_stderr", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir)
        .output()
        .expect("re-run the test binary");
    assert!(out.status.success(), "child build failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.starts_with(prefix))
        .map(String::from)
        .collect()
}

#[test]
fn monorepo_rpc_and_event_sides_carry_their_owner() {
    let (_td, r) = monorepo();
    let m = &r.merged;
    assert_eq!(
        qnames_of(m, node_kind::RPC_PROCEDURE),
        ["rpc:user.list @services/legacy", "rpc:user.list @services/users"]
    );
    assert_eq!(
        qnames_of(m, node_kind::RPC_CALL),
        ["rpc_call:user.list @apps/admin", "rpc_call:user.list @apps/web"]
    );
    assert_eq!(
        qnames_of(m, node_kind::EVENT_EMITTER),
        [
            "event_emit:orderPlaced @services/billing",
            "event_emit:orderPlaced @services/orders",
            "event_emit:orderShipped @services/orders"
        ]
    );
    assert_eq!(
        qnames_of(m, node_kind::EVENT_HANDLER),
        [
            "event_handle:orderPlaced",
            "event_handle:orderPlaced @services/billing",
            "event_handle:orderPlaced @services/orders",
            "event_handle:orderShipped @services/notify"
        ],
        "lib/bus.ts sits under the root manifest only: its handler stays unowned"
    );

    // Each owned handler is HANDLED_BY its own service's function only: the
    // three-way HANDLED_BY the collapsed node carried is split.
    let handled: Vec<(String, String)> = edges(m, edge_category::HANDLED_BY)
        .into_iter()
        .filter(|(from, _)| from.starts_with("event_handle:orderPlaced"))
        .collect();
    assert_eq!(
        handled,
        [
            (s("event_handle:orderPlaced"), s("lib::bus::registerAudit")),
            (
                s("event_handle:orderPlaced @services/billing"),
                s("services::billing::events::registerBillingHandlers")
            ),
            (
                s("event_handle:orderPlaced @services/orders"),
                s("services::orders::events::registerOrderHandlers")
            ),
        ]
    );

    // The delivery scope rides the rekey: the transport sides carry the
    // extractor's ORIGIN, the in-process ones post_passes' plain synthetic one.
    let transport = r#"{"provenance":"synthetic","delivery":"transport","via":"nestjs-microservices"}"#;
    let synthetic = r#"{"provenance":"synthetic"}"#;
    for (q, want) in [
        ("event_emit:orderShipped @services/orders", transport),
        ("event_handle:orderShipped @services/notify", transport),
        ("event_emit:orderPlaced @services/orders", synthetic),
        ("event_handle:orderPlaced", synthetic),
    ] {
        assert_eq!(origins(m, q), [s(want)], "{q}");
    }

    // Display names are unchanged: the owner lives in the qname only.
    for g in &m.graphs {
        for (id, q) in &g.nav.qname_by_id {
            match q.as_str() {
                "rpc:user.list @services/users" => {
                    assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some("user.list"));
                }
                "event_emit:orderPlaced @services/orders" => {
                    assert_eq!(g.nav.name_by_id.get(id).map(String::as_str), Some("orderPlaced"));
                }
                _ => {}
            }
        }
    }
}

#[test]
fn rpc_pairs_across_owners_in_process_events_do_not() {
    let (_td, r) = monorepo();
    let m = &r.merged;
    let rpc = |c: &str, p: &str| (format!("rpc_call:user.list @apps/{c}"), format!("rpc:user.list @services/{p}"));
    assert_eq!(
        edges(m, edge_category::RPC_CALLS),
        [rpc("admin", "legacy"), rpc("admin", "users"), rpc("web", "legacy"), rpc("web", "users")],
        "a network RPC: every call pairs every same-path procedure"
    );
    assert_eq!(
        edges(m, edge_category::EVENT_FLOWS),
        [
            (s("event_emit:orderPlaced @services/billing"), s("event_handle:orderPlaced")),
            (
                s("event_emit:orderPlaced @services/billing"),
                s("event_handle:orderPlaced @services/billing")
            ),
            (s("event_emit:orderPlaced @services/orders"), s("event_handle:orderPlaced")),
            (
                s("event_emit:orderPlaced @services/orders"),
                s("event_handle:orderPlaced @services/orders")
            ),
            (
                s("event_emit:orderShipped @services/orders"),
                s("event_handle:orderShipped @services/notify")
            ),
        ],
        "same owner, unowned side and transport pair; orders <-> billing never does"
    );
}

/// The fired_on markers: the owner pass's second `[channel-owner]` line and
/// the resolver's `[eventbus-owner]` line. LB.8's own `[channel-owner]` line
/// does not print (no queue / ws / graphql / grpc side here).
#[test]
fn monorepo_markers_count_every_side_and_pair() {
    let td = tempfile::tempdir().unwrap();
    copy_tree(&bench_fixture("rpc-event-monorepo-owner"), td.path());
    let owner = build_stderr(td.path(), "[channel-owner]");
    assert_eq!(owner.len(), 1, "{owner:?}");
    assert!(
        owner[0].starts_with(
            "[channel-owner] qualified rpc=4 event=6 over 7 owners (declared=0 unplaced=0 foreign=0) repo="
        ),
        "{}",
        owner[0]
    );
    assert_eq!(
        build_stderr(td.path(), "[eventbus-owner]"),
        ["[eventbus-owner] same-owner=2 unowned-side=2 transport=1 cross-owner-dropped=2"]
    );
}

/// `glia arch`: each project is its own service; the in-process event stays
/// inside its project (a self link), the shared `lib/` handler reads as
/// `(outside projects)`, and nothing is unlocated.
#[test]
fn service_map_places_every_rpc_and_event_side() {
    let (_td, r) = monorepo();
    let map = service_map(&r.merged, &r.repo_labels);
    assert_eq!(map.keying, "project_roots");
    let links: BTreeSet<(String, String, &str, String)> = map
        .links
        .iter()
        .map(|l| (l.from.clone(), l.to.clone(), l.mechanism, l.channel.clone()))
        .collect();
    let mut want: BTreeSet<(String, String, &str, String)> = BTreeSet::new();
    for c in ["apps/admin", "apps/web"] {
        for p in ["services/legacy", "services/users"] {
            want.insert((s(c), s(p), "RPC_CALLS", s("user.list")));
        }
    }
    for e in ["services/billing", "services/orders"] {
        want.insert((s(e), s("(outside projects)"), "EVENT_FLOWS", s("orderPlaced")));
    }
    want.insert((s("services/orders"), s("services/notify"), "EVENT_FLOWS", s("orderShipped")));
    assert_eq!(links, want);
    assert_eq!(map.links.len(), 7);
    assert_eq!(map.self_links, 2, "orders -> orders and billing -> billing");
    assert_eq!(map.unlocated_nodes, 0);
}

/// LA.17's post-cache Connect nodes are owned too: the owner pass runs after
/// `apply_rpc_needles`. The proto GRPC_SERVICE contract stays unowned.
#[test]
fn connect_procedures_take_the_owner() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    let connect = bench_fixture("xcut-connect-rpc");
    copy_tree(&connect.join("server"), &d.join("services/eliza"));
    copy_tree(&connect.join("web"), &d.join("apps/web"));
    write(d, "apps/web/package.json", "{\"name\": \"web\"}\n");
    let m = build(d).merged;
    let say = "connectrpc.eliza.v1.ElizaService.Say";
    let procedures = qnames_of(&m, node_kind::RPC_PROCEDURE);
    assert!(procedures.contains(&format!("rpc:{say} @services/eliza")), "{procedures:?}");
    assert!(procedures.iter().all(|q| q.ends_with(" @services/eliza")), "{procedures:?}");
    assert_eq!(qnames_of(&m, node_kind::RPC_CALL), [format!("rpc_call:{say} @apps/web")]);
    assert_eq!(
        edges(&m, edge_category::RPC_CALLS),
        [(format!("rpc_call:{say} @apps/web"), format!("rpc:{say} @services/eliza"))]
    );
    let services = qnames_of(&m, node_kind::GRPC_SERVICE);
    assert!(!services.is_empty(), "the proto declares ElizaService");
    assert!(services.iter().all(|q| !q.contains(" @")), "a GRPC_SERVICE is never owned: {services:?}");
}

/// A Solidity `event` is an EVENT_EMITTER with a code qname: never owned, and
/// counted `declared` on the marker.
#[test]
fn solidity_declared_events_keep_code_qnames() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    write(d, "contracts/package.json", "{\"name\": \"contracts\"}\n");
    write(
        d,
        "contracts/Auction.sol",
        "pragma solidity ^0.8.0;\n\ncontract Auction {\n    event BidPlaced(address bidder);\n\n    \
         function bid() public {\n        emit BidPlaced(msg.sender);\n    }\n}\n",
    );
    let m = build(d).merged;
    assert_eq!(qnames_of(&m, node_kind::EVENT_EMITTER), ["contracts::Auction::Auction::BidPlaced"]);
    let owner = build_stderr(d, "[channel-owner]");
    assert_eq!(owner.len(), 1, "{owner:?}");
    assert!(
        owner[0].starts_with(
            "[channel-owner] qualified rpc=0 event=0 over 0 owners (declared=1 unplaced=0 foreign=0) repo="
        ),
        "{}",
        owner[0]
    );
}

/// The owner pass runs post-cache, so an incremental build (cold, then warm
/// off the persisted parse cache) writes the same `.gmap` bytes as a clean one,
/// transport ORIGIN cells included.
#[test]
fn incremental_matches_clean() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path().join("mono");
    copy_tree(&bench_fixture("rpc-event-monorepo-owner"), &repo);
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
    let clean = generate_one(path).expect("clean");
    assert!(
        qnames_of(&warm.merged, node_kind::EVENT_HANDLER)
            .contains(&s("event_handle:orderShipped @services/notify")),
        "the warm build qualifies cache-served parses too"
    );
    let clean_bytes = write_gmap(&clean, "clean");
    assert_eq!(write_gmap(&cold, "cold"), clean_bytes, "cold incremental == clean");
    assert_eq!(write_gmap(&warm, "warm"), clean_bytes, "warm incremental == clean");
}
