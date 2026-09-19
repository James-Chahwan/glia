//! The toy-reel graph: [`ToyGraph`], its navigation section [`ReelNav`], the
//! input transformer [`build`], and the `.gmap` round trip
//! ([`ToyGraph::write_gmap`] / [`read_gmap`]).
//!
//! Nodes have no names. A node's identity key is `<kind>/<index>` (hashed
//! into its `NodeId`, never displayed or stored); what a reader needs to tell
//! nodes apart - each node's kind and its index in the reel - lives in
//! [`ReelNav`], which the file carries as the domain section
//! [`NAV_SECTION`]: opaque little-endian rows the store never looks inside.
//! Every name a reader prints (a kind, a category, a cell type) comes from the
//! file's own header registries.

use std::path::Path;

use glia_activation::algo::GraphSource;
use glia_core::{
    Cell, CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
    RepoId,
};
use glia_store::{
    Container, EncodedSection, Header, StoreError, read_to_owned, write_container,
};

use crate::profile::TOY_TABLES;
use crate::registry::cell_type::{LABEL, SCREEN_TIME, TIMECODE};
use crate::registry::edge_category::{CONTAINS_SHOT, FEATURES, NEXT_SHOT};
use crate::registry::node_kind::{OBJECT, SCENE, SHOT};

/// The domain's graph type: the `NodeId` graph-type tag, the header's
/// `graph_type`, and the `domain=` of the `[passes]` marker.
pub const GRAPH_TYPE: &str = "toy-reel";

/// The canonical source every reel [`build`] keys its `RepoId` on.
pub const REEL_REPO: &str = "toy://reel";

/// The name of the domain section holding the encoded [`ReelNav`].
pub const NAV_SECTION: &str = "toy.reel.nav";

/// The identity-key word of a kind, `None` for a kind the domain lacks.
fn kind_key(kind: NodeKindId) -> Option<&'static str> {
    match kind {
        SCENE => Some("scene"),
        SHOT => Some("shot"),
        OBJECT => Some("object"),
        _ => None,
    }
}

/// The id of the `kind` node at `index` in the reel of `repo`: a hash of
/// `(GRAPH_TYPE, repo, kind, "<kind>/<index>")`. `None` for a foreign kind.
pub fn node_id(repo: RepoId, kind: NodeKindId, index: u32) -> Option<NodeId> {
    let key = kind_key(kind)?;
    Some(NodeId::from_parts(
        GRAPH_TYPE,
        repo,
        kind,
        &format!("{key}/{index}"),
    ))
}

// ============================================================================
// ReelNav - the domain's container section
// ============================================================================

/// Bytes per encoded row: `u64` id, `u32` kind, `u32` index.
const ROW_BYTES: usize = 16;

/// The domain's navigation index: every node's kind and reel index, sorted by
/// `NodeId`, looked up by binary search. The core container also carries each
/// node's kind (every domain has kinds); [`read_gmap`] checks the two agree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReelNav {
    kind_by_id: Vec<(NodeId, NodeKindId)>,
    index_by_id: Vec<(NodeId, u32)>,
}

impl ReelNav {
    /// A nav over `(id, kind, index)` rows in any order; an id given twice is
    /// an error.
    pub fn from_rows(mut rows: Vec<(NodeId, NodeKindId, u32)>) -> Result<Self, String> {
        rows.sort_by_key(|(id, _, _)| id.0);
        if let Some(w) = rows.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(format!("node id {:#018x} has two nav rows", w[0].0.0));
        }
        Ok(Self {
            kind_by_id: rows.iter().map(|&(id, kind, _)| (id, kind)).collect(),
            index_by_id: rows.iter().map(|&(id, _, index)| (id, index)).collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.kind_by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kind_by_id.is_empty()
    }

    fn position(&self, id: NodeId) -> Option<usize> {
        self.kind_by_id
            .binary_search_by_key(&id.0, |(i, _)| i.0)
            .ok()
    }

    pub fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.position(id).map(|i| self.kind_by_id[i].1)
    }

    pub fn index(&self, id: NodeId) -> Option<u32> {
        self.position(id).map(|i| self.index_by_id[i].1)
    }

    /// Every node's kind, sorted by id: the core container's `node_kinds`.
    pub fn kinds(&self) -> &[(NodeId, NodeKindId)] {
        &self.kind_by_id
    }

    /// `(id, kind, index)`, sorted by id.
    pub fn rows(&self) -> impl Iterator<Item = (NodeId, NodeKindId, u32)> + '_ {
        self.kind_by_id
            .iter()
            .zip(&self.index_by_id)
            .map(|(&(id, kind), &(_, index))| (id, kind, index))
    }

    /// Little-endian `u32` row count, then one fixed-width row per node in id
    /// order: `u64` id, `u32` kind, `u32` index.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.len() * ROW_BYTES);
        out.extend_from_slice(&(self.len() as u32).to_le_bytes());
        for (id, kind, index) in self.rows() {
            out.extend_from_slice(&id.0.to_le_bytes());
            out.extend_from_slice(&kind.0.to_le_bytes());
            out.extend_from_slice(&index.to_le_bytes());
        }
        out
    }

    /// The inverse of [`Self::encode`]. The length must match the row count
    /// exactly and the ids must be strictly ascending.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let count = bytes
            .get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| format!("{NAV_SECTION}: {} bytes, no row count", bytes.len()))?;
        let want = count.checked_mul(ROW_BYTES).and_then(|n| n.checked_add(4));
        if want != Some(bytes.len()) {
            return Err(format!(
                "{NAV_SECTION}: {count} rows need {want:?} bytes, found {}",
                bytes.len()
            ));
        }
        let mut nav = Self::default();
        for row in bytes[4..].chunks_exact(ROW_BYTES) {
            let mut id = [0u8; 8];
            id.copy_from_slice(&row[..8]);
            let id = NodeId(u64::from_le_bytes(id));
            let kind = NodeKindId(u32::from_le_bytes([row[8], row[9], row[10], row[11]]));
            let index = u32::from_le_bytes([row[12], row[13], row[14], row[15]]);
            if nav
                .kind_by_id
                .last()
                .is_some_and(|(prev, _)| prev.0 >= id.0)
            {
                return Err(format!(
                    "{NAV_SECTION}: row {:#018x} is out of id order",
                    id.0
                ));
            }
            nav.kind_by_id.push((id, kind));
            nav.index_by_id.push((id, index));
        }
        Ok(nav)
    }
}

// ============================================================================
// ToyGraph
// ============================================================================

/// One reel: nodes and edges in the core shape, and the domain's nav.
#[derive(Clone, Debug, PartialEq)]
pub struct ToyGraph {
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: ReelNav,
}

impl GraphSource for ToyGraph {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.iter().map(|n| n.id).collect()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

impl ToyGraph {
    /// The `kind` node at `index`, when the reel has it.
    pub fn id(&self, kind: NodeKindId, index: u32) -> Option<NodeId> {
        node_id(self.repo, kind, index).filter(|id| self.nav.kind(*id) == Some(kind))
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Every `kind` node, in node order.
    pub fn ids_of(&self, kind: NodeKindId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| self.nav.kind(*id) == Some(kind))
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

    /// An object's LABEL.
    pub fn label(&self, id: NodeId) -> Option<&str> {
        match self.cell(id, LABEL)? {
            CellPayload::Text(s) => Some(s),
            _ => None,
        }
    }

    /// A shot's TIMECODE, `(start_ms, end_ms)`.
    pub fn timecode(&self, id: NodeId) -> Option<(u64, u64)> {
        match self.cell(id, TIMECODE)? {
            CellPayload::Json(s) => json_pair(s, "start_ms", "end_ms"),
            _ => None,
        }
    }

    /// An object's SCREEN_TIME, `(ms, shots)`.
    pub fn screen_time(&self, id: NodeId) -> Option<(u64, u64)> {
        match self.cell(id, SCREEN_TIME)? {
            CellPayload::Json(s) => json_pair(s, "ms", "shots"),
            _ => None,
        }
    }

    /// Set `id`'s cell of type `t` to `payload`, replacing one it has.
    pub(crate) fn set_cell(&mut self, id: NodeId, t: CellTypeId, payload: CellPayload) {
        let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        match node.cells.iter_mut().find(|c| c.kind == t) {
            Some(cell) => cell.payload = payload,
            None => node.cells.push(Cell { kind: t, payload }),
        }
    }
}

/// `{"<a>":N,"<b>":M}` - the exact shape this domain writes - as `(N, M)`.
fn json_pair(s: &str, a: &str, b: &str) -> Option<(u64, u64)> {
    let body = s.strip_prefix('{')?.strip_suffix('}')?;
    let (x, y) = body.split_once(',')?;
    let field = |kv: &str, key: &str| -> Option<u64> {
        let (k, v) = kv.split_once(':')?;
        (k.strip_prefix('"')?.strip_suffix('"')? == key).then_some(())?;
        v.parse().ok()
    };
    Some((field(x, a)?, field(y, b)?))
}

// ============================================================================
// Input transformer
// ============================================================================

struct Shot {
    index: u32,
    start_ms: u64,
    scene: Option<u32>,
}

/// Parse a reel description into a [`ToyGraph`] (passes not yet run).
///
/// One record per line; `#` starts a comment:
/// - `scene <index>`
/// - `shot <index> <start_ms> <end_ms> [scene=<scene>]`
/// - `object <index> <label>`
/// - `features <shot> <object>`
///
/// Nodes come in record order, SCENE / SHOT / OBJECT; shots get a TIMECODE
/// cell, objects a LABEL cell. Edges: CONTAINS_SHOT scene -> shot (shot
/// order), NEXT_SHOT between consecutive shots of one scene (by start, then
/// index), FEATURES shot -> object (line order). A repeated record, a
/// reference to an undeclared node, or a shot ending before it starts is an
/// error naming its line.
///
/// fired_on marker (grep token `[toy] reel:`):
///   `[toy] reel: built nodes=<n> edges=<e>`
pub fn build(text: &str) -> Result<ToyGraph, String> {
    let repo = RepoId::from_canonical(REEL_REPO);
    let mut nodes: Vec<Node> = Vec::new();
    let mut rows: Vec<(NodeId, NodeKindId, u32)> = Vec::new();
    let mut scenes: Vec<u32> = Vec::new();
    let mut shots: Vec<Shot> = Vec::new();
    let mut objects: Vec<u32> = Vec::new();
    let mut features: Vec<(usize, u32, u32)> = Vec::new();

    for (n, raw) in text.lines().enumerate() {
        let line_no = n + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |why: &str| format!("reel line {line_no}: {why}: {line:?}");
        let words: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> Result<u64, String> {
            words
                .get(i)
                .and_then(|w| w.parse::<u64>().ok())
                .ok_or_else(|| err("expected a number"))
        };
        let index = |i: usize| -> Result<u32, String> {
            num(i).and_then(|v| u32::try_from(v).map_err(|_| err("index out of range")))
        };
        let (kind, idx, cells) = match words[0] {
            "scene" if words.len() == 2 => {
                let s = index(1)?;
                if scenes.contains(&s) {
                    return Err(err("scene declared twice"));
                }
                scenes.push(s);
                (SCENE, s, Vec::new())
            }
            "shot" if words.len() == 4 || words.len() == 5 => {
                let (i, start_ms, end_ms) = (index(1)?, num(2)?, num(3)?);
                if end_ms < start_ms {
                    return Err(err("shot ends before it starts"));
                }
                let scene = match words.get(4) {
                    None => None,
                    Some(w) => {
                        let s = w.strip_prefix("scene=").and_then(|s| s.parse::<u32>().ok());
                        Some(s.ok_or_else(|| err("expected scene=<index>"))?)
                    }
                };
                if shots.iter().any(|s| s.index == i) {
                    return Err(err("shot declared twice"));
                }
                shots.push(Shot {
                    index: i,
                    start_ms,
                    scene,
                });
                let timecode = format!("{{\"start_ms\":{start_ms},\"end_ms\":{end_ms}}}");
                (
                    SHOT,
                    i,
                    vec![Cell {
                        kind: TIMECODE,
                        payload: CellPayload::Json(timecode),
                    }],
                )
            }
            "object" if words.len() == 3 => {
                let o = index(1)?;
                if objects.contains(&o) {
                    return Err(err("object declared twice"));
                }
                objects.push(o);
                (
                    OBJECT,
                    o,
                    vec![Cell {
                        kind: LABEL,
                        payload: CellPayload::Text(words[2].to_string()),
                    }],
                )
            }
            "features" if words.len() == 3 => {
                features.push((line_no, index(1)?, index(2)?));
                continue;
            }
            _ => return Err(err("unknown record")),
        };
        let id = node_id(repo, kind, idx).ok_or_else(|| err("unknown kind"))?;
        rows.push((id, kind, idx));
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        });
    }

    // Only the domain's own kinds reach here, for which `node_id` is `Some`.
    let id = |kind: NodeKindId, index: u32| node_id(repo, kind, index).unwrap_or(NodeId(0));
    let edge =
        |from: NodeId, to: NodeId, c: EdgeCategoryId| Edge::new(from, to, c, Confidence::Strong);
    let mut edges: Vec<Edge> = Vec::new();
    for shot in &shots {
        if let Some(scene) = shot.scene {
            if !scenes.contains(&scene) {
                return Err(format!(
                    "reel: shot {} names undeclared scene {scene}",
                    shot.index
                ));
            }
            edges.push(edge(id(SCENE, scene), id(SHOT, shot.index), CONTAINS_SHOT));
        }
    }
    for &scene in &scenes {
        let mut own: Vec<&Shot> = shots.iter().filter(|s| s.scene == Some(scene)).collect();
        own.sort_by_key(|s| (s.start_ms, s.index));
        for pair in own.windows(2) {
            edges.push(edge(
                id(SHOT, pair[0].index),
                id(SHOT, pair[1].index),
                NEXT_SHOT,
            ));
        }
    }
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for &(line_no, shot, object) in &features {
        if !shots.iter().any(|s| s.index == shot) || !objects.contains(&object) {
            return Err(format!(
                "reel line {line_no}: features {shot} {object} names an undeclared node"
            ));
        }
        if seen.contains(&(shot, object)) {
            return Err(format!(
                "reel line {line_no}: features {shot} {object} declared twice"
            ));
        }
        seen.push((shot, object));
        edges.push(edge(id(SHOT, shot), id(OBJECT, object), FEATURES));
    }

    let nav = ReelNav::from_rows(rows)?;
    eprintln!(
        "[toy] reel: built nodes={} edges={}",
        nodes.len(),
        edges.len()
    );
    Ok(ToyGraph {
        repo,
        nodes,
        edges,
        nav,
    })
}

// ============================================================================
// .gmap round trip
// ============================================================================

/// `(id, name)` rows as the header constructor takes them.
fn raw<T: Copy>(table: &[(T, &'static str)], id: impl Fn(T) -> u32) -> Vec<(u32, &'static str)> {
    table.iter().map(|&(t, name)| (id(t), name)).collect()
}

impl ToyGraph {
    /// The file header: `graph_type` and the three registries from
    /// [`TOY_TABLES`], so the file names its own ids.
    pub fn header() -> Result<Header, StoreError> {
        let r = TOY_TABLES.registries;
        Header::for_domain(
            TOY_TABLES.graph_type,
            &raw(r.node_kinds, |k| k.0),
            &raw(r.edge_categories, |c| c.0),
            &raw(r.cell_types, |t| t.0),
        )
    }

    /// The core container (header, nodes, edges, every node's kind) and the
    /// domain's one section, the encoded [`ReelNav`] as opaque bytes.
    pub fn to_container(&self) -> Result<(Container, Vec<EncodedSection>), StoreError> {
        let core = Container {
            header: Self::header()?,
            repo: self.repo,
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            node_kinds: self.nav.kinds().to_vec(),
            sections: Vec::new(),
        };
        let mut nav = EncodedSection {
            name: NAV_SECTION.to_string(),
            bytes: Default::default(),
        };
        nav.bytes.extend_from_slice(&self.nav.encode());
        Ok((core, vec![nav]))
    }

    /// Write the reel to `path` (atomically, through the store); the file's
    /// byte length.
    ///
    /// fired_on marker: `[toy] reel: wrote sections=<s> bytes=<file bytes>`
    pub fn write_gmap(&self, path: &Path) -> Result<u64, StoreError> {
        let (mut core, sections) = self.to_container()?;
        write_container(path, &mut core, &sections)?;
        let bytes = std::fs::metadata(path)?.len();
        eprintln!(
            "[toy] reel: wrote sections={} bytes={bytes}",
            core.sections.len()
        );
        Ok(bytes)
    }
}

/// A reel read back from a `.gmap`: the graph and the file's own header.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadBack {
    pub graph: ToyGraph,
    pub header: Header,
}

impl ReadBack {
    /// `id`'s kind name, from the decoded nav and the file's header.
    pub fn kind_name(&self, id: NodeId) -> Option<&str> {
        let kind = self.graph.nav.kind(id)?;
        named(&self.header.node_kind_registry, kind.0)
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

/// Read a reel back from `path`: the core, the [`NAV_SECTION`] decoded, and
/// the header kept for naming. Refuses a file of another graph type, one
/// without the section, one whose core kinds disagree with the section, and
/// one whose header does not name every kind, category and cell type it
/// uses - a reader here never falls back to a compiled-in table.
///
/// fired_on marker: `[toy] reel: read graph_type=<t> kinds=<distinct kinds named>`
pub fn read_gmap(path: &Path) -> Result<ReadBack, String> {
    let file = read_to_owned(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let core = file.core;
    if core.header.graph_type != GRAPH_TYPE {
        return Err(format!(
            "{}: graph_type {:?}, not {GRAPH_TYPE:?}",
            path.display(),
            core.header.graph_type
        ));
    }
    let section = file
        .sections
        .iter()
        .find(|s| s.name == NAV_SECTION)
        .ok_or_else(|| format!("{}: no {NAV_SECTION} section", path.display()))?;
    let nav = ReelNav::decode(section.bytes.as_slice())?;
    if core.node_kinds != nav.kinds() {
        return Err(format!(
            "{}: core node_kinds disagree with {NAV_SECTION}",
            path.display()
        ));
    }
    let back = ReadBack {
        graph: ToyGraph {
            repo: core.repo,
            nodes: core.nodes,
            edges: core.edges,
            nav,
        },
        header: core.header,
    };
    let mut kinds: Vec<&str> = Vec::new();
    for n in &back.graph.nodes {
        let name = back
            .kind_name(n.id)
            .ok_or_else(|| format!("node {:#018x}: kind not named by the file", n.id.0))?;
        if !kinds.contains(&name) {
            kinds.push(name);
        }
        if let Some(c) = n.cells.iter().find(|c| back.cell_name(c.kind).is_none()) {
            return Err(format!("cell type {} is not named by the file", c.kind.0));
        }
    }
    if let Some(e) = back
        .graph
        .edges
        .iter()
        .find(|e| back.category_name(e.category).is_none())
    {
        return Err(format!(
            "edge category {} is not named by the file",
            e.category.0
        ));
    }
    eprintln!(
        "[toy] reel: read graph_type={} kinds={}",
        back.header.graph_type,
        kinds.len()
    );
    Ok(back)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_round_trips_and_rejects_bad_bytes() {
        let rows = vec![
            (NodeId(9), SHOT, 4),
            (NodeId(3), OBJECT, 1),
            (NodeId(u64::MAX), SCENE, 0),
        ];
        let nav = ReelNav::from_rows(rows).unwrap();
        assert_eq!(nav.kind(NodeId(3)), Some(OBJECT));
        assert_eq!(nav.index(NodeId(9)), Some(4));
        assert_eq!(nav.kind(NodeId(4)), None);
        let bytes = nav.encode();
        assert_eq!(bytes.len(), 4 + 3 * ROW_BYTES);
        assert_eq!(bytes[..4], 3u32.to_le_bytes());
        assert_eq!(bytes[4..12], 3u64.to_le_bytes(), "rows in id order");
        assert_eq!(ReelNav::decode(&bytes), Ok(nav.clone()));
        assert_eq!(
            ReelNav::decode(&ReelNav::default().encode()),
            Ok(ReelNav::default())
        );

        assert!(
            ReelNav::decode(&bytes[..bytes.len() - 1])
                .unwrap_err()
                .contains("need")
        );
        assert!(
            ReelNav::decode(&[1, 0])
                .unwrap_err()
                .contains("no row count")
        );
        let mut swapped = bytes[..4].to_vec();
        swapped.extend_from_slice(&bytes[4 + ROW_BYTES..4 + 2 * ROW_BYTES]);
        swapped.extend_from_slice(&bytes[4..4 + ROW_BYTES]);
        swapped.extend_from_slice(&bytes[4 + 2 * ROW_BYTES..]);
        assert!(
            ReelNav::decode(&swapped)
                .unwrap_err()
                .contains("out of id order")
        );
        assert!(ReelNav::from_rows(vec![(NodeId(1), SHOT, 0), (NodeId(1), OBJECT, 0)]).is_err());
    }

    #[test]
    fn build_rejects_malformed_reels() {
        for (text, why) in [
            ("scene 0\nscene 0", "declared twice"),
            ("shot 0 10 5", "ends before it starts"),
            ("shot 0 0 5 scene=3", "undeclared scene 3"),
            ("shot 0 0 5 act=1", "expected scene="),
            ("object 0 cat\nfeatures 0 0", "undeclared node"),
            (
                "shot 0 0 5\nobject 0 cat\nfeatures 0 0\nfeatures 0 0",
                "declared twice",
            ),
            ("frame 0", "unknown record"),
            ("object x cat", "expected a number"),
        ] {
            let e = build(text).unwrap_err();
            assert!(e.contains(why), "{text:?}: {e}");
        }
        let g = build("# only a comment\n\nscene 7 # trailing\n").unwrap();
        assert_eq!((g.nodes.len(), g.edges.len()), (1, 0));
        assert_eq!(g.id(SCENE, 7), g.nodes.first().map(|n| n.id));
        assert_eq!(g.id(SHOT, 7), None);
    }

    #[test]
    fn json_pair_reads_only_its_own_shape() {
        assert_eq!(
            json_pair("{\"ms\":3400,\"shots\":2}", "ms", "shots"),
            Some((3400, 2))
        );
        assert_eq!(json_pair("{\"shots\":2,\"ms\":3400}", "ms", "shots"), None);
        assert_eq!(json_pair("{\"ms\":-1,\"shots\":2}", "ms", "shots"), None);
        assert_eq!(json_pair("\"ms\":1,\"shots\":2", "ms", "shots"), None);
    }
}
