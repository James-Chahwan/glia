//! glia-core — domain-agnostic knowledge graph primitives.
//!
//! Strict Node shape: `{id, repo, confidence, cells}`. Navigation lives in
//! domain-owned indices stored in the container, not in Node fields — this
//! keeps the core agnostic to code vs chemistry vs video vs policy.
//!
//! Cell payloads are one of `Text` / `Json` / `Bytes`. Cell/Edge/NodeKind
//! tags are `u32` registry-backed; the registries live in the container
//! header (not in this crate). `GraphType` is a self-describing string.
//!
//! See memory: `reference_format_spec.md`, `project_040_vision.md`.
//!
//! # No domain residue
//!
//! Nothing here names a code concept. The old code-flow enum and struct were
//! never archived or read, and the manifest-reading project-name helper lives in
//! `glia_code_domain::project_roots::project_name` (LD.10). The
//! `compile_fail` doctests below keep them out; this one is their control: the
//! same path shapes compile against items that do exist.
//!
//! ```
//! let _ = glia_core::Confidence::Strong;
//! let _ = glia_core::GraphType("code".into()).as_str().len();
//! ```
//!
//! ```compile_fail
//! let _ = glia_core::FlowKind::Http;
//! ```
//!
//! ```compile_fail
//! let _ = glia_core::project_name(std::path::Path::new("."));
//! ```
//!
//! ```compile_fail
//! let _ = glia_core::GraphType::code();
//! ```

use core::hash::Hasher;
use twox_hash::XxHash64;

// ============================================================================
// IDs
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct RepoId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct ShardId(pub u64);

/// Self-describing graph-type tag — one per container file.
/// Code = `"code"`, chemistry = `"chemistry"`, etc. Core interprets no values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct GraphType(pub String);

impl GraphType {
    pub fn as_str(&self) -> &str { &self.0 }
}

impl NodeId {
    /// `NodeId = xxhash(graph_type, repo, kind, qualified_name)`.
    /// Separators prevent field-boundary collisions.
    pub fn from_parts(graph_type: &str, repo: RepoId, kind: NodeKindId, qname: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write(graph_type.as_bytes());
        h.write_u8(0xFF);
        h.write_u64(repo.0);
        h.write_u8(0xFF);
        h.write_u32(kind.0);
        h.write_u8(0xFF);
        h.write(qname.as_bytes());
        Self(h.finish())
    }
}

impl RepoId {
    pub fn from_canonical(url_or_path: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write(url_or_path.as_bytes());
        Self(h.finish())
    }
}

impl ShardId {
    pub fn from_parts(repo: RepoId, shard_name: &str) -> Self {
        let mut h = XxHash64::with_seed(0);
        h.write_u64(repo.0);
        h.write_u8(0xFF);
        h.write(shard_name.as_bytes());
        Self(h.finish())
    }
}

// ============================================================================
// Kinds & registry-backed tags
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub enum Confidence {
    Strong,
    Medium,
    Weak,
}

/// Registry-backed cell type tag. Interpretation lives in the per-domain
/// cell registry stored in the container header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct CellTypeId(pub u32);

/// Registry-backed edge category tag. Interpretation lives in the per-domain
/// edge registry stored in the container header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct EdgeCategoryId(pub u32);

/// Registry-backed node-kind tag. Code: Module/Class/Method/Route/...
/// Chemistry: Atom/Bond/Molecule/...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct NodeKindId(pub u32);

// ============================================================================
// Cells
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub enum CellPayload {
    /// Most cells: code, intent, doc, conv.
    Text(String),
    /// Structured cells: position, attn, decisions.
    Json(String),
    /// Binary cells: cached embeddings.
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct Cell {
    pub kind: CellTypeId,
    pub payload: CellPayload,
}

// ============================================================================
// Core graph types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Node {
    pub id: NodeId,
    pub repo: RepoId,
    pub confidence: Confidence,
    pub cells: Vec<Cell>,
}

/// A directed, categorised edge. `cells` carries what the edge itself asserts
/// (where it was seen, by whom, how it accesses its target) exactly as a
/// [`Node`]'s cells do; it is empty until an emitter stamps one (LC.2).
///
/// Not `Copy`: a cell vector owns heap data. Equality and hashing include the
/// cells, so two builds that saw the same edge at different lines compare
/// unequal: compare edges across builds by [`Edge::key`]. Two call sites of one
/// callee stay two edges; nothing merges same-key edges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug, PartialEq, Eq, Hash))]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub category: EdgeCategoryId,
    pub confidence: Confidence,
    /// Defaulted when absent, so a self-describing serde form written
    /// before the field (JSON without `cells`) still reads. Always written:
    /// `skip_serializing_if` would break the non-self-describing bincode
    /// parse cache, which expects every field in order.
    #[serde(default)]
    pub cells: Vec<Cell>,
}

impl Edge {
    /// An edge with no cells. New emitters build edges with this rather than
    /// a struct literal, so a later field does not touch them.
    pub fn new(from: NodeId, to: NodeId, category: EdgeCategoryId, confidence: Confidence) -> Self {
        Self { from, to, category, confidence, cells: Vec::new() }
    }

    /// `self` with `cell` appended.
    pub fn with_cell(mut self, cell: Cell) -> Self {
        self.cells.push(cell);
        self
    }

    /// The first cell of type `kind`, if the edge carries one.
    pub fn cell(&self, kind: CellTypeId) -> Option<&Cell> {
        self.cells.iter().find(|c| c.kind == kind)
    }

    /// The edge's identity without its cells: `(from, to, category)`. Delta
    /// and dedupe key on this, because cells (a call-site line, say) move
    /// between builds while the edge stays the same edge.
    pub fn key(&self) -> (NodeId, NodeId, EdgeCategoryId) {
        (self.from, self.to, self.category)
    }
}

/// The rank a confidence sorts by: `Strong < Medium < Weak`.
fn confidence_rank(c: Confidence) -> u8 {
    match c {
        Confidence::Strong => 0,
        Confidence::Medium => 1,
        Confidence::Weak => 2,
    }
}

/// `(variant rank, payload bytes)`: `Text < Json < Bytes`, then bytewise.
fn payload_order_key(p: &CellPayload) -> (u8, &[u8]) {
    match p {
        CellPayload::Text(s) => (0, s.as_bytes()),
        CellPayload::Json(s) => (1, s.as_bytes()),
        CellPayload::Bytes(b) => (2, b.as_slice()),
    }
}

fn cell_cmp(a: &Cell, b: &Cell) -> core::cmp::Ordering {
    a.kind
        .0
        .cmp(&b.kind.0)
        .then_with(|| payload_order_key(&a.payload).cmp(&payload_order_key(&b.payload)))
}

/// The canonical total order on edges: `from`, `to`, `category` (by raw id),
/// then confidence (`Strong < Medium < Weak`), then the cells compared
/// lexicographically, each by `(kind, payload variant Text < Json < Bytes,
/// payload bytes)`. Every field takes part, so two edges compare `Equal` only
/// when they are equal: an unstable sort under it yields one order whatever
/// the input order, and so one set of bytes on disk. With no cells it orders
/// exactly as the pre-LC.2 key `(from, to, category, confidence)` did.
///
/// The id newtypes are `Hash`, not `Ord`, on purpose; this reads their `.0`.
pub fn canonical_edge_cmp(a: &Edge, b: &Edge) -> core::cmp::Ordering {
    a.from
        .0
        .cmp(&b.from.0)
        .then_with(|| a.to.0.cmp(&b.to.0))
        .then_with(|| a.category.0.cmp(&b.category.0))
        .then_with(|| confidence_rank(a.confidence).cmp(&confidence_rank(b.confidence)))
        .then_with(|| {
            let mut ai = a.cells.iter();
            let mut bi = b.cells.iter();
            loop {
                match (ai.next(), bi.next()) {
                    (None, None) => return core::cmp::Ordering::Equal,
                    (None, Some(_)) => return core::cmp::Ordering::Less,
                    (Some(_), None) => return core::cmp::Ordering::Greater,
                    (Some(x), Some(y)) => match cell_cmp(x, y) {
                        core::cmp::Ordering::Equal => continue,
                        other => return other,
                    },
                }
            }
        })
}

// ============================================================================
// Traits — same surface on Owned and Archived forms
// ============================================================================

pub trait NodeLike {
    fn id(&self) -> NodeId;
    fn repo(&self) -> RepoId;
    fn confidence(&self) -> Confidence;
    fn cell_count(&self) -> usize;
}

impl NodeLike for Node {
    fn id(&self) -> NodeId { self.id }
    fn repo(&self) -> RepoId { self.repo }
    fn confidence(&self) -> Confidence { self.confidence }
    fn cell_count(&self) -> usize { self.cells.len() }
}

impl NodeLike for ArchivedNode {
    fn id(&self) -> NodeId { NodeId(self.id.0.to_native()) }
    fn repo(&self) -> RepoId { RepoId(self.repo.0.to_native()) }
    fn confidence(&self) -> Confidence { (&self.confidence).into() }
    fn cell_count(&self) -> usize { self.cells.len() }
}

#[allow(clippy::wrong_self_convention)]
pub trait EdgeLike {
    fn from_id(&self) -> NodeId;
    fn to_id(&self) -> NodeId;
    fn category(&self) -> EdgeCategoryId;
    fn confidence(&self) -> Confidence;
    fn cell_count(&self) -> usize;
}

impl EdgeLike for Edge {
    fn from_id(&self) -> NodeId { self.from }
    fn to_id(&self) -> NodeId { self.to }
    fn category(&self) -> EdgeCategoryId { self.category }
    fn confidence(&self) -> Confidence { self.confidence }
    fn cell_count(&self) -> usize { self.cells.len() }
}

impl EdgeLike for ArchivedEdge {
    fn from_id(&self) -> NodeId { NodeId(self.from.0.to_native()) }
    fn to_id(&self) -> NodeId { NodeId(self.to.0.to_native()) }
    fn category(&self) -> EdgeCategoryId { EdgeCategoryId(self.category.0.to_native()) }
    fn confidence(&self) -> Confidence { (&self.confidence).into() }
    fn cell_count(&self) -> usize { self.cells.len() }
}

// Bridge the archived unit-variant enum back to its owned form — needed to
// make the traits uniform across Owned/Archived.
impl From<&ArchivedConfidence> for Confidence {
    fn from(v: &ArchivedConfidence) -> Self {
        match v {
            ArchivedConfidence::Strong => Confidence::Strong,
            ArchivedConfidence::Medium => Confidence::Medium,
            ArchivedConfidence::Weak => Confidence::Weak,
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("id collision: {0}")]
    IdCollision(String),
    #[error("missing parent for node {0:?}")]
    MissingParent(NodeId),
    #[error("invalid utf-8: {0}")]
    InvalidUtf8(#[from] core::str::Utf8Error),
    #[error("registry has no entry for id {0}")]
    RegistryUnknown(u32),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_deterministic() {
        let repo = RepoId::from_canonical("github.com/x/y");
        let a = NodeId::from_parts("code", repo, NodeKindId(1), "foo.bar.baz");
        let b = NodeId::from_parts("code", repo, NodeKindId(1), "foo.bar.baz");
        assert_eq!(a, b);
    }

    #[test]
    fn node_id_separators_prevent_field_collision() {
        let repo = RepoId::from_canonical("r");
        let a = NodeId::from_parts("co", repo, NodeKindId(1), "de");
        let b = NodeId::from_parts("c", repo, NodeKindId(1), "ode");
        assert_ne!(a, b);
    }

    #[test]
    fn rkyv_roundtrip_nodes() {
        let nodes = vec![Node {
            id: NodeId::from_parts("code", RepoId(1), NodeKindId(1), "mod.a"),
            repo: RepoId(1),
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: CellTypeId(0),
                payload: CellPayload::Text("hello".into()),
            }],
        }];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&nodes).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Node>>, rkyv::rancor::Error>(&bytes).unwrap();
        let back: Vec<Node> =
            rkyv::deserialize::<Vec<Node>, rkyv::rancor::Error>(archived).unwrap();
        assert_eq!(nodes, back);
    }

    #[test]
    fn node_like_trait_works_on_both_forms() {
        let n = Node {
            id: NodeId(42),
            repo: RepoId(7),
            confidence: Confidence::Medium,
            cells: vec![],
        };
        // Owned
        assert_eq!(n.id(), NodeId(42));
        assert_eq!(n.repo(), RepoId(7));
        assert_eq!(n.confidence(), Confidence::Medium);
        assert_eq!(n.cell_count(), 0);

        // Archived
        let nodes = vec![n.clone()];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&nodes).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Node>>, rkyv::rancor::Error>(&bytes).unwrap();
        let arch_n = &archived[0];
        assert_eq!(arch_n.id(), NodeId(42));
        assert_eq!(arch_n.repo(), RepoId(7));
        assert_eq!(arch_n.confidence(), Confidence::Medium);
        assert_eq!(arch_n.cell_count(), 0);
    }

    #[test]
    fn edge_like_trait_works_on_both_forms() {
        let e = Edge::new(NodeId(1), NodeId(2), EdgeCategoryId(5), Confidence::Weak);
        assert_eq!(e.from_id(), NodeId(1));
        assert_eq!(e.cell_count(), 0);

        let edges = vec![e];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edges).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Edge>>, rkyv::rancor::Error>(&bytes).unwrap();
        let arch_e = &archived[0];
        assert_eq!(arch_e.from_id(), NodeId(1));
        assert_eq!(arch_e.to_id(), NodeId(2));
        assert_eq!(arch_e.category(), EdgeCategoryId(5));
        assert_eq!(arch_e.confidence(), Confidence::Weak);
        assert_eq!(arch_e.cell_count(), 0);
    }

    fn json_cell(kind: u32, payload: &str) -> Cell {
        Cell { kind: CellTypeId(kind), payload: CellPayload::Json(payload.into()) }
    }

    #[test]
    fn edge_cells_round_trip_rkyv() {
        let e = Edge::new(NodeId(10), NodeId(20), EdgeCategoryId(3), Confidence::Strong)
            .with_cell(json_cell(21, r#"{"line":12,"emitter":"python"}"#));
        assert_eq!(e.cell_count(), 1);
        assert_eq!(e.cell(CellTypeId(21)), Some(&e.cells[0]));
        assert_eq!(e.cell(CellTypeId(22)), None);
        assert_eq!(e.key(), (NodeId(10), NodeId(20), EdgeCategoryId(3)));

        let edges = vec![e.clone()];
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&edges).unwrap();
        let archived =
            rkyv::access::<rkyv::Archived<Vec<Edge>>, rkyv::rancor::Error>(&bytes).unwrap();
        let arch_e = &archived[0];
        assert_eq!(arch_e.cell_count(), 1);
        assert_eq!(arch_e.cells[0].kind.0.to_native(), 21);
        match &arch_e.cells[0].payload {
            ArchivedCellPayload::Json(s) => {
                assert_eq!(s.as_str(), r#"{"line":12,"emitter":"python"}"#)
            }
            other => panic!("expected a Json payload, got {other:?}"),
        }
        let back: Vec<Edge> =
            rkyv::deserialize::<Vec<Edge>, rkyv::rancor::Error>(archived).unwrap();
        assert_eq!(back, edges);

        // serde: JSON written before the field (no `cells` key) still reads,
        // and an edge with cells round-trips.
        let bare = Edge::new(NodeId(1), NodeId(2), EdgeCategoryId(5), Confidence::Weak);
        let old_json = r#"{"from":1,"to":2,"category":5,"confidence":"Weak"}"#;
        assert_eq!(serde_json::from_str::<Edge>(old_json).unwrap(), bare);
        let with = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Edge>(&with).unwrap(), e);
    }

    /// Heap's algorithm: every permutation of `items`, in a fixed order.
    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        let mut a = items.to_vec();
        let n = a.len();
        let mut out = vec![a.clone()];
        let mut c = vec![0usize; n];
        let mut i = 0;
        while i < n {
            if c[i] < i {
                if i % 2 == 0 {
                    a.swap(0, i);
                } else {
                    a.swap(c[i], i);
                }
                out.push(a.clone());
                c[i] += 1;
                i = 0;
            } else {
                c[i] = 0;
                i += 1;
            }
        }
        out
    }

    /// Three edges share one key and differ only in a cell payload, two differ
    /// only in confidence, one differs only by carrying a Text instead of a
    /// Json payload of the same bytes.
    fn tie_edges() -> Vec<Edge> {
        let base = || Edge::new(NodeId(7), NodeId(9), EdgeCategoryId(4), Confidence::Strong);
        vec![
            base().with_cell(json_cell(21, r#"{"line":3}"#)),
            base().with_cell(json_cell(21, r#"{"line":17}"#)),
            base().with_cell(json_cell(21, r#"{"line":170}"#)),
            Edge::new(NodeId(7), NodeId(9), EdgeCategoryId(4), Confidence::Medium),
            Edge::new(NodeId(7), NodeId(9), EdgeCategoryId(4), Confidence::Weak),
            base().with_cell(Cell {
                kind: CellTypeId(21),
                payload: CellPayload::Text(r#"{"line":3}"#.into()),
            }),
        ]
    }

    #[test]
    fn canonical_order_is_total() {
        let edges = tie_edges();
        let perms = permutations(&edges);
        assert_eq!(perms.len(), 720);
        let mut first: Option<Vec<Edge>> = None;
        for mut p in perms {
            p.sort_unstable_by(canonical_edge_cmp);
            match &first {
                None => first = Some(p),
                Some(f) => assert_eq!(&p, f),
            }
        }
        let sorted = first.unwrap();
        // Cells sort after the bare edge of the same confidence: only the
        // cell-less Medium / Weak ones follow the Strong ones.
        assert_eq!(sorted[4].confidence, Confidence::Medium);
        assert_eq!(sorted[5].confidence, Confidence::Weak);
        // Text < Json at equal kind, then payload bytes.
        assert!(matches!(sorted[0].cells[0].payload, CellPayload::Text(_)));
        let lines: Vec<&CellPayload> = sorted[1..4].iter().map(|e| &e.cells[0].payload).collect();
        assert_eq!(
            lines,
            vec![
                &CellPayload::Json(r#"{"line":170}"#.into()),
                &CellPayload::Json(r#"{"line":17}"#.into()),
                &CellPayload::Json(r#"{"line":3}"#.into()),
            ]
        );
        // Equal only when equal.
        for a in &sorted {
            for b in &sorted {
                assert_eq!(canonical_edge_cmp(a, b) == core::cmp::Ordering::Equal, a == b);
            }
        }
        // No cells: the pre-LC.2 key order.
        let mut plain = vec![
            Edge::new(NodeId(2), NodeId(1), EdgeCategoryId(1), Confidence::Weak),
            Edge::new(NodeId(1), NodeId(3), EdgeCategoryId(2), Confidence::Strong),
            Edge::new(NodeId(1), NodeId(3), EdgeCategoryId(1), Confidence::Medium),
            Edge::new(NodeId(1), NodeId(2), EdgeCategoryId(9), Confidence::Weak),
        ];
        let mut by_old_key = plain.clone();
        by_old_key.sort_by_key(|e| (e.from.0, e.to.0, e.category.0, confidence_rank(e.confidence)));
        plain.sort_unstable_by(canonical_edge_cmp);
        assert_eq!(plain, by_old_key);
    }

    #[test]
    fn hash_collision_smoke_1000() {
        use std::collections::HashSet;
        let repo = RepoId::from_canonical("repo");
        let mut seen = HashSet::new();
        for i in 0..1000 {
            let id = NodeId::from_parts("code", repo, NodeKindId(1), &format!("entity_{i}"));
            assert!(seen.insert(id), "collision at i={i}");
        }
    }
}
