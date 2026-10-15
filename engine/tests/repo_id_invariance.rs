//! DE.1: graph CONTENT does not depend on the RepoId. A NodeId hashes the
//! RepoId (LB.1: the remote URL, or the directory name outside git), so one
//! tree under two identities has two id sets; nothing a build decides may read
//! that order. Before DE.1 the doc linker bound an ambiguous backticked name to
//! the candidate with the lowest NodeId, so the same README documented a
//! different symbol under a different directory name (Engram's two Kina
//! clones, docs/eval-plan-2026-09.md).
//!
//! Every copy here lives under one per-run parent in `std::env::temp_dir()`,
//! never inside the glia checkout: there LB.1 would key each copy
//! `git:<glia remote>/<rel>`, here each is `dir:<name>`. Only qnames, kinds,
//! confidences and cell payloads are compared, never a NodeId.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glia_code_domain::evidence::Evidence;
use glia_code_domain::walk_gating::repo_identity;
use glia_code_domain::edge_category;
use glia_core::{Cell, CellPayload, NodeId};
use glia_engine::generate_one;
use glia_graph::MergedGraph;

/// A per-run parent directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("glia-repo-id-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch parent");
        Scratch(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn build(root: &Path) -> MergedGraph {
    generate_one(&root.to_string_lossy()).unwrap_or_else(|e| panic!("build {}: {e}", root.display())).merged
}

/// `(qname, kind id)` of every node, for edge endpoints. Lookup only.
fn endpoints(m: &MergedGraph) -> HashMap<NodeId, (String, u32)> {
    let mut out = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            let kind = g.nav.kind_by_id.get(&n.id).map_or(u32::MAX, |k| k.0);
            out.insert(n.id, (qname, kind));
        }
    }
    out
}

fn payload_text(c: &Cell) -> (u32, String) {
    let text = match &c.payload {
        CellPayload::Text(s) => format!("text:{s}"),
        CellPayload::Json(s) => format!("json:{s}"),
        CellPayload::Bytes(b) => format!("bytes:{b:?}"),
    };
    (c.kind.0, text)
}

type NodeRow = (String, u32, String, Vec<(u32, String)>);
type EdgeRow = (String, u32, u32, String, u32, String, Vec<(u32, String)>);

/// The qname-level graph: nodes as (qname, kind, confidence, cells) and edges
/// as (from qname, from kind, category, to qname, to kind, confidence, cells),
/// each sorted. No NodeId appears in either.
fn projection(m: &MergedGraph) -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let ends = endpoints(m);
    let mut nodes: Vec<NodeRow> = m
        .graphs
        .iter()
        .flat_map(|g| g.nodes.iter().map(move |n| (g, n)))
        .map(|(g, n)| {
            (
                g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default(),
                g.nav.kind_by_id.get(&n.id).map_or(u32::MAX, |k| k.0),
                format!("{:?}", n.confidence),
                n.cells.iter().map(payload_text).collect(),
            )
        })
        .collect();
    nodes.sort();
    let unknown = (String::from("?"), u32::MAX);
    let mut edges: Vec<EdgeRow> = m
        .all_edges()
        .map(|e| {
            let (fq, fk) = ends.get(&e.from).unwrap_or(&unknown).clone();
            let (tq, tk) = ends.get(&e.to).unwrap_or(&unknown).clone();
            (fq, fk, e.category.0, tq, tk, format!("{:?}", e.confidence), e.cells.iter().map(payload_text).collect())
        })
        .collect();
    edges.sort();
    (nodes, edges)
}

/// Every DOCUMENTS edge as (from qname, to qname, confidence, EVIDENCE rule), sorted.
fn documents(m: &MergedGraph) -> Vec<(String, String, String, String)> {
    let ends = endpoints(m);
    let qname = |id: &NodeId| ends.get(id).map(|(q, _)| q.clone()).unwrap_or_default();
    let mut out: Vec<_> = m
        .all_edges()
        .filter(|e| e.category == edge_category::DOCUMENTS)
        .map(|e| {
            let rule = Evidence::of(e).and_then(|ev| ev.rule).unwrap_or_default();
            (qname(&e.from), qname(&e.to), format!("{:?}", e.confidence), rule)
        })
        .collect();
    out.sort();
    out
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).expect("create dir");
    }
    std::fs::write(p, text).expect("write file");
}

/// The minimal repro: one bare `refresh` (two candidates) and one qualified
/// `Feed.refresh` (one candidate). Before DE.1 the bare mention bound to
/// whichever `refresh` hashed lowest under the directory's RepoId; when that
/// was `Feed.refresh` the qualified mention was deduped and the section held
/// one edge, else two.
#[test]
fn doclink_target_is_independent_of_the_repo_identity() {
    let scratch = Scratch::new("doclink");
    let names = ["doclink-a", "doclink-b", "doclink-c", "doclink-d", "doclink-e", "doclink-f"];
    let mut keys = Vec::new();
    for name in names {
        let root = scratch.path(name);
        write(&root, "README.md", "# Notes\n\n## Caching\n\nCall `refresh` after a write; `Feed.refresh` reloads the feed.\n");
        write(&root, "billing/wallet.py", "class Wallet:\n    def refresh(self):\n        return 1\n");
        write(&root, "web/feed.py", "class Feed:\n    def refresh(self):\n        return 2\n");
        keys.push(repo_identity(&root).key);
    }
    let mut distinct = keys.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), names.len(), "each copy has its own RepoId: {keys:?}");

    let expected = vec![
        (
            "docs::README::caching".to_string(),
            "billing::wallet::Wallet::refresh".to_string(),
            "Weak".to_string(),
            "ambiguous".to_string(),
        ),
        (
            "docs::README::caching".to_string(),
            "web::feed::Feed::refresh".to_string(),
            "Strong".to_string(),
            "qualified".to_string(),
        ),
    ];
    let wrong: Vec<_> = names
        .iter()
        .map(|name| (*name, documents(&build(&scratch.path(name)))))
        .filter(|(_, got)| *got != expected)
        .collect();
    assert!(wrong.is_empty(), "{} of {} names differ from {expected:?}: {wrong:#?}", wrong.len(), names.len());
}

/// Copy `src` to `dst`, dotfiles included (`.glia/` snapshots, sidecars and
/// overlays are build inputs), skipping `.ai/` (an untracked cache an old
/// wheel left in some fixtures) and a persisted `.glia/graph/` layout.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create copy dir");
    let mut entries: Vec<_> = std::fs::read_dir(src).expect("read fixture dir").flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let from = e.path();
        let to = dst.join(&name);
        let ty = e.file_type().expect("file type");
        if ty.is_dir() {
            if name == ".ai" || (name == "graph" && src.file_name().is_some_and(|n| n == ".glia")) {
                continue;
            }
            copy_tree(&from, &to);
        } else if ty.is_file() {
            std::fs::copy(&from, &to).expect("copy file");
        }
    }
}

/// A guard: these fixtures (a monorepo, a history snapshot, the doc-link cap,
/// project roots, Go mounts, gRPC, Angular HTTP, an external-cells sidecar,
/// a SCIP snapshot) build to the same qname-level graph under two directory
/// names. It held before DE.1; it stops any later RepoId-ordered pick.
#[test]
fn graph_content_is_independent_of_the_repo_identity() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/substrate-gap/fixtures");
    let scratch = Scratch::new("fixtures");
    for f in [
        "channel-monorepo-owner",
        "history-cochange",
        "docs-link-past-cap",
        "walk-project-roots",
        "go-route-mounts",
        "xcut-grpc-grpc_calls",
        "angular-http-calls",
        "external-cells-sidecar",
        "scip-python-dict-dispatch",
    ] {
        let (one, two) = (scratch.path(&format!("{f}-one")), scratch.path(&format!("{f}-two")));
        copy_tree(&fixtures.join(f), &one);
        copy_tree(&fixtures.join(f), &two);
        assert_ne!(repo_identity(&one).key, repo_identity(&two).key, "{f}: the copies share a RepoId");
        let (a, b) = (build(&one), build(&two));
        let (pa, pb) = (projection(&a), projection(&b));
        assert!(!pa.0.is_empty() && !pa.1.is_empty(), "{f}: an empty graph proves nothing");
        assert_eq!(pa.0.len(), pb.0.len(), "{f}: node counts differ");
        for (x, y) in pa.0.iter().zip(&pb.0) {
            assert_eq!(x, y, "{f}: nodes differ");
        }
        assert_eq!(pa.1.len(), pb.1.len(), "{f}: edge counts differ");
        for (x, y) in pa.1.iter().zip(&pb.1) {
            assert_eq!(x, y, "{f}: edges differ");
        }
        eprintln!("[repo-id-invariance] {f}: {} nodes, {} edges equal under two RepoIds", pa.0.len(), pa.1.len());
    }
}
