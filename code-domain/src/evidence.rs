//! Edge evidence: the EVIDENCE edge-cell payload
//! ([`crate::cell_type::EVIDENCE`]) and its attach / of / locate helpers.
//!
//! Every edge carries at most ONE EVIDENCE cell, a JSON object in this field
//! order (absent optional fields are skipped):
//!
//! ```text
//! {"emitter":"graph:calls","rule":"import_binding","file":"b.py","line":5,"basis":"site"}
//! ```
//!
//! - `emitter` — `<stage>:<name>`, the stage and the component inside it that
//!   put the edge in the graph. The stage vocabulary is closed ([`STAGES`]):
//!   - `parser`    — a language parser's own `FileParse.edges`
//!     (`parser:<lang tag>`);
//!   - `extractor` — a cross-cutting extractor run on a parsed file
//!     (`extractor:queues`, `extractor:anchor`, ...), a synthetic parse
//!     (`extractor:<lang key>`: yaml, proto, graphql, ...) or a post-cache
//!     graft (`extractor:rpc_needles`);
//!   - `graph`     — per-repo resolution in the graph crate, one emitter per
//!     mechanism (LC.3d): `graph:calls`, `graph:refs`, `graph:imports`,
//!     `graph:iface`, `graph:rust_paths`, `graph:go_packages`, `graph:nav`;
//!     `graph:build` stamps whatever edge the build added without naming one;
//!   - `resolver`  — a cross-graph resolver (`resolver:http`, ...);
//!   - `pass`      — an engine post-pass (`pass:doclink`, `pass:tests`, ...);
//!   - `docs`      — doc ingestion (`docs:<source tag>`);
//!   - `overlay` / `history` — the LF stages (user overlay, git history).
//! - `rule` — optional sub-path of the emitter: which resolution branch or
//!   tier asserted the edge.
//! - `file` — repo-relative, `/` separators.
//! - `line` — 0-based, the POSITION `start_line` convention; the record
//!   boundary (LD.1) converts to an editor line.
//! - `basis` — how `file` / `line` were obtained ([`Basis`]).
//!
//! A CALLS / IMPORTS edge, and an edge resolved from an `UnresolvedRef`, is a
//! `site`: its line is the call / import / reference row the parser recorded
//! on the `CallSite` / `ImportStmt` / `UnresolvedRef` (LC.3b), and so is a
//! CALLS edge a parser resolves inside its own file (`parser:<lang>`, rule
//! `intra_file`).
//!
//! Emitters that know no location stamp the emitter alone; the engine's fill
//! pass (`passes::fill_evidence_sites`) then completes `file` / `line` from the
//! edge's endpoints through [`Evidence::fill`], never overwriting a location an
//! emitter recorded.

use repo_graph_core::{Cell, CellPayload, Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId};

use crate::{cell_type, edge_category, endpoint, node_kind};

/// The closed stage vocabulary of an [`Evidence::emitter`] (the part before
/// the first `:`). A new stage is added here, never invented at a call site.
pub const STAGES: &[&str] = &[
    "parser",
    "extractor",
    "graph",
    "resolver",
    "pass",
    "docs",
    // LF: the `.glia/overlay.toml` edges (LF.2b) and git-history edges (LF.5b).
    "overlay",
    "history",
];

/// How an [`Evidence`]'s location was obtained: the FACT / DERIVED tier the
/// answer surface shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// The emitter recorded the exact location of the asserting construct.
    Site,
    /// The declaration of the edge's `from` node: exact for a structural
    /// edge, the enclosing declaration otherwise.
    FromNode,
    /// The declaration of the edge's `to` node, same caveat.
    ToNode,
    /// The file is known, the line is not.
    File,
    /// No location.
    #[default]
    None,
}

/// A `(repo-relative file, 0-based line)` location, as [`locate`] returns it.
pub type Location = (String, Option<u32>);

/// Why an edge exists: who emitted it, by which rule, and where.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Evidence {
    pub emitter: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default)]
    pub basis: Basis,
}

impl Evidence {
    /// Evidence naming only its emitter (`<stage>:<name>`), location unknown
    /// (basis `none` until the fill pass places it).
    pub fn emitter(e: impl Into<String>) -> Self {
        Self {
            emitter: e.into(),
            rule: None,
            file: None,
            line: None,
            basis: Basis::None,
        }
    }

    /// `self` with its `rule` set.
    pub fn rule(mut self, r: impl Into<String>) -> Self {
        self.rule = Some(r.into());
        self
    }

    /// `self` located at the asserting construct: `file` and 0-based `line`,
    /// basis `site`.
    pub fn at(mut self, file: impl Into<String>, line: u32) -> Self {
        self.file = Some(file.into());
        self.line = Some(line);
        self.basis = Basis::Site;
        self
    }

    /// `self` at 0-based `line` of a file the emitter does not name, basis
    /// `site`: the fill pass takes the file from the edge's `from` node.
    pub fn line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self.basis = Basis::Site;
        self
    }

    /// The EVIDENCE cell carrying `self` as JSON (struct field order, absent
    /// optional fields skipped).
    pub fn to_cell(&self) -> Cell {
        // Serialising plain strings, an integer and a unit enum cannot fail;
        // an empty object would read back as None rather than lie.
        let json = serde_json::to_string(self).unwrap_or_else(|_| String::from("{}"));
        Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Json(json),
        }
    }

    /// The evidence in the first EVIDENCE cell of `cells`. `None` when there
    /// is none, or when its payload is not an evidence object (never panics).
    pub fn read(cells: &[Cell]) -> Option<Evidence> {
        let c = cells.iter().find(|c| c.kind == cell_type::EVIDENCE)?;
        let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
            return None;
        };
        serde_json::from_str(s).ok()
    }

    /// The evidence an edge carries, if any.
    pub fn of(e: &Edge) -> Option<Evidence> {
        Self::read(&e.cells)
    }

    /// Complete a file-less evidence from its edge's endpoint locations.
    /// Never overwrites a `file` or `line` already recorded, so a second run
    /// changes nothing (LC.10b re-runs the passes over loaded graphs).
    ///
    /// - `file` already known: untouched.
    /// - a site line with no file ([`Evidence::line`]): the file is the `from`
    ///   node's, the basis stays `site`; with no `from` location it stays
    ///   file-less.
    /// - otherwise the emitter's own `from_node` / `to_node` choice, else
    ///   [`default_basis`], picks the endpoint whose location is used; when
    ///   that endpoint has none the other one is used with its own basis;
    ///   a location without a line gives basis `file`; neither endpoint
    ///   located gives basis `none`.
    pub fn fill(
        &mut self,
        category: EdgeCategoryId,
        to_kind: Option<NodeKindId>,
        from: Option<&Location>,
        to: Option<&Location>,
    ) {
        if self.file.is_some() {
            return;
        }
        if self.line.is_some() {
            if let Some((file, _)) = from {
                self.file = Some(file.clone());
            }
            self.basis = Basis::Site;
            return;
        }
        let first = match self.basis {
            Basis::FromNode | Basis::ToNode => self.basis,
            _ => default_basis(category, to_kind),
        };
        let second = if first == Basis::ToNode {
            Basis::FromNode
        } else {
            Basis::ToNode
        };
        for side in [first, second] {
            let loc = if side == Basis::ToNode { to } else { from };
            if let Some((file, line)) = loc {
                self.file = Some(file.clone());
                self.line = *line;
                self.basis = if line.is_some() { side } else { Basis::File };
                return;
            }
        }
        self.basis = Basis::None;
    }
}

/// Put `ev` on `e` as its one EVIDENCE cell: an existing one is replaced in
/// place (any further EVIDENCE cells are dropped), otherwise it is appended.
pub fn attach(e: &mut Edge, ev: Evidence) {
    let cell = ev.to_cell();
    match e.cells.iter().position(|c| c.kind == cell_type::EVIDENCE) {
        Some(i) => {
            let mut seen = 0usize;
            e.cells.retain(|c| {
                if c.kind != cell_type::EVIDENCE {
                    return true;
                }
                seen += 1;
                seen == 1
            });
            e.cells[i] = cell;
        }
        None => e.cells.push(cell),
    }
}

/// A CALLS edge a parser resolved inside its own file (LC.3b), with its
/// evidence: emitter `parser:<lang>`, rule `intra_file`, and the call's 0-based
/// `line` (basis `site`; the fill pass takes the file from `from`, the caller).
pub fn intra_file_call(lang: &str, from: NodeId, to: NodeId, line: u32) -> Edge {
    let ev = Evidence::emitter(format!("parser:{lang}"))
        .rule("intra_file")
        .line(line);
    Edge::new(from, to, edge_category::CALLS, Confidence::Strong).with_cell(ev.to_cell())
}

/// Attach `Evidence::emitter(emitter)` to every edge of `edges` that carries
/// no EVIDENCE cell. Never overrides one an emitter attached itself. Returns
/// how many edges it stamped.
pub fn stamp_missing(edges: &mut [Edge], emitter: &str) -> usize {
    stamp_missing_with(edges, &Evidence::emitter(emitter))
}

/// [`stamp_missing`] with a whole evidence (an emitter plus a rule, say).
pub fn stamp_missing_with(edges: &mut [Edge], ev: &Evidence) -> usize {
    let mut cell: Option<Cell> = None;
    let mut stamped = 0usize;
    for e in edges.iter_mut() {
        if e.cells.iter().any(|c| c.kind == cell_type::EVIDENCE) {
            continue;
        }
        let c = cell.get_or_insert_with(|| ev.to_cell());
        e.cells.push(c.clone());
        stamped += 1;
    }
    stamped
}

/// A node's location from its cells: the first POSITION cell with a file
/// (`file`, `start_line`); else the HTTP span fallback
/// ([`endpoint::http_node_span`]: ENDPOINT_HIT, then a JSON ROUTE_METHOD),
/// which is how `locate_node` places an HTTP node that carries no POSITION.
/// Lines are 0-based; one that does not fit a `u32` reads as unknown.
pub fn locate(cells: &[Cell]) -> Option<Location> {
    for c in cells.iter().filter(|c| c.kind == cell_type::POSITION) {
        let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(s) else {
            continue;
        };
        let Some(file) = v
            .get("file")
            .and_then(serde_json::Value::as_str)
            .filter(|f| !f.is_empty())
        else {
            continue;
        };
        let line = v
            .get("start_line")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        return Some((file.to_string(), line));
    }
    let (file, line0) = endpoint::http_node_span(cells)?;
    Some((file, line0.and_then(|n| u32::try_from(n).ok())))
}

/// Call-site-shaped node kinds: a node minted AT the construct that asserts an
/// edge into it (a client call, a publish, an invocation), so its own location
/// is the edge's site.
pub const SITE_KINDS: &[NodeKindId] = &[
    node_kind::ENDPOINT,
    node_kind::GRPC_CLIENT,
    node_kind::WS_CLIENT,
    node_kind::EVENT_EMITTER,
    node_kind::QUEUE_PRODUCER,
    node_kind::QUEUE_CONSUMER,
    node_kind::CLI_INVOCATION,
    node_kind::RPC_CALL,
    node_kind::GRAPHQL_OPERATION,
];

/// Which endpoint locates an edge that recorded no site: the child for a
/// structural edge (DEFINES / CONTAINS / HAS_ATTRIBUTE) or an edge into a
/// call-site node ([`SITE_KINDS`]), the source otherwise.
pub fn default_basis(category: EdgeCategoryId, to_kind: Option<NodeKindId>) -> Basis {
    let structural = category == edge_category::DEFINES
        || category == edge_category::CONTAINS
        || category == edge_category::HAS_ATTRIBUTE;
    if structural || to_kind.is_some_and(|k| SITE_KINDS.contains(&k)) {
        Basis::ToNode
    } else {
        Basis::FromNode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::{Confidence, NodeId};

    fn edge() -> Edge {
        Edge::new(
            NodeId(1),
            NodeId(2),
            edge_category::CALLS,
            Confidence::Strong,
        )
    }

    fn json(kind: repo_graph_core::CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(s.to_string()),
        }
    }

    #[test]
    fn json_round_trip_in_struct_order_skipping_absent_fields() {
        let ev = Evidence::emitter("graph:calls")
            .rule("import_binding")
            .at("b.py", 5);
        let cell = ev.to_cell();
        assert_eq!(cell.kind, cell_type::EVIDENCE);
        assert_eq!(
            cell.payload,
            CellPayload::Json(
                r#"{"emitter":"graph:calls","rule":"import_binding","file":"b.py","line":5,"basis":"site"}"#
                    .to_string()
            )
        );
        assert_eq!(Evidence::read(&[cell]), Some(ev));

        let bare = Evidence::emitter("resolver:http");
        assert_eq!(
            bare.to_cell().payload,
            CellPayload::Json(r#"{"emitter":"resolver:http","basis":"none"}"#.to_string())
        );
        assert_eq!(Evidence::read(&[bare.to_cell()]), Some(bare));

        let line_only = Evidence::emitter("parser:python").line(3);
        assert_eq!((line_only.file.as_deref(), line_only.line), (None, Some(3)));
        assert_eq!(line_only.basis, Basis::Site);
    }

    #[test]
    fn malformed_payload_reads_as_none() {
        for bad in [
            "",
            "{",
            "[]",
            r#"{"rule":"x"}"#,
            r#"{"emitter":"a","basis":"sideways"}"#,
        ] {
            assert_eq!(
                Evidence::read(&[json(cell_type::EVIDENCE, bad)]),
                None,
                "{bad:?}"
            );
        }
        let bytes = Cell {
            kind: cell_type::EVIDENCE,
            payload: CellPayload::Bytes(vec![1, 2]),
        };
        assert_eq!(Evidence::read(&[bytes]), None);
        // Only the EVIDENCE kind is read.
        assert_eq!(
            Evidence::read(&[json(cell_type::ORIGIN, r#"{"emitter":"x"}"#)]),
            None
        );
        // A missing basis defaults to none rather than failing.
        assert_eq!(
            Evidence::read(&[json(cell_type::EVIDENCE, r#"{"emitter":"x"}"#)]),
            Some(Evidence::emitter("x"))
        );
    }

    #[test]
    fn attach_replaces_and_keeps_at_most_one() {
        let mut e = edge();
        attach(&mut e, Evidence::emitter("parser:python"));
        e.cells.push(json(cell_type::ORIGIN, "{}"));
        attach(&mut e, Evidence::emitter("graph:build").rule("r"));
        assert_eq!(e.cells.len(), 2);
        assert_eq!(e.cells[0].kind, cell_type::EVIDENCE, "replaced in place");
        assert_eq!(
            Evidence::of(&e),
            Some(Evidence::emitter("graph:build").rule("r"))
        );

        // A stray second EVIDENCE cell is dropped on the next attach.
        e.cells.push(Evidence::emitter("dup").to_cell());
        attach(&mut e, Evidence::emitter("pass:tests"));
        let n = e
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::EVIDENCE)
            .count();
        assert_eq!(n, 1);
        assert_eq!(
            Evidence::of(&e).map(|ev| ev.emitter),
            Some("pass:tests".to_string())
        );
    }

    #[test]
    fn stamp_missing_never_overrides() {
        let mut edges = vec![edge(), edge(), edge()];
        attach(
            &mut edges[1],
            Evidence::emitter("resolver:http").rule("suffix"),
        );
        assert_eq!(stamp_missing(&mut edges, "graph:build"), 2);
        assert_eq!(
            stamp_missing(&mut edges, "graph:other"),
            0,
            "second stamp is a no-op"
        );
        let emitters: Vec<String> = edges
            .iter()
            .filter_map(Evidence::of)
            .map(|ev| ev.emitter)
            .collect();
        assert_eq!(emitters, ["graph:build", "resolver:http", "graph:build"]);
        assert_eq!(
            Evidence::of(&edges[1]).and_then(|ev| ev.rule),
            Some("suffix".to_string())
        );
    }

    #[test]
    fn locate_prefers_position_then_falls_back_to_endpoint_hit() {
        let pos = json(
            cell_type::POSITION,
            r#"{"file":"a.py","start_line":4,"end_line":9}"#,
        );
        let hit = json(
            cell_type::ENDPOINT_HIT,
            r#"{"file":"client.ts","line":2,"method":"GET"}"#,
        );
        assert_eq!(
            locate(&[hit.clone(), pos.clone()]),
            Some(("a.py".to_string(), Some(4)))
        );
        // No POSITION: ENDPOINT_HIT's 1-based line becomes 0-based.
        assert_eq!(locate(&[hit]), Some(("client.ts".to_string(), Some(1))));
        // A JSON ROUTE_METHOD places a route; the bare verb does not.
        let rm = json(
            cell_type::ROUTE_METHOD,
            r#"{"method":"GET","file":"r.go","line":7}"#,
        );
        assert_eq!(locate(&[rm]), Some(("r.go".to_string(), Some(6))));
        let verb = Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text("GET".into()),
        };
        assert_eq!(locate(&[verb]), None);
        // A POSITION with no file is skipped; a file without a line is kept.
        let nofile = json(cell_type::POSITION, r#"{"start_line":1}"#);
        let noline = json(cell_type::POSITION, r#"{"file":"x.rs"}"#);
        assert_eq!(locate(&[nofile, noline]), Some(("x.rs".to_string(), None)));
        assert_eq!(locate(&[]), None);
    }

    #[test]
    fn default_basis_points_structural_and_site_edges_at_the_child() {
        assert_eq!(default_basis(edge_category::DEFINES, None), Basis::ToNode);
        assert_eq!(default_basis(edge_category::CONTAINS, None), Basis::ToNode);
        assert_eq!(
            default_basis(edge_category::HAS_ATTRIBUTE, None),
            Basis::ToNode
        );
        assert_eq!(
            default_basis(edge_category::CALLS, Some(node_kind::ENDPOINT)),
            Basis::ToNode
        );
        assert_eq!(
            default_basis(edge_category::CALLS, Some(node_kind::FUNCTION)),
            Basis::FromNode
        );
        assert_eq!(
            default_basis(edge_category::HTTP_CALLS, Some(node_kind::ROUTE)),
            Basis::FromNode
        );
        assert_eq!(
            default_basis(edge_category::DOCUMENTS, None),
            Basis::FromNode
        );
    }

    #[test]
    fn fill_uses_the_chosen_endpoint_then_the_other_and_is_idempotent() {
        let from: Location = ("b.py".to_string(), Some(3));
        let to: Location = ("a.py".to_string(), Some(0));
        let cat = edge_category::CALLS;

        let mut ev = Evidence::emitter("graph:build");
        ev.fill(cat, Some(node_kind::FUNCTION), Some(&from), Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.line, ev.basis),
            (Some("b.py"), Some(3), Basis::FromNode)
        );
        let once = ev.clone();
        ev.fill(cat, Some(node_kind::FUNCTION), Some(&to), Some(&from));
        assert_eq!(ev, once, "a located evidence is never rewritten");

        // Structural: the child.
        let mut ev = Evidence::emitter("parser:python");
        ev.fill(edge_category::DEFINES, None, Some(&from), Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.basis),
            (Some("a.py"), Basis::ToNode)
        );

        // The chosen endpoint unlocated: the other one, with its own basis.
        let mut ev = Evidence::emitter("resolver:http");
        ev.fill(cat, None, None, Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.basis),
            (Some("a.py"), Basis::ToNode)
        );

        // A location without a line: basis file.
        let fileonly: Location = ("x.md".to_string(), None);
        let mut ev = Evidence::emitter("pass:doclink");
        ev.fill(edge_category::DOCUMENTS, None, Some(&fileonly), Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.line, ev.basis),
            (Some("x.md"), None, Basis::File)
        );

        // Neither endpoint located.
        let mut ev = Evidence::emitter("resolver:queue");
        ev.fill(cat, None, None, None);
        assert_eq!((ev.file, ev.basis), (None, Basis::None));

        // A site line takes the from node's file and keeps its line and basis.
        let mut ev = Evidence::emitter("parser:python").line(5);
        ev.fill(cat, None, Some(&from), Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.line, ev.basis),
            (Some("b.py"), Some(5), Basis::Site)
        );

        // An emitter's explicit endpoint choice beats the default.
        let mut ev = Evidence::emitter("resolver:http");
        ev.basis = Basis::ToNode;
        ev.fill(cat, None, Some(&from), Some(&to));
        assert_eq!(
            (ev.file.as_deref(), ev.basis),
            (Some("a.py"), Basis::ToNode)
        );
    }

    #[test]
    fn intra_file_call_is_a_parser_site() {
        let e = intra_file_call("python", NodeId(1), NodeId(2), 7);
        assert_eq!((e.from, e.to, e.category), (NodeId(1), NodeId(2), edge_category::CALLS));
        assert_eq!(e.confidence, Confidence::Strong);
        let ev = Evidence::of(&e).expect("one EVIDENCE cell");
        assert_eq!(
            ev,
            Evidence::emitter("parser:python").rule("intra_file").line(7)
        );
        // The fill pass keeps the line and takes the caller's file.
        let mut filled = ev.clone();
        let from: Location = ("b.py".to_string(), Some(3));
        filled.fill(edge_category::CALLS, Some(node_kind::FUNCTION), Some(&from), None);
        assert_eq!(
            (filled.file.as_deref(), filled.line, filled.basis),
            (Some("b.py"), Some(7), Basis::Site)
        );
    }

    #[test]
    fn every_stage_is_lowercase_and_colon_free() {
        for s in STAGES {
            assert!(!s.is_empty() && !s.contains(':'));
            assert_eq!(*s, s.to_ascii_lowercase());
        }
    }
}
