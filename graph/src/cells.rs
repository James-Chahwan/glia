//! External cell writes (LF.1a): the ONE qname resolver and the ONE apply
//! function behind every writer of CONSTRAINT / DECISION / CONV / VECTOR
//! cells - the build's `.glia/cells.jsonl` + `.glia/vectors.jsonl` stage
//! (engine `external::cells`), a live in-memory write and the persisted-gmap
//! write-through (LF.1b). All three bind a row here, so a write made in
//! memory lands on the node the next build re-applies it to.
//!
//! Binding, in order: the exact qname (restricted to one repo, filtered by a
//! NodeKind NAME when the row gives one); several matches narrowed by the
//! row's move-stable hint (LB.6); no match re-bound through the hint
//! ([`IdentityIndex::rebind`]: a HEURISTIC move, reported with its tier). An
//! ambiguous row is reported and never applied. The row shapes and entry
//! rules live in `code_domain::external_inputs`.
//!
//! Module slot declared by L0.3: reached as `repo_graph_graph::cells::<item>`.

use std::collections::{BTreeMap, HashMap};

use repo_graph_code_domain::external_inputs::{
    CellWrite, WRITABLE, WritePayload, check_vector, merge_entry, validate_entry,
};
use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{Cell, CellPayload, NodeId, NodeKindId, RepoId};

use crate::identity::{IdentityIndex, MoveTier, Rebind};
use crate::merged::MergedGraph;

/// Where one write binds.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellTarget {
    /// The exact qname (and kind), or the one the hint singled out of several.
    Bound(NodeId),
    /// No node carries the qname; the hint re-bound it to this moved node.
    /// HEURISTIC evidence: the tier says which.
    Rekeyed { id: NodeId, tier: MoveTier },
    /// Several nodes match and nothing narrows them: nothing is written. The
    /// smallest candidate id, for the report.
    Ambiguous(NodeId),
    /// No node matches: nothing is written.
    Orphaned,
    /// An invalid write (a cell type that is not writable, a payload that
    /// does not fit its cell, an entry that fails its rules, an unknown kind
    /// name): nothing is written.
    Rejected(String),
}

/// The binding index over one repo's nodes (or every repo's). Built once per
/// graph and reused for every write; it reads each graph's `nodes` in order
/// and never iterates the nav's HashMaps.
#[derive(Debug, Clone, Default)]
pub struct QnameIndex {
    /// qname -> `(id, kind)`, smallest id first.
    by_qname: BTreeMap<String, Vec<(NodeId, NodeKindId)>>,
    /// Every in-scope instance of a node: `(graph index, node index)`, in
    /// graph then node order. One id can sit in several graphs. Looked up by
    /// id only, never iterated (NodeId is Hash, not Ord).
    at: HashMap<NodeId, Vec<(usize, usize)>>,
    /// Hint lookups (LB.6). Built over the whole graph: a re-bind that lands
    /// outside the scope is not taken.
    identity: IdentityIndex,
}

impl QnameIndex {
    /// Index `merged`'s nodes; `repo = Some(r)` keeps only graphs of repo `r`.
    pub fn build(merged: &MergedGraph, repo: Option<RepoId>) -> Self {
        let mut idx = QnameIndex { identity: IdentityIndex::build(merged), ..QnameIndex::default() };
        for (gi, g) in merged.graphs.iter().enumerate() {
            if repo.is_some_and(|r| r != g.repo) {
                continue;
            }
            for (ni, n) in g.nodes.iter().enumerate() {
                let seen = idx.at.contains_key(&n.id);
                idx.at.entry(n.id).or_default().push((gi, ni));
                if seen {
                    continue;
                }
                if let (Some(q), Some(k)) = (g.nav.qname_by_id.get(&n.id), g.nav.kind_by_id.get(&n.id)) {
                    idx.by_qname.entry(q.clone()).or_default().push((n.id, *k));
                }
            }
        }
        idx.by_qname.values_mut().for_each(|v| v.sort_by_key(|(id, _)| id.0));
        idx
    }

    /// Bind a `(qname, kind NAME, hint)` triple. Never writes.
    pub fn resolve(&self, qname: &str, kind: Option<&str>, hint: Option<&str>) -> CellTarget {
        let kind_id = match kind {
            None => None,
            Some(name) => match node_kind::ALL.iter().find(|(_, n)| *n == name) {
                Some((id, _)) => Some(*id),
                None => return CellTarget::Rejected(format!("unknown node kind {name:?}")),
            },
        };
        let exact: Vec<NodeId> = self
            .by_qname
            .get(qname)
            .into_iter()
            .flatten()
            .filter(|(_, k)| kind_id.is_none_or(|want| want == *k))
            .map(|(id, _)| *id)
            .collect();
        if let [only] = exact.as_slice() {
            return CellTarget::Bound(*only);
        }
        if hint.is_some() {
            match self.identity.rebind(qname, kind_id, hint) {
                Rebind::Exact(id) if exact.contains(&id) => return CellTarget::Bound(id),
                Rebind::Moved { id, tier } if exact.is_empty() && self.at.contains_key(&id) => {
                    return CellTarget::Rekeyed { id, tier };
                }
                _ => {}
            }
        }
        match exact.first() {
            Some(first) => CellTarget::Ambiguous(*first),
            None => CellTarget::Orphaned,
        }
    }
}

/// Apply one write to `merged` through `idx` (built over this same graph).
/// `Ok` carries where it bound - only `Bound` and `Rekeyed` wrote anything.
/// An entry is validated, then upserted by `(source, id)` into the node's
/// entry array for its cell type (the cell is replaced in place, else
/// appended); a vector replaces the node's VECTOR cell with the bytes. Every
/// in-scope instance of the node is written. `Err` only when `idx` does not
/// describe `merged` (a node it names is gone).
pub fn apply_cell_write(
    merged: &mut MergedGraph,
    idx: &QnameIndex,
    w: &CellWrite,
) -> Result<CellTarget, String> {
    if !WRITABLE.contains(&w.cell) {
        return Ok(CellTarget::Rejected(format!("cell type {} is not externally writable", cell_name(w.cell))));
    }
    let is_vector_cell = w.cell == cell_type::VECTOR;
    match &w.payload {
        WritePayload::Entry(v) if !is_vector_cell => {
            if let Err(e) = validate_entry(w.cell, v) {
                return Ok(CellTarget::Rejected(e));
            }
        }
        WritePayload::Vector { bytes, dims, .. } if is_vector_cell => {
            if let Err(e) = check_vector(bytes, *dims) {
                return Ok(CellTarget::Rejected(e));
            }
        }
        _ => {
            return Ok(CellTarget::Rejected(format!(
                "cell type {} does not take this payload (VECTOR takes a vector, the others an entry)",
                cell_name(w.cell)
            )));
        }
    }
    let target = idx.resolve(&w.qname, w.kind.as_deref(), w.hint.as_deref());
    let id = match target {
        CellTarget::Bound(id) | CellTarget::Rekeyed { id, .. } => id,
        other => return Ok(other),
    };
    let places = idx.at.get(&id).map_or(&[][..], Vec::as_slice);
    for &(gi, ni) in places {
        let node = merged
            .graphs
            .get_mut(gi)
            .and_then(|g| g.nodes.get_mut(ni))
            .filter(|n| n.id == id)
            .ok_or_else(|| format!("stale QnameIndex: node {} is not at graph {gi} node {ni}", id.0))?;
        let slot = node.cells.iter().position(|c| c.kind == w.cell);
        let payload = match &w.payload {
            WritePayload::Entry(v) => {
                match merge_entry(slot.and_then(|i| node.cells.get(i)).map(|c| &c.payload), v) {
                    Ok(p) => p,
                    Err(e) => return Ok(CellTarget::Rejected(e)),
                }
            }
            WritePayload::Vector { bytes, .. } => CellPayload::Bytes(bytes.clone()),
            // Unreachable: the payload check above rejects any other shape
            // before a node is touched.
            _ => return Ok(CellTarget::Rejected("unsupported payload".into())),
        };
        match slot.and_then(|i| node.cells.get_mut(i)) {
            Some(cell) => cell.payload = payload,
            None => node.cells.push(Cell { kind: w.cell, payload }),
        }
    }
    Ok(target)
}

fn cell_name(c: repo_graph_core::CellTypeId) -> String {
    cell_type::ALL
        .iter()
        .find(|(id, _)| *id == c)
        .map_or_else(|| c.0.to_string(), |(_, n)| (*n).to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use repo_graph_code_domain::{CodeNav, GRAPH_TYPE};
    use repo_graph_core::{Confidence, Node};

    use super::*;
    use crate::test_support::{flow_graph, repo};
    use crate::types::{RepoGraph, SymbolTable};

    fn node(id: NodeId, cells: Vec<Cell>) -> Node {
        Node { id, repo: repo(), confidence: Confidence::Strong, cells }
    }

    /// A CLASS and a SERVICE that share the qname `m::Svc`, plus a FUNCTION.
    fn twins() -> (MergedGraph, NodeId, NodeId) {
        let r = repo();
        let class = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "m::Svc");
        let service = NodeId::from_parts(GRAPH_TYPE, r, node_kind::SERVICE, "m::Svc");
        let mut nav = CodeNav::default();
        nav.record(class, "Svc", "m::Svc", node_kind::CLASS, None);
        nav.record(service, "Svc", "m::Svc", node_kind::SERVICE, None);
        let g = RepoGraph {
            repo: r,
            nodes: vec![node(class, vec![]), node(service, vec![])],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        (MergedGraph::new(vec![g, flow_graph()]), class, service)
    }

    #[test]
    fn resolve_prefers_kind_filter() {
        let (m, class, service) = twins();
        let idx = QnameIndex::build(&m, Some(repo()));
        assert_eq!(idx.resolve("m::Svc", Some("SERVICE"), None), CellTarget::Bound(service));
        assert_eq!(idx.resolve("m::Svc", Some("CLASS"), None), CellTarget::Bound(class));
        assert_eq!(idx.resolve("m::a", None, None), CellTarget::Bound(m.graphs[1].nodes[0].id));
        assert_eq!(idx.resolve("m::gone", None, None), CellTarget::Orphaned);
        assert!(matches!(idx.resolve("m::Svc", Some("NOPE"), None), CellTarget::Rejected(_)));
        // Another repo's scope sees none of them.
        let other = QnameIndex::build(&m, Some(RepoId::from_canonical("test://other")));
        assert_eq!(other.resolve("m::Svc", Some("SERVICE"), None), CellTarget::Orphaned);
    }

    #[test]
    fn ties_take_smallest_node_id() {
        let (mut m, class, service) = twins();
        let idx = QnameIndex::build(&m, None);
        let smaller = if class.0 < service.0 { class } else { service };
        assert_eq!(idx.resolve("m::Svc", None, None), CellTarget::Ambiguous(smaller));
        // An ambiguous write is reported, never applied.
        let w = CellWrite::vector("m::Svc", vec![0; 8], None, Some(2));
        assert_eq!(apply_cell_write(&mut m, &idx, &w), Ok(CellTarget::Ambiguous(smaller)));
        assert!(m.graphs[0].nodes.iter().all(|n| n.cells.is_empty()));
    }

    #[test]
    fn code_cell_is_rejected() {
        let mut m = MergedGraph::new(vec![flow_graph()]);
        let idx = QnameIndex::build(&m, None);
        let w = CellWrite::entry("m::a", cell_type::CODE, Default::default());
        assert!(matches!(apply_cell_write(&mut m, &idx, &w), Ok(CellTarget::Rejected(_))));
        // A vector payload on an entry cell, and an entry on VECTOR, too.
        let mut w = CellWrite::vector("m::a", vec![0; 4], None, None);
        w.cell = cell_type::CONV;
        assert!(matches!(apply_cell_write(&mut m, &idx, &w), Ok(CellTarget::Rejected(_))));
        let w = CellWrite::entry("m::a", cell_type::VECTOR, Default::default());
        assert!(matches!(apply_cell_write(&mut m, &idx, &w), Ok(CellTarget::Rejected(_))));
        // A CONV entry that is not an object fails its rules.
        let w = CellWrite::entry("m::a", cell_type::CONV, Default::default());
        assert!(matches!(apply_cell_write(&mut m, &idx, &w), Ok(CellTarget::Rejected(_))));
        assert!(m.graphs[0].nodes.iter().all(|n| n.cells.is_empty()));
    }

    #[test]
    fn vector_dims_mismatch_is_rejected() {
        let mut m = MergedGraph::new(vec![flow_graph()]);
        let idx = QnameIndex::build(&m, None);
        let bytes = vec![0u8, 0, 128, 63, 0, 0, 0, 64];
        let bad = CellWrite::vector("m::a", bytes.clone(), None, Some(3));
        assert!(matches!(apply_cell_write(&mut m, &idx, &bad), Ok(CellTarget::Rejected(_))));
        assert!(m.graphs[0].nodes[0].cells.is_empty());
        let good = CellWrite::vector("m::a", bytes.clone(), None, Some(2));
        let a = m.graphs[0].nodes[0].id;
        assert_eq!(apply_cell_write(&mut m, &idx, &good), Ok(CellTarget::Bound(a)));
        // A second write replaces the VECTOR cell, never adds one.
        let again = CellWrite::vector("m::a", vec![1; 4], None, Some(1));
        assert_eq!(apply_cell_write(&mut m, &idx, &again), Ok(CellTarget::Bound(a)));
        assert_eq!(
            m.graphs[0].nodes[0].cells,
            vec![Cell { kind: cell_type::VECTOR, payload: CellPayload::Bytes(vec![1; 4]) }]
        );
    }

    #[test]
    fn a_stale_index_errs() {
        let mut m = MergedGraph::new(vec![flow_graph()]);
        let idx = QnameIndex::build(&m, None);
        m.graphs[0].nodes.remove(0);
        let w = CellWrite::vector("m::a", vec![0; 4], None, None);
        assert!(apply_cell_write(&mut m, &idx, &w).is_err());
    }
}
