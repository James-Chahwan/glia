//! The hints side file (T12): each exported fact's LB.6 move-stable identity
//! hint, written as `<out>.hints.jsonl` ([`crate::hints_side_path`]) beside
//! the gmap on every `glia-export-engram` run.
//!
//! **Why.** The gmap's `identity_hint` is the export's own
//! `<file token>:<kind>:<ordinal>` form (the crate's identity story), while
//! glia's cell store keys a CONSTRAINT / DECISION / CONV row by
//! [`Identity::hint`] (`v1|<kind id>|<file basename>|<body:016x>|<local>`).
//! This file gives Engram the v1 form of every fact, so a row it writes for
//! a fact names that fact's glia node and re-binds after a move. It is one
//! hint format between the two, not a GMAP contract change.
//!
//! **Data, never a read path.** No glia code reads the file. Writing it
//! changes nothing in the gmap, the `files.json` / `meta.json` sidecars, the
//! `--since` diff, the PPR side file or `<out>.glia/`.
//!
//! **The rows.** One per gmap node (`facts` is `gmap.nodes.len()`, so a reader
//! can check the file is complete), sorted by key in byte order; keys are
//! unique in an export. `hint` is the v1 string of the fact's glia node
//! ([`crate::build_gmap_with_nodes`] names it), or `null` when the node has
//! no v1 identity (no MODULE ancestor or no POSITION file: ENDPOINTs, ROUTEs,
//! config and data-entity nodes, doc sections, non-code files) and for a
//! NatSpec tag fact, which has no glia node of its own: the symbol's hint
//! under a `<sym>#natspec:..` key would re-key the row onto the symbol.
//! v1 hints are not unique (two `index.ts` modules share a basename), so a
//! hint narrows a qname shared by several nodes and never stands alone.
//!
//! **The format.** JSONL, every object's keys sorted (`BTreeMap`), serde_json
//! compact, non-ASCII literal, each line `\n`-terminated. Line 1 is the header
//! that binds the file to its gmap by
//! [`content_digest`](engram_core::content_digest), the same `{:016x}` the
//! PPR side file and `meta.json`'s `gmap_digest` use:
//! `{"facts":<n>,"gmap_digest":"<16 hex>","hint_version":"v1","hinted":<h>,
//! "hints_side":1}`; a reader drops a file whose `gmap_digest` is not its
//! gmap's. Each further line is `{"hint":"v1|..."|null,"key":"<key>"}`.
//! The bytes are a pure function of the graph and the gmap: an [`Identity`]
//! holds no RepoId (the kind id, the file basename, the nav name chain and
//! FNV-1a of the whitespace-collapsed CODE), the rows are sorted by key, and
//! no map is iterated into them.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use engram_core::Gmap;
use glia_core::NodeId;
use glia_graph::MergedGraph;
use glia_graph::identity::{HINT_VERSION, Identity, identities};

/// The side file's schema version (the header's `hints_side`): bumped when a
/// field is renamed or removed or changes meaning.
pub const HINTS_SIDE_VERSION: u32 = 1;

/// One fact's row: its gmap key and the v1 hint of its glia node, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct HintRow {
    pub key: String,
    pub hint: Option<String>,
}

/// What [`write_hints_side`] wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HintsSideStats {
    /// Rows written: every gmap node.
    pub facts: usize,
    /// Rows whose `hint` is a v1 string, not null.
    pub hinted: usize,
}

/// One row per `gmap` node, sorted by key. `fact_nodes[i]` is
/// `gmap.nodes[i]`'s glia node ([`crate::build_gmap_with_nodes`]); a `None`
/// entry, a missing one, or a node with no identity gives a null hint.
pub fn hint_rows(merged: &MergedGraph, gmap: &Gmap, fact_nodes: &[Option<NodeId>]) -> Vec<HintRow> {
    hint_rows_with(&identities(merged), gmap, fact_nodes)
}

/// [`hint_rows`] over `ids`, the graph's
/// [`identities`](glia_graph::identity::identities) derived beforehand: the
/// bin derives them beside the gmap build, which they do not depend on.
/// `ids` is looked up, never iterated into the rows.
pub fn hint_rows_with(
    ids: &HashMap<NodeId, Identity>,
    gmap: &Gmap,
    fact_nodes: &[Option<NodeId>],
) -> Vec<HintRow> {
    let mut rows: Vec<HintRow> = gmap
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| HintRow {
            key: node.key.clone(),
            hint: fact_nodes.get(i).copied().flatten().and_then(|id| ids.get(&id)).map(Identity::hint),
        })
        .collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    rows
}

/// The side file's bytes for `rows` (of a gmap whose `content_digest` is
/// `digest`).
fn side_bytes(rows: &[HintRow], digest: u64) -> serde_json::Result<(Vec<u8>, HintsSideStats)> {
    let stats = HintsSideStats { facts: rows.len(), hinted: rows.iter().filter(|r| r.hint.is_some()).count() };
    let header: BTreeMap<&str, serde_json::Value> = BTreeMap::from([
        ("facts", stats.facts.into()),
        ("gmap_digest", format!("{digest:016x}").into()),
        ("hint_version", HINT_VERSION.into()),
        ("hinted", stats.hinted.into()),
        ("hints_side", HINTS_SIDE_VERSION.into()),
    ]);
    let mut out = serde_json::to_vec(&header)?;
    out.push(b'\n');
    for row in rows {
        let hint = row.hint.as_deref().map_or(serde_json::Value::Null, serde_json::Value::from);
        let line: BTreeMap<&str, serde_json::Value> =
            BTreeMap::from([("hint", hint), ("key", row.key.as_str().into())]);
        out.extend(serde_json::to_vec(&line)?);
        out.push(b'\n');
    }
    Ok((out, stats))
}

/// Write the side file for `gmap` (whose `content_digest` is `digest`, built
/// from `merged` with `fact_nodes`) to `out`, tmp-then-rename: the header,
/// then one row per fact.
pub fn write_hints_side(
    merged: &MergedGraph,
    gmap: &Gmap,
    fact_nodes: &[Option<NodeId>],
    digest: u64,
    out: &Path,
) -> io::Result<HintsSideStats> {
    write_hints_side_with(&identities(merged), gmap, fact_nodes, digest, out)
}

/// [`write_hints_side`] over identities derived beforehand
/// ([`hint_rows_with`]): the same bytes.
pub fn write_hints_side_with(
    ids: &HashMap<NodeId, Identity>,
    gmap: &Gmap,
    fact_nodes: &[Option<NodeId>],
    digest: u64,
    out: &Path,
) -> io::Result<HintsSideStats> {
    let rows = hint_rows_with(ids, gmap, fact_nodes);
    let (bytes, stats) = side_bytes(&rows, digest)?;
    crate::write_atomic(out, &bytes)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::{Content, GmapNode, SpanRef};
    use glia_code_domain::{CodeNav, cell_type, node_kind};
    use glia_core::{Cell, CellPayload, Confidence, Node, RepoId};
    use glia_graph::RepoGraph;
    use glia_graph::identity::identity_of;

    const MODULE: NodeId = NodeId(1);
    const FUNC: NodeId = NodeId(2);
    const ROUTE: NodeId = NodeId(3);

    /// `app::m` (a MODULE in `src/m.py`) holding `app::m::f`, beside a
    /// `GET /x` ROUTE with no MODULE: the ROUTE has no v1 identity.
    fn merged() -> MergedGraph {
        let repo = RepoId(1);
        let pos = |row: u32| Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(r#"{{"file":"src/m.py","start_line":{row},"end_line":{row}}}"#)),
        };
        let node = |id: NodeId, cells: Vec<Cell>| Node { id, repo, confidence: Confidence::Strong, cells };
        let mut nav = CodeNav::default();
        nav.record(MODULE, "m", "app::m", node_kind::MODULE, None);
        nav.record(FUNC, "f", "app::m::f", node_kind::FUNCTION, Some(MODULE));
        nav.record(ROUTE, "GET /x", "GET /x", node_kind::ROUTE, None);
        let body = "def f(rows):\n    return sum(r.amount for r in rows if r.ok)\n";
        MergedGraph::new(vec![RepoGraph {
            repo,
            nodes: vec![
                node(MODULE, vec![pos(0)]),
                node(FUNC, vec![pos(0), Cell { kind: cell_type::CODE, payload: CellPayload::Text(body.into()) }]),
                node(ROUTE, Vec::new()),
            ],
            edges: Vec::new(),
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        }])
    }

    fn fact(key: &str) -> GmapNode {
        GmapNode {
            key: key.to_string(),
            content: Content::Symbol {
                name: key.to_string(),
                span: SpanRef::NONE,
                qname: Some(key.to_string()),
                doc: None,
                imports: None,
            },
            provenance: None,
            concept_hint: None,
            identity_hint: None,
        }
    }

    /// Three facts out of key order: the function, the ROUTE, and a NatSpec
    /// tag fact (no glia node).
    fn gmap() -> (Gmap, Vec<Option<NodeId>>) {
        let gmap = Gmap {
            format_version: engram_core::GMAP_FORMAT_VERSION,
            nodes: vec![fact("app::m::f"), fact("GET /x"), fact("app::m::f#natspec:notice")],
            edges: Vec::new(),
            files: BTreeMap::new(),
        };
        (gmap, vec![Some(FUNC), Some(ROUTE), None])
    }

    #[test]
    fn header_and_rows_have_the_documented_bytes() {
        let (m, (g, nodes)) = (merged(), gmap());
        let (bytes, stats) = side_bytes(&hint_rows(&m, &g, &nodes), 0xabc).unwrap();
        assert_eq!(stats, HintsSideStats { facts: 3, hinted: 1 });
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.ends_with('\n'));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            r#"{"facts":3,"gmap_digest":"0000000000000abc","hint_version":"v1","hinted":1,"hints_side":1}"#
        );
        let hint = identity_of(&m, FUNC).expect("the function has a MODULE and a file").hint();
        assert!(hint.starts_with(&format!("v1|{}|m.py|", node_kind::FUNCTION.0)), "{hint}");
        assert!(hint.ends_with("|f") && !hint.contains("|0000000000000000|"), "a hashed body: {hint}");
        assert_eq!(
            &lines[1..],
            [
                r#"{"hint":null,"key":"GET /x"}"#.to_string(),
                format!(r#"{{"hint":"{hint}","key":"app::m::f"}}"#),
                r#"{"hint":null,"key":"app::m::f#natspec:notice"}"#.to_string(),
            ]
        );
    }

    #[test]
    fn rows_are_sorted_by_key_whatever_the_gmap_order() {
        let m = merged();
        let (mut g, mut nodes) = gmap();
        let want = hint_rows(&m, &g, &nodes);
        let keys: Vec<&str> = want.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["GET /x", "app::m::f", "app::m::f#natspec:notice"]);
        g.nodes.reverse();
        nodes.reverse();
        assert_eq!(hint_rows(&m, &g, &nodes), want);
    }

    #[test]
    fn a_missing_or_none_node_is_a_null_hint() {
        let m = merged();
        let (g, _) = gmap();
        // Nothing named: every hint is null.
        assert!(hint_rows(&m, &g, &[]).iter().all(|r| r.hint.is_none()));
        // Out of range past the first entry: only the first fact is hinted.
        let rows = hint_rows(&m, &g, &[Some(FUNC)]);
        let hinted: Vec<&str> = rows.iter().filter(|r| r.hint.is_some()).map(|r| r.key.as_str()).collect();
        assert_eq!(hinted, ["app::m::f"]);
        // A node with no identity, or one the graph does not have.
        let rows = hint_rows(&m, &g, &[Some(ROUTE), Some(NodeId(99)), None]);
        assert!(rows.iter().all(|r| r.hint.is_none()), "{rows:?}");
    }

    #[test]
    fn two_calls_give_equal_bytes() {
        let (m, (g, nodes)) = (merged(), gmap());
        let one = side_bytes(&hint_rows(&m, &g, &nodes), 7).unwrap();
        let two = side_bytes(&hint_rows(&m, &g, &nodes), 7).unwrap();
        assert_eq!(one, two);
        let dir = std::env::temp_dir().join(format!("glia-dt12a-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("x.engram-gmap.hints.jsonl");
        let stats = write_hints_side(&m, &g, &nodes, 7, &out).unwrap();
        let written = std::fs::read(&out).unwrap();
        // Identities derived beforehand write the same bytes.
        let with = write_hints_side_with(&identities(&m), &g, &nodes, 7, &out).unwrap();
        let written_with = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((written, stats), one);
        assert_eq!((written_with, with), one);
    }
}
