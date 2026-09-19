//! LF.3b: `.glia/overlay.toml` `[entrypoints]` qname patterns land as
//! ENTRYPOINT cells on the nodes they name, and liveness seeds from them.
//!
//! Probe e1: `app/jobs.py` `nightly_rollup()` is wired by a scheduler string,
//! so nothing in the graph calls it, and `summarise()`, its only callee,
//! reads `live: false`. Declaring the job an entrypoint makes both live.
//!
//! The layout is the substrate-gap fixture `overlay-entrypoints`, copied into
//! a temp dir per test so a test can swap its overlay file.

use std::path::Path;

use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_engine::{
    BlastOptions, BuildOptions, GenerateResult, ParseCache, blast_radius, entrypoint_reachable,
    generate_one, generate_one_opts, generate_one_with_cache,
};
use repo_graph_graph::{MergedGraph, Reach};
use repo_graph_store::{read_merged_sharded, write_merged_sharded};

const FIXTURE: &str = "../bench/substrate-gap/fixtures/overlay-entrypoints";
const OVERLAY: &str = include_str!("../../bench/substrate-gap/fixtures/overlay-entrypoints/.glia/overlay.toml");
const FILES: &[&str] = &["app/jobs.py"];

const JOB: &str = "app::jobs::nightly_rollup";
const HELPER: &str = "app::jobs::summarise";

/// The fixture tree under `<tmp>/repo`, with `overlay` as its overlay file
/// (none when `None`).
fn repo(tmp: &Path, overlay: Option<&str>) -> String {
    let dir = tmp.join("repo");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    for f in FILES {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        std::fs::copy(src.join(f), dir.join(f)).unwrap();
    }
    if let Some(o) = overlay {
        std::fs::create_dir_all(dir.join(".glia")).unwrap();
        std::fs::write(dir.join(".glia/overlay.toml"), o).unwrap();
    }
    dir.to_str().unwrap().to_string()
}

fn overlay(patterns: &[&str]) -> String {
    let list: Vec<String> = patterns.iter().map(|p| format!("{p:?}")).collect();
    format!("version = 1\n\n[entrypoints]\nqnames = [{}]\n", list.join(", "))
}

fn build(overlay: Option<&str>) -> GenerateResult {
    let tmp = tempfile::tempdir().unwrap();
    generate_one(&repo(tmp.path(), overlay)).unwrap()
}

/// The one node whose qname is `qname` and kind is FUNCTION.
fn function(merged: &MergedGraph, qname: &str) -> NodeId {
    let ids: Vec<NodeId> = merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter(move |n| {
                g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname)
                    && g.nav.kind_by_id.get(&n.id) == Some(&node_kind::FUNCTION)
            })
        })
        .map(|n| n.id)
        .collect();
    assert_eq!(ids.len(), 1, "exactly one FUNCTION is {qname}");
    ids[0]
}

/// Every `(qname, payload)` of an ENTRYPOINT cell in the graph, in node order.
fn entrypoints(merged: &MergedGraph) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            for c in n.cells.iter().filter(|c| c.kind == cell_type::ENTRYPOINT) {
                let CellPayload::Json(s) = &c.payload else {
                    panic!("ENTRYPOINT is a Json cell, got {:?}", c.payload);
                };
                out.push((g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default(), s.clone()));
            }
        }
    }
    out
}

/// `summarise`'s `live` flag in the forward blast radius of the job.
fn helper_live(merged: &MergedGraph) -> bool {
    let mut forward = BlastOptions::default();
    forward.direction = Reach::Forward;
    let answer = blast_radius(merged, &[JOB], &forward);
    assert!(answer.unresolved.is_empty(), "the job resolves");
    answer
        .results
        .iter()
        .find(|h| h.qname == HELPER)
        .unwrap_or_else(|| panic!("{HELPER} is in the radius, got {:?}", answer.results))
        .live
}

#[test]
fn declared_job_is_live() {
    let bare = build(None);
    assert!(!helper_live(&bare.merged), "control: without the declaration summarise is dead");
    assert!(entrypoints(&bare.merged).is_empty());

    let r = build(Some(OVERLAY));
    assert_eq!(
        entrypoints(&r.merged),
        [(
            JOB.to_string(),
            r#"{"decl":".glia/overlay.toml:4","pattern":"app::jobs::nightly_rollup","source":"config"}"#.to_string()
        )]
    );
    assert!(helper_live(&r.merged), "summarise is reached from the declared job");
    let live = entrypoint_reachable(&r.merged);
    assert!(live.contains(&function(&r.merged, JOB)));
    assert!(live.contains(&function(&r.merged, HELPER)));
}

#[test]
fn prefix_pattern_matches_descendants() {
    let r = build(Some(&overlay(&["app::jobs::*"])));
    let got = entrypoints(&r.merged);
    let qnames: Vec<&str> = got.iter().map(|(q, _)| q.as_str()).collect();
    assert_eq!(qnames, [JOB, HELPER], "every node under app::jobs::, and not app::jobs itself");
    for (_, p) in &got {
        assert!(p.contains(r#""pattern":"app::jobs::*""#), "{p}");
    }
    assert!(helper_live(&r.merged));
}

#[test]
fn first_matching_pattern_wins() {
    let r = build(Some(&overlay(&["app::jobs::*", JOB])));
    let got = entrypoints(&r.merged);
    assert_eq!(got.len(), 2, "one cell per node: {got:?}");
    assert!(got.iter().all(|(_, p)| p.contains(r#""pattern":"app::jobs::*""#)), "{got:?}");
}

/// A pattern that binds nothing writes nothing: every edge and every node
/// cell is what the same tree without the overlay file gives.
#[test]
fn unmatched_pattern_changes_nothing() {
    let with = build(Some(&overlay(&["app::none", "app::jobs::nightly_rollup::*"])));
    let bare = build(None);
    assert!(entrypoints(&with.merged).is_empty());
    assert!(!helper_live(&with.merged));
    assert_eq!(with.merged.cross_edges, bare.merged.cross_edges);
    assert_eq!(with.merged.graphs.len(), bare.merged.graphs.len());
    for (a, b) in with.merged.graphs.iter().zip(&bare.merged.graphs) {
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.nodes, b.nodes);
    }
}

/// `[entrypoints]` is user config, not inference: `--no-overlay` keeps it.
#[test]
fn no_overlay_keeps_declared_entrypoints() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = repo(tmp.path(), Some(OVERLAY));
    let on = generate_one(&dir).unwrap();
    let off = generate_one_opts(&dir, false, &BuildOptions::default().with_overlay(false)).unwrap();
    assert_eq!(entrypoints(&off.merged).len(), 1);
    assert_eq!(entrypoints(&off.merged), entrypoints(&on.merged));
}

#[test]
fn entrypoint_survives_gmap_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let r = generate_one(&repo(tmp.path(), Some(OVERLAY))).unwrap();
    let out = tmp.path().join("gmap");
    write_merged_sharded(&r.merged, &out).unwrap();
    let back = read_merged_sharded(&out).unwrap();
    assert_eq!(entrypoints(&back), entrypoints(&r.merged));
    let live = entrypoint_reachable(&back);
    assert!(live.contains(&function(&back, JOB)), "the declared job is an entry after the round trip");
    assert!(live.contains(&function(&back, HELPER)));
    assert!(helper_live(&back));
}

/// Map of file name -> bytes for every file in a sharded output dir
/// (byte_identical.rs's helper).
fn dir_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn declared_entrypoint_builds_are_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_s = repo(tmp.path(), Some(&overlay(&["app::jobs::*"])));

    let out = |name: &str| tmp.path().join(name);
    write_merged_sharded(&generate_one(&repo_s).unwrap().merged, &out("clean1")).unwrap();
    write_merged_sharded(&generate_one(&repo_s).unwrap().merged, &out("clean2")).unwrap();
    let mut cache = ParseCache::new();
    generate_one_with_cache(&repo_s, &mut cache).unwrap();
    let warm = generate_one_with_cache(&repo_s, &mut cache).unwrap();
    assert!(cache.stats.reused > 0, "the cached build must reuse its parse");
    write_merged_sharded(&warm.merged, &out("cached")).unwrap();

    let clean1 = dir_bytes(&out("clean1"));
    assert!(!clean1.is_empty());
    assert_eq!(clean1, dir_bytes(&out("clean2")), "clean vs clean");
    assert_eq!(clean1, dir_bytes(&out("cached")), "clean vs cached");
    assert_eq!(entrypoints(&warm.merged).len(), 2);
}
