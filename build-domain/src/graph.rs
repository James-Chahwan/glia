//! The build-target graph: [`BuildGraph`], its node ids ([`node_id`]), and
//! the `targets.gmap` round trip ([`BuildGraph::write_gmap`] /
//! [`read_gmap`]).
//!
//! A node is addressed by its workspace and its label, `"<workspace>|<label>"`
//! hashed into its `NodeId` with the graph type `build-targets`: two
//! workspaces' `//fortune:fortune` are two nodes, and no build node id ever
//! equals a code one. The file carries the core container (header, nodes,
//! edges, each node's kind) and one named section, [`LABELS_SECTION`]: every
//! node's kind, workspace and label, plus the graph's string metadata (a
//! reader's freshness keys), as JSON the store never looks inside. Every name
//! a reader prints (a kind, a category, a cell type) comes from the file's own
//! header registries.

use std::collections::BTreeMap;
use std::path::Path;

use glia_activation::algo::GraphSource;
use glia_core::{
    Cell, CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
    RepoId,
};
use glia_store::{Container, EncodedSection, Header, read_to_owned, write_container};
use serde::{Deserialize, Serialize};

use crate::profile::BUILD_TABLES;
use crate::registry::cell_type::{ROLE, VIA};
use crate::registry::node_kind::TARGET;

/// The domain's graph type: the `NodeId` graph-type tag, the header's
/// `graph_type`, and the `domain=` of the `[passes]` marker.
pub const GRAPH_TYPE: &str = "build-targets";

/// The name of the section holding every node's kind, workspace and label.
pub const LABELS_SECTION: &str = "build-targets.labels";

/// The [`LABELS_SECTION`] encoding this crate writes and reads.
const LABELS_VERSION: u32 = 1;

/// The identity key of a node: `"<workspace>|<label>"`, the workspace `""`
/// for the repo root. A workspace is a repo-relative directory and never
/// holds `|`; a label may (Bazel allows it in a target name), so a key splits
/// at its first `|`.
pub fn node_key(workspace: &str, label: &str) -> String {
    format!("{workspace}|{label}")
}

/// The id of the `kind` node `label` of `workspace` in `repo`:
/// `NodeId::from_parts("build-targets", repo, kind, "<workspace>|<label>")`.
pub fn node_id(repo: RepoId, kind: NodeKindId, workspace: &str, label: &str) -> NodeId {
    NodeId::from_parts(GRAPH_TYPE, repo, kind, &node_key(workspace, label))
}

/// What the graph knows a node by: its kind, workspace and label.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeLabel {
    pub kind: NodeKindId,
    pub workspace: String,
    pub label: String,
}

/// One build-target graph: nodes and edges in the core shape, every node's
/// [`NodeLabel`] keyed by its `NodeId`'s value (the id types are `Hash`, not
/// `Ord`), and string metadata a writer keys freshness on (empty unless a
/// caller sets it). Nodes and edges keep the order they were added in until
/// the passes sort the edges; node lookups are linear in the node count.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct BuildGraph {
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub labels: BTreeMap<u64, NodeLabel>,
    pub meta: BTreeMap<String, String>,
}

impl GraphSource for BuildGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|n| n.id).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

impl BuildGraph {
    /// An empty graph whose node ids hash `repo`.
    pub fn new(repo: RepoId) -> Self {
        Self {
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            labels: BTreeMap::new(),
            meta: BTreeMap::new(),
        }
    }

    /// Add the `kind` node `label` of `workspace` with `cells`, and return
    /// its id. A node already present (same kind, workspace and label) is
    /// kept as it is, `cells` dropped: the first add wins.
    pub fn add_node(
        &mut self,
        kind: NodeKindId,
        workspace: &str,
        label: &str,
        cells: Vec<Cell>,
    ) -> NodeId {
        let id = node_id(self.repo, kind, workspace, label);
        if self.labels.contains_key(&id.0) {
            return id;
        }
        self.labels.insert(
            id.0,
            NodeLabel {
                kind,
                workspace: workspace.to_string(),
                label: label.to_string(),
            },
        );
        self.nodes.push(Node {
            id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells,
        });
        id
    }

    /// Add one `category` edge `from -> to` whose VIA cell is `via` (the
    /// attribute names that declared it, sorted and deduplicated here; no
    /// VIA cell when empty). One call per (from, to, category): a caller
    /// gathers every attribute naming a pair first (`deps` and `embed` on one
    /// pair are one edge, VIA `deps,embed`).
    pub fn add_edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId, via: &[&str]) {
        let mut names: Vec<&str> = via.to_vec();
        names.sort_unstable();
        names.dedup();
        let mut edge = Edge::new(from, to, category, Confidence::Strong);
        if !names.is_empty() {
            edge.cells.push(Cell {
                kind: VIA,
                payload: CellPayload::Text(names.join(",")),
            });
        }
        self.edges.push(edge);
    }

    /// The `kind` node `label` of `workspace`, when the graph has it.
    pub fn id(&self, kind: NodeKindId, workspace: &str, label: &str) -> Option<NodeId> {
        let id = node_id(self.repo, kind, workspace, label);
        self.labels.contains_key(&id.0).then_some(id)
    }

    /// `id`'s kind, workspace and label.
    pub fn node_label(&self, id: NodeId) -> Option<&NodeLabel> {
        self.labels.get(&id.0)
    }

    pub fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.node_label(id).map(|l| l.kind)
    }

    pub fn workspace(&self, id: NodeId) -> Option<&str> {
        self.node_label(id).map(|l| l.workspace.as_str())
    }

    pub fn label(&self, id: NodeId) -> Option<&str> {
        self.node_label(id).map(|l| l.label.as_str())
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Every `kind` node, in node order.
    pub fn ids_of(&self, kind: NodeKindId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| self.kind(*id) == Some(kind))
            .collect()
    }

    /// The payload of `id`'s cell of type `t`.
    pub fn cell(&self, id: NodeId, t: CellTypeId) -> Option<&CellPayload> {
        self.node(id)?
            .cells
            .iter()
            .find(|c| c.kind == t)
            .map(|c| &c.payload)
    }

    /// A target's ROLE (`test`, `binary`, `library`, `alias`), once the
    /// passes ran.
    pub fn role(&self, id: NodeId) -> Option<&str> {
        match self.cell(id, ROLE)? {
            CellPayload::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Is `id` an entry of [`BUILD_TABLES`]: a TARGET whose ROLE is `binary`
    /// or `test` (the entry rule's name is the ROLE)?
    pub fn is_entry(&self, id: NodeId) -> bool {
        let role = self.role(id).unwrap_or("");
        BUILD_TABLES.entry.is_entry(self.kind(id), role, &[])
    }

    /// An edge's VIA attribute names, in their stored (sorted) order.
    pub fn via(edge: &Edge) -> Vec<&str> {
        edge.cells
            .iter()
            .find(|c| c.kind == VIA)
            .and_then(|c| match &c.payload {
                CellPayload::Text(s) => Some(s.split(',').collect()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Every TARGET, in node order.
    pub fn targets(&self) -> Vec<NodeId> {
        self.ids_of(TARGET)
    }
}

// ============================================================================
// targets.gmap round trip
// ============================================================================

/// The [`LABELS_SECTION`] payload: the encoding version, the metadata, and
/// one row per node in id order.
#[derive(Serialize, Deserialize)]
struct LabelsSection {
    version: u32,
    meta: BTreeMap<String, String>,
    nodes: Vec<LabelRow>,
}

#[derive(Serialize, Deserialize)]
struct LabelRow {
    id: u64,
    kind: u32,
    workspace: String,
    label: String,
}

/// `(id, name)` rows as the header constructor takes them.
fn raw<T: Copy>(table: &[(T, &'static str)], id: impl Fn(T) -> u32) -> Vec<(u32, &'static str)> {
    table.iter().map(|&(t, name)| (id(t), name)).collect()
}

impl BuildGraph {
    /// The file header: `graph_type` and the three registries from
    /// [`BUILD_TABLES`], so the file names its own ids.
    pub fn header() -> Result<Header, String> {
        let r = BUILD_TABLES.registries;
        Header::for_domain(
            BUILD_TABLES.graph_type,
            &raw(r.node_kinds, |k| k.0),
            &raw(r.edge_categories, |c| c.0),
            &raw(r.cell_types, |t| t.0),
        )
        .map_err(|e| e.to_string())
    }

    /// The core container (header, nodes, edges, every node's kind) and the
    /// domain's one section, [`LABELS_SECTION`].
    pub fn to_container(&self) -> Result<(Container, Vec<EncodedSection>), String> {
        let section = LabelsSection {
            version: LABELS_VERSION,
            meta: self.meta.clone(),
            nodes: self
                .labels
                .iter()
                .map(|(id, l)| LabelRow {
                    id: *id,
                    kind: l.kind.0,
                    workspace: l.workspace.clone(),
                    label: l.label.clone(),
                })
                .collect(),
        };
        let json = serde_json::to_vec(&section).map_err(|e| format!("{LABELS_SECTION}: {e}"))?;
        let core = Container {
            header: Self::header()?,
            repo: self.repo,
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            node_kinds: self
                .labels
                .iter()
                .map(|(id, l)| (NodeId(*id), l.kind))
                .collect(),
            sections: Vec::new(),
        };
        let mut labels = EncodedSection {
            name: LABELS_SECTION.to_string(),
            bytes: Default::default(),
        };
        labels.bytes.extend_from_slice(&json);
        Ok((core, vec![labels]))
    }

    /// Write the graph to `path` (atomically, through the store); the file's
    /// byte length.
    ///
    /// fired_on marker: `[build-targets] gmap wrote nodes=<n> edges=<e> bytes=<b>`
    pub fn write_gmap(&self, path: &Path) -> Result<u64, String> {
        let (mut core, sections) = self.to_container()?;
        write_container(path, &mut core, &sections)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = std::fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        eprintln!(
            "[build-targets] gmap wrote nodes={} edges={} bytes={bytes}",
            self.nodes.len(),
            self.edges.len()
        );
        Ok(bytes)
    }
}

/// A graph read back from a `targets.gmap`: the graph and the file's own
/// header, which names its ids.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct ReadBack {
    pub graph: BuildGraph,
    pub header: Header,
}

impl ReadBack {
    /// `id`'s kind name, from the file's header.
    pub fn kind_name(&self, id: NodeId) -> Option<&str> {
        named(&self.header.node_kind_registry, self.graph.kind(id)?.0)
    }

    /// A category's name, from the file's header.
    pub fn category_name(&self, c: EdgeCategoryId) -> Option<&str> {
        named(&self.header.edge_category_registry, c.0)
    }

    /// A cell type's name, from the file's header.
    pub fn cell_name(&self, t: CellTypeId) -> Option<&str> {
        named(&self.header.cell_registry, t.0)
    }
}

fn named(registry: &[glia_store::RegistryEntry], id: u32) -> Option<&str> {
    registry
        .iter()
        .find(|e| e.id == id)
        .map(|e| e.name.as_str())
}

/// Read a build-target graph back from `path`. Refuses a file of another
/// graph type, one without [`LABELS_SECTION`] or with another encoding
/// version, one whose core kinds disagree with the section's, a node with no
/// label row, and one whose header does not name every kind, category and
/// cell type it uses: a reader here never falls back to a compiled-in table.
///
/// fired_on marker: `[build-targets] gmap read nodes=<n> edges=<e>`
pub fn read_gmap(path: &Path) -> Result<ReadBack, String> {
    let at = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let file = read_to_owned(path).map_err(|e| at(&e))?;
    let core = file.core;
    if core.header.graph_type != GRAPH_TYPE {
        return Err(at(&format!(
            "graph_type {:?}, not {GRAPH_TYPE:?}",
            core.header.graph_type
        )));
    }
    let section = file
        .sections
        .iter()
        .find(|s| s.name == LABELS_SECTION)
        .ok_or_else(|| at(&format!("no {LABELS_SECTION} section")))?;
    let decoded: LabelsSection = serde_json::from_slice(section.bytes.as_slice())
        .map_err(|e| at(&format!("{LABELS_SECTION}: {e}")))?;
    if decoded.version != LABELS_VERSION {
        return Err(at(&format!(
            "{LABELS_SECTION} version {}, this glia reads {LABELS_VERSION}",
            decoded.version
        )));
    }
    let mut labels = BTreeMap::new();
    for row in decoded.nodes {
        let label = NodeLabel {
            kind: NodeKindId(row.kind),
            workspace: row.workspace,
            label: row.label,
        };
        if labels.insert(row.id, label).is_some() {
            return Err(at(&format!(
                "{LABELS_SECTION}: node {:#018x} has two rows",
                row.id
            )));
        }
    }
    let section_kinds: Vec<(NodeId, NodeKindId)> =
        labels.iter().map(|(id, l)| (NodeId(*id), l.kind)).collect();
    if core.node_kinds != section_kinds {
        return Err(at(&format!(
            "core node_kinds disagree with {LABELS_SECTION}"
        )));
    }
    let back = ReadBack {
        graph: BuildGraph {
            repo: core.repo,
            nodes: core.nodes,
            edges: core.edges,
            labels,
            meta: decoded.meta,
        },
        header: core.header,
    };
    for n in &back.graph.nodes {
        if back.kind_name(n.id).is_none() {
            return Err(at(&format!(
                "node {:#018x}: no label row, or kind not named by the file",
                n.id.0
            )));
        }
        if let Some(c) = n.cells.iter().find(|c| back.cell_name(c.kind).is_none()) {
            return Err(at(&format!(
                "cell type {:#010x} is not named by the file",
                c.kind.0
            )));
        }
    }
    for e in &back.graph.edges {
        if back.category_name(e.category).is_none() {
            return Err(at(&format!(
                "edge category {:#010x} is not named by the file",
                e.category.0
            )));
        }
        if let Some(c) = e.cells.iter().find(|c| back.cell_name(c.kind).is_none()) {
            return Err(at(&format!(
                "cell type {:#010x} is not named by the file",
                c.kind.0
            )));
        }
    }
    eprintln!(
        "[build-targets] gmap read nodes={} edges={}",
        back.graph.nodes.len(),
        back.graph.edges.len()
    );
    Ok(back)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::node_kind::{EXTERNAL, SOURCE};

    #[test]
    fn ids_carry_the_workspace_and_the_graph_type() {
        let repo = RepoId::from_canonical("build://test");
        let a = node_id(repo, TARGET, "cpp", "//fortune:fortune");
        let b = node_id(repo, TARGET, "go", "//fortune:fortune");
        assert_ne!(a, b, "two workspaces' labels never collide");
        assert_ne!(a, node_id(repo, SOURCE, "cpp", "//fortune:fortune"));
        assert_ne!(
            a,
            NodeId::from_parts("code", repo, TARGET, &node_key("cpp", "//fortune:fortune")),
            "the graph type is hashed in"
        );
        assert_eq!(node_key("", "//:x"), "|//:x");
    }

    #[test]
    fn add_node_is_first_wins_and_via_sorts() {
        let mut g = BuildGraph::new(RepoId::from_canonical("build://test"));
        let t = g.add_node(TARGET, "", "//a:a", Vec::new());
        let again = g.add_node(
            TARGET,
            "",
            "//a:a",
            vec![Cell {
                kind: ROLE,
                payload: CellPayload::Text("x".into()),
            }],
        );
        assert_eq!(t, again);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.nodes[0].cells.is_empty());
        let x = g.add_node(EXTERNAL, "", "@@rules_cc+//cc:x", Vec::new());
        g.add_edge(
            t,
            x,
            crate::edge_category::DEPENDS_ON,
            &["embed", "deps", "deps"],
        );
        g.add_edge(t, x, crate::edge_category::RUNTIME_DEP, &[]);
        assert_eq!(BuildGraph::via(&g.edges[0]), ["deps", "embed"]);
        assert!(BuildGraph::via(&g.edges[1]).is_empty());
        assert_eq!(g.id(EXTERNAL, "", "@@rules_cc+//cc:x"), Some(x));
        assert_eq!(g.id(TARGET, "", "@@rules_cc+//cc:x"), None);
        assert_eq!(g.label(x), Some("@@rules_cc+//cc:x"));
        assert_eq!(g.workspace(x), Some(""));
        assert_eq!(g.targets(), [t]);
        assert!(!g.is_entry(t), "no ROLE yet");
    }
}
