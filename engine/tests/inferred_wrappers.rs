//! CA.4: a Go function that hands its own parameter to the driver's
//! `.Collection(name)` (or to another such function) is a data_entity
//! wrapper with no `.glia/overlay.toml`: each call site's literal at that
//! parameter mints `data_entity:nosql:<name>` with a function-level
//! ACCESSES_DATA, through the LF.2e / LG.3d wrapper stage.
//!
//! Pre-fix baseline (HEAD 797e494, fixture go-inferred-collection-wrapper,
//! grade.py on the W1 wheel): DATA_ENTITY 0/2, ACCESSES_DATA 0/2, ORIGIN 0/1,
//! FORBID VIOLATIONS 0; quokka a77d4cb with no overlay: 0 DATA_ENTITY nodes.

use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, CellTypeId, Confidence, Edge, Node, NodeId};
use glia_engine::{
    BuildOptions, GenerateResult, ParseCache, generate_many_opts, generate_one,
    generate_one_with_cache,
};
use glia_store::write_merged_sharded;

/// The substrate-gap fixture this packet adds.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/go-inferred-collection-wrapper")
}

/// The LG.3d fixture, whose overlay declares `NewCollection` by hand.
fn overlay_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bench/substrate-gap/fixtures/go-overlay-data-wrapper")
}

fn s(p: &Path) -> String {
    p.to_str().expect("utf-8 path").to_string()
}

/// A fresh, empty temp dir for one test.
fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("glia_ca4_{}_{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Copy the files of `from` (recursively) under `to`.
fn copy_tree(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).expect("read dir").flatten() {
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            std::fs::create_dir_all(&dest).expect("mkdir");
            copy_tree(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), &dest).expect("copy");
        }
    }
}

fn qname_of(r: &GenerateResult, id: NodeId) -> String {
    r.merged
        .graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

fn node(r: &GenerateResult, qname: &str) -> Option<Node> {
    r.merged.graphs.iter().find_map(|g| {
        g.nodes
            .iter()
            .find(|n| g.nav.qname_by_id.get(&n.id).is_some_and(|q| q == qname))
            .cloned()
    })
}

/// Every DATA_ENTITY qname, sorted.
fn entities(r: &GenerateResult) -> Vec<String> {
    let mut out: Vec<String> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| {
            g.nodes.iter().filter_map(move |n| {
                (g.nav.kind_by_id.get(&n.id) == Some(&node_kind::DATA_ENTITY))
                    .then(|| g.nav.qname_by_id.get(&n.id).cloned())
                    .flatten()
            })
        })
        .collect();
    out.sort();
    out
}

/// The ACCESSES_DATA edges into `entity`.
fn accesses<'r>(r: &'r GenerateResult, entity: &str) -> Vec<&'r Edge> {
    r.merged
        .all_edges()
        .filter(|e| e.category == edge_category::ACCESSES_DATA && qname_of(r, e.to) == entity)
        .collect()
}

fn json_cells(cells: &[glia_core::Cell], kind: CellTypeId) -> Vec<serde_json::Value> {
    cells
        .iter()
        .filter(|c| c.kind == kind)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(j) => serde_json::from_str(j).ok(),
            _ => None,
        })
        .collect()
}

/// Assert `entity` is minted by the inferred wrapper `call` (defined at
/// `def`) with one Medium ACCESSES_DATA from the function whose qname ends
/// in `from`, evidenced at 0-based `line` of `file`.
fn assert_inferred(
    r: &GenerateResult,
    entity: &str,
    call: &str,
    def: &str,
    from: &str,
    (file, line): (&str, u32),
) {
    let n = node(r, entity).unwrap_or_else(|| panic!("{entity}: {:?}", entities(r)));
    assert_eq!(n.confidence, Confidence::Medium, "{entity}");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({
            "provenance": "inferred:wrapper",
            "rule": format!("inferred:{call}"),
            "def": def
        })],
        "{entity}"
    );
    let edges = accesses(r, entity);
    assert_eq!(
        edges.len(),
        1,
        "{entity}: one function-level edge, no module edge"
    );
    let e = edges[0];
    assert!(
        qname_of(r, e.from).ends_with(from),
        "{}",
        qname_of(r, e.from)
    );
    assert_eq!(e.confidence, Confidence::Medium);
    let ev = Evidence::of(e).expect("EVIDENCE");
    let rule = format!("inferred:{call}");
    assert_eq!(
        (
            ev.emitter.as_str(),
            ev.rule.as_deref(),
            ev.file.as_deref(),
            ev.line
        ),
        (
            "pass:inferred_wrapper",
            Some(rule.as_str()),
            Some(file),
            Some(line)
        ),
        "{entity}"
    );
}

/// (a) No overlay: the direct wrapper and the one forwarding into it both
/// mint; the commented call, the definitions and the forwarded non-literal
/// mint nothing.
#[test]
fn inferred_wrappers_mint_without_an_overlay() {
    let root = fixture();
    assert!(!root.join(".glia").exists(), "the fixture holds no overlay");
    let r = generate_one(&s(&root)).expect("fixture builds");
    assert_eq!(
        entities(&r),
        ["data_entity:nosql:chat_previews", "data_entity:nosql:users"],
        "no legacy_previews / name / database"
    );
    assert_inferred(
        &r,
        "data_entity:nosql:chat_previews",
        "NewCollection",
        "repositories/collection.go:13",
        "NewChatPreviewRepository",
        ("repositories/chat_preview_repository.go", 20),
    );
    assert_inferred(
        &r,
        "data_entity:nosql:users",
        "NewNamedCollection",
        "repositories/collection.go:18",
        "NewUserRepository",
        ("repositories/user_repository.go", 13),
    );
    // `why` tiers the edge DERIVED (stage `pass`), with no overlay note.
    let why = glia_engine::why::why_edge(
        &r.merged,
        "NewUserRepository",
        "data_entity:nosql:users",
        Some("ACCESSES_DATA"),
    )
    .expect("why");
    assert!(why.found);
    let row = &why.edges[0];
    assert_eq!(
        (row.tier, row.emitter.as_deref(), row.note.as_deref()),
        ("derived", Some("pass:inferred_wrapper"), None)
    );
}

/// A direct driver call naming the same collection collapses onto the one
/// node the inferred wrapper mints (the extractor's qname).
#[test]
fn inferred_and_extracted_sites_share_one_node() {
    let root = tmp("shared");
    copy_tree(&fixture(), &root);
    std::fs::write(
        root.join("repositories/count.go"),
        "package repositories\n\nimport \"go.mongodb.org/mongo-driver/mongo\"\n\nfunc CountUsers(client *mongo.Client) {\n\tclient.Database(\"app\").Collection(\"users\")\n}\n",
    )
    .expect("write");
    let r = generate_one(&s(&root)).expect("build");
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(
        entities(&r),
        ["data_entity:nosql:chat_previews", "data_entity:nosql:users"]
    );
    let mut from: Vec<String> = accesses(&r, "data_entity:nosql:users")
        .iter()
        .map(|e| qname_of(&r, e.from))
        .collect();
    from.sort();
    assert_eq!(from.len(), 2, "{from:?}");
    assert!(
        from[0].ends_with("CountUsers") && from[1].ends_with("NewUserRepository"),
        "{from:?}"
    );
}

/// Every node and edge (with cells) of a build, keyed by identity.
fn contents(r: &GenerateResult) -> (Vec<Node>, Vec<Edge>) {
    let mut nodes: Vec<Node> = r
        .merged
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().cloned())
        .collect();
    nodes.sort_by_key(|n| n.id.0);
    let mut edges: Vec<Edge> = r.merged.all_edges().cloned().collect();
    edges.sort_by_key(|e| (e.from.0, e.to.0, e.category.0));
    (nodes, edges)
}

/// (b) The LG.3d fixture WITH its overlay: the hand stanza shadows the
/// inference, so the ORIGIN stays overlay:human and no inferred provenance
/// or emitter appears. Built again without the overlay, the inference mints
/// the same node and edge: only their provenance (ORIGIN, EVIDENCE) moves.
#[test]
fn an_overlay_stanza_shadows_the_inference() {
    let root = overlay_fixture();
    let with = generate_one(&s(&root)).expect("with overlay");
    let q = "data_entity:nosql:chat_previews";
    assert_eq!(entities(&with), [q]);
    let n = node(&with, q).expect("node");
    assert_eq!(
        json_cells(&n.cells, cell_type::ORIGIN),
        [serde_json::json!({"provenance": "overlay:human", "rule": "wrapper#1"})]
    );
    let (nodes, edges) = contents(&with);
    for n in &nodes {
        for c in json_cells(&n.cells, cell_type::ORIGIN) {
            assert_ne!(c["provenance"], "inferred:wrapper", "{c}");
        }
    }
    for e in &edges {
        let ev = Evidence::of(e).expect("EVIDENCE");
        assert_ne!(ev.emitter, "pass:inferred_wrapper");
    }

    // (c) `--no-overlay` still infers.
    let without = generate_many_opts(
        &[s(&root)],
        false,
        &BuildOptions::default().with_overlay(false),
    )
    .expect("without overlay");
    assert_eq!(entities(&without), [q]);
    let (nodes_off, edges_off) = contents(&without);
    assert_eq!(
        nodes.iter().map(|n| n.id).collect::<Vec<_>>(),
        nodes_off.iter().map(|n| n.id).collect::<Vec<_>>()
    );
    assert_eq!(
        edges
            .iter()
            .map(|e| (e.from, e.to, e.category))
            .collect::<Vec<_>>(),
        edges_off
            .iter()
            .map(|e| (e.from, e.to, e.category))
            .collect::<Vec<_>>()
    );
    let entity_id = n.id;
    for (a, b) in nodes.iter().zip(&nodes_off) {
        if a.id == entity_id {
            let o = json_cells(&b.cells, cell_type::ORIGIN);
            assert_eq!(o[0]["provenance"], "inferred:wrapper", "{o:?}");
            assert_eq!(o[0]["def"], "collection.go:13", "{o:?}");
        } else {
            assert_eq!(a, b, "only the entity's provenance moves");
        }
    }
    for (a, b) in edges.iter().zip(&edges_off) {
        if a.to == entity_id {
            let (ea, eb) = (Evidence::of(a).expect("ev"), Evidence::of(b).expect("ev"));
            assert_eq!(
                (ea.emitter.as_str(), eb.emitter.as_str()),
                ("overlay:wrapper", "pass:inferred_wrapper")
            );
            assert_eq!((ea.file, ea.line), (eb.file, eb.line), "one site");
        } else {
            assert_eq!(a, b, "only the entity edge's evidence moves");
        }
    }
}

/// Map of file name -> bytes of a written layout.
fn store_bytes(r: &GenerateResult, dir: &Path) -> Vec<(String, Vec<u8>)> {
    write_merged_sharded(&r.merged, dir).expect("write store");
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).expect("read"),
            )
        })
        .collect();
    out.sort();
    out
}

/// Clean builds agree byte for byte, and a warm parse cache (the inference
/// runs post-cache) gives the same layout as a clean build.
#[test]
fn inferred_builds_are_byte_identical() {
    let root = tmp("bytes");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    copy_tree(&fixture(), &repo);
    let path = s(&repo);
    let a = generate_one(&path).expect("build a");
    let b = generate_one(&path).expect("build b");
    assert_eq!(entities(&a).len(), 2);
    let bytes_a = store_bytes(&a, &root.join("a"));
    assert_eq!(bytes_a, store_bytes(&b, &root.join("b")), "clean vs clean");
    let mut cache = ParseCache::new();
    let _cold = generate_one_with_cache(&path, &mut cache).expect("cold");
    let warm = generate_one_with_cache(&path, &mut cache).expect("warm");
    assert!(cache.stats.reused > 0, "the parse came from the cache");
    assert_eq!(
        bytes_a,
        store_bytes(&warm, &root.join("warm")),
        "incremental vs clean"
    );
    std::fs::remove_dir_all(&root).ok();
}
