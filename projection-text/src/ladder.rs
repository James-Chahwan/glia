//! The fidelity ladder (CC.4a): one node rendered at one of four rungs, and a
//! pack renderer that lays out many `(node, rung)` picks as one text.
//!
//! Every other projection in this crate renders the whole output at ONE
//! fidelity ([`crate::render_merged`] one-line previews,
//! [`crate::render_merged_full`] whole bodies, [`crate::render_prose`] three
//! lines). A budgeted context pack needs each node at its own rung: the seed's
//! whole body, its callers' signatures, the far neighbourhood as one line or
//! only a name. The rungs, cheapest first:
//!
//! - [`Fidelity::Qname`](crate::ladder::Fidelity::Qname): the bare qname.
//! - [`Fidelity::Outline`](crate::ladder::Fidelity::Outline): one located line, `- <qname> (<KIND> <file>:<line>)`.
//! - [`Fidelity::Preview`](crate::ladder::Fidelity::Preview): a `### ` header, the DOC's first line and the
//!   SIGNATURE (the CODE lines up to the first one that opens a body, at most
//!   [`PREVIEW_LINES`](crate::ladder::PREVIEW_LINES)), fenced.
//! - [`Fidelity::Full`](crate::ladder::Fidelity::Full): the header, the DOC's first paragraph and the whole
//!   CODE text, fenced.
//!
//! Full and Preview show a body, so they need one: a node with no CODE text
//! (a ROUTE, an ENDPOINT, or a CODE cell that came back from the store as
//! `code_span` JSON because its source moved or is gone, CD.7c) renders at
//! Outline when asked for either ([`rendered_as`](crate::ladder::rendered_as) says which rung a pick
//! really renders at).
//!
//! NOTES (FB.11). A node's DECISION and CONV entries (each cell a JSON array
//! of `{"source", "id", ..}` entries, `glia_code_domain::external_inputs`'
//! PAYLOADS) are what a writer said about it: a human's overlay `[[decision]]`
//! / `[[note]]`, an ADR, or a cell-API write such as Engram's corrections. At
//! Outline and above a node renders them after its doc line, one
//! `note[<source>/<id>, <tier>]: <title or text>` line each (whitespace
//! collapsed, cut at [`NOTE_CHARS`](crate::ladder::NOTE_CHARS) chars), at most
//! [`MAX_NOTES`](crate::ladder::MAX_NOTES) then `(+<k> more notes)`. The tier
//! is `heuristic` for source `api` (a writer's statement, never a fact glia
//! read) and `declared` for `overlay` / `adr`. Left out: CONV entries of
//! `kind` `convention` (FB.12b renders conventions) and overlay DECISION
//! entries on a PROJECT (a `[[decision]]` scope stanza's record, not per-node
//! text). A Qname renders none; a node with no entry renders as before.
//! [`node_notes`](crate::ladder::node_notes) lists them for a caller that
//! counts them (the engine pack's manifest).
//!
//! Locations are 1-based (an editor's line): [`crate::node_position`]'s
//! 0-based POSITION rows plus one. The kind is the code-domain registry's
//! upper-case name ([`node_kind::name`]), the name the engine's located rows
//! carry. A node's qname and kind come from the FIRST graph (in
//! `MergedGraph::graphs` order) whose node list holds its id.
//!
//! Pure renderer: it prints nothing. CC.4b's engine pack chooses the picks and
//! prints the `[pack] ...` marker. No `HashMap` iteration reaches the output:
//! blocks follow the caller's pick order and links are sorted.

use std::collections::{HashMap, HashSet};

use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, EdgeCategoryId, Node, NodeId, NodeKindId};
use glia_graph::{MergedGraph, RepoGraph};

use crate::node_position;

/// The most CODE lines a Preview shows when no line within them opens a body.
pub const PREVIEW_LINES: usize = 6;

/// The most `## links` lines a pack carries; the rest are counted in one
/// `… <n> more links` line.
pub const MAX_LINKS: usize = 200;

/// Edge categories a pack never lists as links: structure (DEFINES, CONTAINS)
/// the qnames already show, and git co-change history, which is not code.
const NOT_LINKS: [EdgeCategoryId; 3] = [
    edge_category::DEFINES,
    edge_category::CONTAINS,
    edge_category::CO_CHANGES,
];

/// The most note lines one node renders; the rest are counted in one
/// `(+<k> more notes)` line.
pub const MAX_NOTES: usize = 3;

/// The most chars of a note's text a note line shows.
pub const NOTE_CHARS: usize = 200;

/// A line of CODE ending (trailing whitespace ignored) in one of these opens
/// the body: the Preview's signature ends there.
const SIGNATURE_ENDS: [&str; 4] = ["{", ":", "=>", "->"];

/// A rung of the ladder. `Ord` follows cost: `Qname < Outline < Preview < Full`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Fidelity {
    Qname,
    Outline,
    Preview,
    Full,
}

impl Fidelity {
    /// Every rung, cheapest first.
    pub const LADDER: [Fidelity; 4] = [
        Fidelity::Qname,
        Fidelity::Outline,
        Fidelity::Preview,
        Fidelity::Full,
    ];

    /// The rung's name: `qname`, `outline`, `preview` or `full`.
    pub fn name(self) -> &'static str {
        match self {
            Fidelity::Qname => "qname",
            Fidelity::Outline => "outline",
            Fidelity::Preview => "preview",
            Fidelity::Full => "full",
        }
    }
}

/// One node of a pack at the rung the caller chose for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub id: NodeId,
    pub fidelity: Fidelity,
}

/// The node's CODE text: its first `Text` CODE cell. A CODE cell that the
/// store handed back as `code_span` JSON (CD.7c: its source moved, changed or
/// is gone) is a `Json` payload, a reference rather than code, so it is not
/// text here. The ladder and [`crate::render_prose`] read CODE only through
/// this accessor. The crate's other CODE readers do not render its text as
/// code: the dense projection's `render_cell` prints every payload as it is
/// (a `code_span` JSON included) and the research-only
/// `driver_utils::extract_code_cell` reads Text CODE cells itself.
pub fn code_text(n: &Node) -> Option<&str> {
    first_text(n, cell_type::CODE)
}

/// The node's DOC text: its first `Text` DOC cell.
pub(crate) fn doc_text(n: &Node) -> Option<&str> {
    first_text(n, cell_type::DOC)
}

fn first_text(n: &Node, kind: glia_core::CellTypeId) -> Option<&str> {
    n.cells.iter().find_map(|c| match &c.payload {
        CellPayload::Text(s) if c.kind == kind => Some(s.as_str()),
        _ => None,
    })
}

/// One DECISION or CONV entry as the ladder renders it (FB.11).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Note {
    /// The entry's writer: `api`, `overlay` or `adr`.
    pub source: String,
    /// The entry's id within its source (Engram's are `engram:<..>`).
    pub id: String,
    /// `heuristic` (source `api`) or `declared` (overlay / adr).
    pub tier: &'static str,
    /// The DECISION's `title` (else `text`) or the CONV's `text`, whitespace
    /// collapsed, at most [`NOTE_CHARS`] chars (a cut one ends in `…`).
    pub text: String,
}

/// The tier a note's source carries: `declared` for what a human declared in
/// the repo (overlay, ADR), `heuristic` for anything else (a cell-API writer).
fn note_tier(source: &str) -> &'static str {
    match source {
        "overlay" | "adr" => "declared",
        _ => "heuristic",
    }
}

/// The notes of `n` (of kind `kind`), sorted by `(source, id)` (DECISION
/// before CONV on a tie). An entry that is not an object, lacks a string
/// `source` / `id` or has no text is skipped, and so is a payload that is not
/// a JSON array; see the module doc for what else is left out.
fn notes_of(n: &Node, kind: Option<NodeKindId>) -> Vec<Note> {
    let mut out: Vec<(u8, Note)> = Vec::new();
    for c in &n.cells {
        let rank = match c.kind {
            k if k == cell_type::DECISION => 0u8,
            k if k == cell_type::CONV => 1u8,
            _ => continue,
        };
        let CellPayload::Json(s) = &c.payload else {
            continue;
        };
        let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(s)
        else {
            continue;
        };
        for e in &entries {
            let str_of = |k: &str| e.get(k).and_then(serde_json::Value::as_str);
            let (Some(source), Some(id)) = (str_of("source"), str_of("id")) else {
                continue;
            };
            if rank == 1 && str_of("kind") == Some("convention") {
                continue;
            }
            if rank == 0 && source == "overlay" && kind == Some(node_kind::PROJECT) {
                continue;
            }
            let raw = if rank == 0 {
                str_of("title")
                    .filter(|t| !t.trim().is_empty())
                    .or(str_of("text"))
            } else {
                str_of("text")
            };
            let words = raw.unwrap_or("").split_whitespace().collect::<Vec<_>>();
            if words.is_empty() {
                continue;
            }
            let whole = words.join(" ");
            let text = if whole.chars().count() > NOTE_CHARS {
                let mut cut: String = whole.chars().take(NOTE_CHARS).collect();
                cut.push('…');
                cut
            } else {
                whole
            };
            out.push((
                rank,
                Note {
                    source: source.to_string(),
                    id: id.to_string(),
                    tier: note_tier(source),
                    text,
                },
            ));
        }
    }
    out.sort_by(|(ra, a), (rb, b)| {
        (a.source.as_str(), a.id.as_str(), ra).cmp(&(b.source.as_str(), b.id.as_str(), rb))
    });
    out.into_iter().map(|(_, n)| n).collect()
}

/// The notes [`render_node`] renders for `id` at Outline and above (all of
/// them, the ones past [`MAX_NOTES`] included), in render order. Empty when
/// the node has none or no graph holds `id`.
pub fn node_notes(m: &MergedGraph, id: NodeId) -> Vec<Note> {
    held(m, id).map(|h| h.notes()).unwrap_or_default()
}

/// The note lines for `notes`, each prefixed with `indent` and ending in a
/// newline: at most [`MAX_NOTES`], then `(+<k> more notes)`.
fn note_lines(notes: &[Note], indent: &str) -> String {
    let mut out = String::new();
    for n in notes.iter().take(MAX_NOTES) {
        out.push_str(&format!(
            "{indent}note[{}/{}, {}]: {}\n",
            n.source, n.id, n.tier, n.text
        ));
    }
    let more = notes.len().saturating_sub(MAX_NOTES);
    if more > 0 {
        out.push_str(&format!("{indent}(+{more} more notes)\n"));
    }
    out
}

/// A node as the ladder renders it: the node, its qname and its kind name,
/// read from the graph that holds it.
struct Held<'a> {
    node: &'a Node,
    qname: &'a str,
    kind: &'static str,
    kind_id: Option<NodeKindId>,
}

impl<'a> Held<'a> {
    fn of(g: &'a RepoGraph, node: &'a Node) -> Self {
        let qname = g
            .nav
            .qname_by_id
            .get(&node.id)
            .map(String::as_str)
            .unwrap_or("");
        let kind_id = g.nav.kind_by_id.get(&node.id).copied();
        let kind = kind_id.map(node_kind::name).unwrap_or("UNKNOWN");
        Held {
            node,
            qname,
            kind,
            kind_id,
        }
    }

    fn notes(&self) -> Vec<Note> {
        notes_of(self.node, self.kind_id)
    }

    /// The rung this node really renders at when asked for `f`.
    fn rung(&self, f: Fidelity) -> Fidelity {
        if f >= Fidelity::Preview && body_lines(self.node).is_empty() {
            Fidelity::Outline
        } else {
            f
        }
    }

    fn render(&self, f: Fidelity) -> String {
        match self.rung(f) {
            Fidelity::Full => self.block(true),
            Fidelity::Preview => self.block(false),
            Fidelity::Outline => self.outline(),
            Fidelity::Qname => self.qname.to_string(),
        }
    }

    /// `- <qname> (<KIND> <file>:<start>)`, then the node's note lines
    /// indented two spaces under it; no trailing newline.
    fn outline(&self) -> String {
        let mut out = self.outline_line();
        let notes = note_lines(&self.notes(), "  ");
        if !notes.is_empty() {
            out.push('\n');
            out.push_str(notes.trim_end_matches('\n'));
        }
        out
    }

    /// The bare located line.
    fn outline_line(&self) -> String {
        match node_position(self.node) {
            Some(p) => {
                format!(
                    "- {} ({} {}:{})",
                    self.qname,
                    self.kind,
                    p.file,
                    p.start_line.saturating_add(1)
                )
            }
            None => format!("- {} ({})", self.qname, self.kind),
        }
    }

    /// The Full (`whole`) or Preview block, ending in a newline.
    fn block(&self, whole: bool) -> String {
        let pos = node_position(self.node);
        let mut out = match &pos {
            Some(p) => format!(
                "### {} ({} {}:{}-{})\n",
                self.qname,
                self.kind,
                p.file,
                p.start_line.saturating_add(1),
                p.end_line.saturating_add(1)
            ),
            None => format!("### {} ({})\n", self.qname, self.kind),
        };
        let doc = doc_text(self.node).map(doc_paragraph).unwrap_or_default();
        let doc: &[&str] = if whole {
            &doc
        } else {
            doc.get(..1).unwrap_or(&[])
        };
        for line in doc {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&note_lines(&self.notes(), ""));

        let body = body_lines(self.node);
        let shown: &[&str] = if whole { &body } else { signature(&body) };
        let fence = fence_for(&body);
        out.push_str(&fence);
        out.push_str(pos.as_ref().map(|p| lang_word(&p.file)).unwrap_or(""));
        out.push('\n');
        for line in shown {
            out.push_str(line);
            out.push('\n');
        }
        if shown.len() < body.len() {
            out.push_str("…\n");
        }
        out.push_str(&fence);
        out.push('\n');
        out
    }
}

/// The node's CODE text as lines, with leading and trailing blank lines
/// dropped. Empty when the node has no CODE text, or only whitespace: then
/// there is no body for Full or Preview to show.
fn body_lines(n: &Node) -> Vec<&str> {
    let Some(code) = code_text(n) else {
        return Vec::new();
    };
    let lines: Vec<&str> = code.lines().collect();
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    match (first, last) {
        (Some(a), Some(b)) => lines.get(a..=b).map(<[&str]>::to_vec).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The DOC's first paragraph: its lines (trimmed) from the first non-blank
/// one up to the next blank one.
fn doc_paragraph(doc: &str) -> Vec<&str> {
    doc.lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect()
}

/// The Preview's signature: `body` up to and including the first line that
/// ends in one of [`SIGNATURE_ENDS`], at most [`PREVIEW_LINES`] lines. A cut by
/// lines, so never inside a UTF-8 character.
fn signature<'b, 'a>(body: &'b [&'a str]) -> &'b [&'a str] {
    let window = body.get(..PREVIEW_LINES.min(body.len())).unwrap_or(body);
    let end = window
        .iter()
        .position(|l| {
            let l = l.trim_end();
            SIGNATURE_ENDS.iter().any(|e| l.ends_with(e))
        })
        .map_or(window.len(), |i| i + 1);
    window.get(..end).unwrap_or(window)
}

/// A backtick fence longer than any backtick run in `body` (at least three),
/// so a body holding its own fences (a markdown doc chunk) cannot close ours.
fn fence_for(body: &[&str]) -> String {
    let longest = body
        .iter()
        .flat_map(|l| l.split(|c| c != '`'))
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.saturating_add(1).max(3))
}

/// The fence info string for `file`: the language word of its extension, or
/// empty for an extension that is not a code language here.
fn lang_word(file: &str) -> &'static str {
    let name = file.rsplit('/').next().unwrap_or(file);
    let Some((_, ext)) = name.rsplit_once('.') else {
        return "";
    };
    match ext.to_ascii_lowercase().as_str() {
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "java" => "java",
        "kt" => "kotlin",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "dart" => "dart",
        "ex" | "exs" => "elixir",
        "scala" => "scala",
        "clj" => "clojure",
        "sol" => "solidity",
        "c" | "h" | "cc" | "cpp" | "hpp" => "cpp",
        _ => "",
    }
}

/// The first graph (in `m.graphs` order) whose node list holds `id`.
fn held(m: &MergedGraph, id: NodeId) -> Option<Held<'_>> {
    m.graphs
        .iter()
        .find_map(|g| g.nodes.iter().find(|n| n.id == id).map(|n| Held::of(g, n)))
}

/// One node's text at rung `f`: a Full or Preview block ends in a newline, an
/// Outline line and a Qname do not. A Full or Preview request for a node with
/// no CODE text renders its Outline line. `None` when no graph holds `id`.
pub fn render_node(m: &MergedGraph, id: NodeId, f: Fidelity) -> Option<String> {
    held(m, id).map(|h| h.render(f))
}

/// The rung [`render_node`] really renders `id` at when asked for `f`: `f`,
/// or Outline for a Full / Preview request on a node with no CODE text.
/// `None` when no graph holds `id`.
pub fn rendered_as(m: &MergedGraph, id: NodeId, f: Fidelity) -> Option<Fidelity> {
    held(m, id).map(|h| h.rung(f))
}

/// The whole pack for `picks`, in the given order (the caller ranks):
///
/// ```text
/// # <title>
///
/// <every Full and Preview block, in pick order, blank-separated>
///
/// ## outline
/// <the Outline lines, in pick order>
///
/// also: <the Qname picks, comma-separated, in pick order>
///
/// ## links
/// <from qname> -<CATEGORY>-> <to qname>
/// ```
///
/// Each part is followed by one blank line; a part with nothing in it (no
/// blocks, no outline lines, no Qname picks, no links) is left out. The title
/// is one line (its whitespace runs collapse to one space). A pick whose id no
/// graph holds is skipped, and so is a repeat of an id already picked (the
/// first pick of an id wins). Links are the edges between two picks rendered
/// at Outline or above, whose category is not DEFINES / CONTAINS / CO_CHANGES,
/// over [`MergedGraph::all_edges`]: deduplicated by `(from, category, to)` and
/// by their text, sorted by `(from qname, category name, to qname)`, at most
/// [`MAX_LINKS`] of them.
pub fn render_pack(m: &MergedGraph, title: &str, picks: &[Pick]) -> String {
    let wanted: HashSet<NodeId> = picks.iter().map(|p| p.id).collect();
    let mut at: HashMap<NodeId, Held<'_>> = HashMap::with_capacity(wanted.len());
    'graphs: for g in &m.graphs {
        for n in &g.nodes {
            if wanted.contains(&n.id) && !at.contains_key(&n.id) {
                at.insert(n.id, Held::of(g, n));
                if at.len() == wanted.len() {
                    break 'graphs;
                }
            }
        }
    }

    let mut blocks: Vec<String> = Vec::new();
    let mut outline: Vec<String> = Vec::new();
    let mut also: Vec<&str> = Vec::new();
    let mut linked: HashMap<NodeId, &str> = HashMap::new();
    let mut done: HashSet<NodeId> = HashSet::with_capacity(picks.len());
    for p in picks {
        let Some(h) = at.get(&p.id) else { continue };
        if !done.insert(p.id) {
            continue;
        }
        let rung = h.rung(p.fidelity);
        match rung {
            Fidelity::Full | Fidelity::Preview => blocks.push(h.render(rung)),
            Fidelity::Outline => outline.push(h.outline()),
            Fidelity::Qname => also.push(h.qname),
        }
        if rung >= Fidelity::Outline {
            linked.insert(p.id, h.qname);
        }
    }

    let mut parts: Vec<String> = Vec::with_capacity(blocks.len() + 4);
    parts.push(format!(
        "# {}\n",
        title.split_whitespace().collect::<Vec<_>>().join(" ")
    ));
    parts.extend(blocks);
    if !outline.is_empty() {
        parts.push(format!("## outline\n{}\n", outline.join("\n")));
    }
    if !also.is_empty() {
        parts.push(format!("also: {}\n", also.join(", ")));
    }
    let links = links(m, &linked);
    if !links.is_empty() {
        parts.push(format!("## links\n{}", links));
    }
    parts.join("\n")
}

/// The `## links` body: one `<from> -<CATEGORY>-> <to>` line per edge between
/// two `linked` picks (see [`render_pack`]), each ending in a newline. Empty
/// when there are none.
fn links(m: &MergedGraph, linked: &HashMap<NodeId, &str>) -> String {
    if linked.is_empty() {
        return String::new();
    }
    let mut keys: HashSet<(NodeId, EdgeCategoryId, NodeId)> = HashSet::new();
    let mut rows: Vec<(&str, &'static str, &str)> = Vec::new();
    for e in m.all_edges() {
        if NOT_LINKS.contains(&e.category) {
            continue;
        }
        let (Some(from), Some(to)) = (linked.get(&e.from), linked.get(&e.to)) else {
            continue;
        };
        if keys.insert((e.from, e.category, e.to)) {
            rows.push((from, edge_category::name(e.category), to));
        }
    }
    rows.sort_unstable();
    rows.dedup();
    let more = rows.len().saturating_sub(MAX_LINKS);
    let mut out = String::new();
    for (from, cat, to) in rows.iter().take(MAX_LINKS) {
        out.push_str(from);
        out.push_str(" -");
        out.push_str(cat);
        out.push_str("-> ");
        out.push_str(to);
        out.push('\n');
    }
    if more > 0 {
        out.push_str(&format!("… {more} more links\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::code_span::CodeSpan;
    use glia_core::{Cell, CellTypeId, Confidence, Edge, NodeKindId, RepoId};

    const PRICE_CODE: &str = "def price(o):\n    total = o\n    total = total + 0\n    total = total * 1\n    return total";

    const PRICE_FULL: &str = "### shop::a::price (FUNCTION shop/a.py:1-5)\nPrice an order.\n```python\ndef price(o):\n    total = o\n    total = total + 0\n    total = total * 1\n    return total\n```\n";

    fn pos(file: &str, s: u32, e: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{s},"end_line":{e}}}"#
            )),
        }
    }

    fn text(kind: CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Text(s.into()),
        }
    }

    fn edge(from: NodeId, to: NodeId, category: EdgeCategoryId) -> Edge {
        Edge {
            from,
            to,
            category,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        }
    }

    struct Shop {
        m: MergedGraph,
        module: NodeId,
        price: NodeId,
        place: NodeId,
    }

    /// The acceptance graph: MODULE shop::a, FUNCTION shop::a::price (DOC +
    /// CODE), FUNCTION shop::a::place (CODE), place -CALLS-> price and
    /// shop::a -DEFINES-> price. Built the way lib.rs's `mini_graph` is.
    fn shop() -> Shop {
        let repo = RepoId::from_canonical("test://shop");
        let id = |k: NodeKindId, q: &str| NodeId::from_parts("code", repo, k, q);
        let module = id(node_kind::MODULE, "shop::a");
        let price = id(node_kind::FUNCTION, "shop::a::price");
        let place = id(node_kind::FUNCTION, "shop::a::place");
        let node = |id: NodeId, cells: Vec<Cell>| Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        };
        let mut g = RepoGraph {
            repo,
            nodes: vec![
                node(module, vec![pos("shop/a.py", 0, 9)]),
                node(
                    price,
                    vec![
                        pos("shop/a.py", 0, 4),
                        text(cell_type::DOC, "Price an order.\n\nLong text."),
                        text(cell_type::CODE, PRICE_CODE),
                    ],
                ),
                node(
                    place,
                    vec![
                        pos("shop/a.py", 7, 8),
                        text(cell_type::CODE, "def place(o):\n    return price(o)"),
                    ],
                ),
            ],
            edges: vec![
                edge(place, price, edge_category::CALLS),
                edge(module, price, edge_category::DEFINES),
            ],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        g.nav
            .record(module, "a", "shop::a", node_kind::MODULE, None);
        g.nav.record(
            price,
            "price",
            "shop::a::price",
            node_kind::FUNCTION,
            Some(module),
        );
        g.nav.record(
            place,
            "place",
            "shop::a::place",
            node_kind::FUNCTION,
            Some(module),
        );
        Shop {
            m: MergedGraph::new(vec![g]),
            module,
            price,
            place,
        }
    }

    #[test]
    fn full_block() {
        let s = shop();
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Full).as_deref(),
            Some(PRICE_FULL)
        );
    }

    #[test]
    fn preview_stops_at_the_signature() {
        let s = shop();
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Preview).as_deref(),
            Some(
                "### shop::a::price (FUNCTION shop/a.py:1-5)\nPrice an order.\n```python\ndef price(o):\n…\n```\n"
            )
        );
    }

    #[test]
    fn outline_and_qname() {
        let s = shop();
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Outline).as_deref(),
            Some("- shop::a::price (FUNCTION shop/a.py:1)")
        );
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Qname).as_deref(),
            Some("shop::a::price")
        );
    }

    #[test]
    fn pack_orders_sections_and_links() {
        let s = shop();
        let picks = [
            Pick {
                id: s.price,
                fidelity: Fidelity::Full,
            },
            Pick {
                id: s.place,
                fidelity: Fidelity::Outline,
            },
            Pick {
                id: s.module,
                fidelity: Fidelity::Qname,
            },
        ];
        let want = format!(
            "# t\n\n{PRICE_FULL}\n## outline\n- shop::a::place (FUNCTION shop/a.py:8)\n\nalso: shop::a\n\n## links\nshop::a::place -CALLS-> shop::a::price\n"
        );
        assert_eq!(render_pack(&s.m, "t", &picks), want);
    }

    fn json(kind: CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(s.into()),
        }
    }

    /// `shop()` with `cells` appended to `id`'s node.
    fn shop_with(id_of: impl Fn(&Shop) -> NodeId, cells: Vec<Cell>) -> Shop {
        let mut s = shop();
        let id = id_of(&s);
        if let Some(n) = s.m.graphs[0].nodes.iter_mut().find(|n| n.id == id) {
            n.cells.extend(cells);
        }
        s
    }

    #[test]
    fn notes_render_at_outline_and_above_never_at_qname() {
        let s = shop_with(
            |s| s.price,
            vec![json(
                cell_type::DECISION,
                r#"[{"id":"engram:3f2a","source":"api","text":"keep   charge_card\n idempotent"}]"#,
            )],
        );
        let note = "note[api/engram:3f2a, heuristic]: keep charge_card idempotent";
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Outline).as_deref(),
            Some(format!("- shop::a::price (FUNCTION shop/a.py:1)\n  {note}").as_str())
        );
        let full = render_node(&s.m, s.price, Fidelity::Full).unwrap_or_default();
        assert_eq!(
            full,
            PRICE_FULL.replace("Price an order.\n", &format!("Price an order.\n{note}\n"))
        );
        let preview = render_node(&s.m, s.price, Fidelity::Preview).unwrap_or_default();
        assert!(
            preview.contains(&format!("Price an order.\n{note}\n```python")),
            "{preview}"
        );
        assert_eq!(
            render_node(&s.m, s.price, Fidelity::Qname).as_deref(),
            Some("shop::a::price")
        );
        assert_eq!(node_notes(&s.m, s.price).len(), 1);
        // In a pack the Outline's notes stay under its line.
        let picks = [Pick {
            id: s.price,
            fidelity: Fidelity::Outline,
        }];
        assert_eq!(
            render_pack(&s.m, "t", &picks),
            format!("# t\n\n## outline\n- shop::a::price (FUNCTION shop/a.py:1)\n  {note}\n")
        );
    }

    #[test]
    fn notes_sort_cap_tier_and_skip() {
        let long = "x".repeat(NOTE_CHARS + 5);
        let s = shop_with(
            |s| s.place,
            vec![
                json(
                    cell_type::CONV,
                    &format!(
                        r#"[{{"id":"000001","source":"api","text":"{long}"}},{{"id":"c1","kind":"convention","source":"api","text":"skip me"}},{{"id":"note#1","source":"overlay","text":"declared note"}}]"#
                    ),
                ),
                json(
                    cell_type::DECISION,
                    r#"[{"id":"0001-x","source":"adr","title":"Use queues","text":"body"},{"id":"d2","source":"api","title":"  ","text":"t2"},{"source":"api","text":"no id"}]"#,
                ),
                // Not an entry array: ignored.
                json(cell_type::CONV, "{}"),
            ],
        );
        let notes = node_notes(&s.m, s.place);
        let keys: Vec<(&str, &str, &str)> = notes
            .iter()
            .map(|n| (n.source.as_str(), n.id.as_str(), n.tier))
            .collect();
        assert_eq!(
            keys,
            [
                ("adr", "0001-x", "declared"),
                ("api", "000001", "heuristic"),
                ("api", "d2", "heuristic"),
                ("overlay", "note#1", "declared"),
            ]
        );
        assert_eq!(notes[0].text, "Use queues");
        assert_eq!(notes[2].text, "t2", "a blank title falls back to text");
        assert_eq!(notes[1].text.chars().count(), NOTE_CHARS + 1);
        assert!(notes[1].text.ends_with('…'));
        let out = render_node(&s.m, s.place, Fidelity::Outline).unwrap_or_default();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1 + MAX_NOTES + 1, "{out}");
        assert_eq!(lines[1], "  note[adr/0001-x, declared]: Use queues");
        assert_eq!(lines[4], "  (+1 more notes)");
        assert!(!out.contains("skip me"), "conventions are FB.12b's: {out}");
    }

    #[test]
    fn overlay_decisions_on_a_project_are_scope_records() {
        let mut s = shop();
        let decision = json(
            cell_type::DECISION,
            r#"[{"id":"d1","source":"overlay","title":"scope record"},{"id":"d2","source":"api","title":"kept"}]"#,
        );
        let g = &mut s.m.graphs[0];
        g.nav.kind_by_id.insert(s.module, node_kind::PROJECT);
        if let Some(n) = g.nodes.iter_mut().find(|n| n.id == s.module) {
            n.cells.push(decision);
        }
        let notes = node_notes(&s.m, s.module);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].id, "d2");
    }

    #[test]
    fn unknown_id_is_none() {
        let s = shop();
        let ghost = NodeId::from_parts(
            "code",
            RepoId::from_canonical("test://ghost"),
            node_kind::FUNCTION,
            "x",
        );
        for f in Fidelity::LADDER {
            assert_eq!(render_node(&s.m, ghost, f), None);
            assert_eq!(rendered_as(&s.m, ghost, f), None);
        }
        // An unknown pick is skipped in a pack, and links never reach it.
        let picks = [
            Pick {
                id: ghost,
                fidelity: Fidelity::Full,
            },
            Pick {
                id: s.place,
                fidelity: Fidelity::Outline,
            },
        ];
        assert_eq!(
            render_pack(&s.m, "t", &picks),
            "# t\n\n## outline\n- shop::a::place (FUNCTION shop/a.py:8)\n"
        );
    }

    /// node_doc_or_code before CC.4a, verbatim: the reference the routed
    /// version must agree with.
    fn node_doc_or_code_before(node: &Node) -> Option<&str> {
        let mut code = None;
        for c in &node.cells {
            if let CellPayload::Text(s) = &c.payload {
                if c.kind == cell_type::DOC {
                    return Some(s.as_str());
                }
                if c.kind == cell_type::CODE && code.is_none() {
                    code = Some(s.as_str());
                }
            }
        }
        code
    }

    #[test]
    fn code_text_reads_code_only() {
        let s = shop();
        let repo = RepoId::from_canonical("test://doc");
        let id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "d");
        let with = |cells: Vec<Cell>| Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells,
        };

        let doc_only = with(vec![text(cell_type::DOC, "only a doc")]);
        assert_eq!(code_text(&doc_only), None);
        let price = held(&s.m, s.price).map(|h| h.node);
        assert_eq!(price.and_then(code_text), Some(PRICE_CODE));

        let span = CodeSpan::of("shop/a.py", 0, PRICE_CODE.as_bytes()).to_payload();
        let spanned = with(vec![Cell {
            kind: cell_type::CODE,
            payload: span.clone(),
        }]);
        assert_eq!(
            code_text(&spanned),
            None,
            "a code_span payload is a reference, not code"
        );

        // node_doc_or_code reads CODE through code_text and answers as before.
        let cases = [
            doc_only,
            spanned,
            with(vec![
                text(cell_type::CODE, "c1"),
                text(cell_type::DOC, "d1"),
                text(cell_type::CODE, "c2"),
            ]),
            with(vec![
                text(cell_type::CODE, "c1"),
                text(cell_type::CODE, "c2"),
            ]),
            with(vec![
                Cell {
                    kind: cell_type::CODE,
                    payload: span,
                },
                text(cell_type::CODE, "c2"),
            ]),
            with(vec![Cell {
                kind: cell_type::DOC,
                payload: CellPayload::Json("{}".into()),
            }]),
            with(vec![pos("a.py", 0, 1)]),
            with(vec![]),
        ];
        for n in &cases {
            assert_eq!(
                crate::node_doc_or_code(n),
                node_doc_or_code_before(n),
                "{:?}",
                n.cells
            );
        }
        for n in held(&s.m, s.module)
            .into_iter()
            .chain(held(&s.m, s.place))
            .map(|h| h.node)
        {
            assert_eq!(crate::node_doc_or_code(n), node_doc_or_code_before(n));
        }
    }

    #[test]
    fn a_span_payload_renders_at_outline() {
        let mut s = shop();
        let span = CodeSpan::of("shop/a.py", 0, PRICE_CODE.as_bytes()).to_payload();
        for g in &mut s.m.graphs {
            for n in &mut g.nodes {
                for c in &mut n.cells {
                    if n.id == s.price && c.kind == cell_type::CODE {
                        c.payload = span.clone();
                    }
                }
            }
        }
        let outline = "- shop::a::price (FUNCTION shop/a.py:1)";
        for f in [Fidelity::Full, Fidelity::Preview, Fidelity::Outline] {
            assert_eq!(
                render_node(&s.m, s.price, f).as_deref(),
                Some(outline),
                "{f:?}"
            );
            assert_eq!(rendered_as(&s.m, s.price, f), Some(Fidelity::Outline));
        }
        assert_eq!(
            rendered_as(&s.m, s.price, Fidelity::Qname),
            Some(Fidelity::Qname)
        );
        // In a pack the Full pick sits in the outline section, and still links.
        let picks = [
            Pick {
                id: s.price,
                fidelity: Fidelity::Full,
            },
            Pick {
                id: s.place,
                fidelity: Fidelity::Full,
            },
        ];
        assert_eq!(
            render_pack(&s.m, "t", &picks),
            "# t\n\n### shop::a::place (FUNCTION shop/a.py:8-9)\n```python\ndef place(o):\n    return price(o)\n```\n\n## outline\n- shop::a::price (FUNCTION shop/a.py:1)\n\n## links\nshop::a::place -CALLS-> shop::a::price\n"
        );
    }

    #[test]
    fn pack_is_byte_identical_across_calls() {
        let s = shop();
        let picks: Vec<Pick> = [s.module, s.place, s.price]
            .into_iter()
            .zip(Fidelity::LADDER.into_iter().rev())
            .map(|(id, fidelity)| Pick { id, fidelity })
            .collect();
        let a = render_pack(&s.m, "t", &picks);
        let b = render_pack(&s.m, "t", &picks);
        assert_eq!(a, b);
        // Same graph rebuilt from scratch: the same bytes.
        assert_eq!(render_pack(&shop().m, "t", &picks), a);
    }

    #[test]
    fn preview_cuts_at_the_line_cap_and_keeps_decorators() {
        let repo = RepoId::from_canonical("test://pv");
        let id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "p::f");
        let graph = |code: &str, file: &str| {
            let mut g = RepoGraph {
                repo,
                nodes: vec![Node {
                    id,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![pos(file, 3, 20), text(cell_type::CODE, code)],
                }],
                edges: Vec::new(),
                nav: Default::default(),
                symbols: Default::default(),
                unresolved_calls: Vec::new(),
                unresolved_refs: Vec::new(),
                properties: HashSet::new(),
            };
            g.nav.record(id, "f", "p::f", node_kind::FUNCTION, None);
            MergedGraph::new(vec![g])
        };
        // A decorator line ends in `)`: the cut runs on to the `def` line.
        let m = graph("@app.get(\"/x\")\ndef f():\n    return 1\n", "p.py");
        assert_eq!(
            render_node(&m, id, Fidelity::Preview).as_deref(),
            Some("### p::f (FUNCTION p.py:4-21)\n```python\n@app.get(\"/x\")\ndef f():\n…\n```\n")
        );
        // No line in the first six opens a body: six lines, then `…`.
        let long =
            "fn f(\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n    e: u8,\n    g: u8,\n) {\n}";
        let m = graph(long, "src/p.rs");
        let want = "### p::f (FUNCTION src/p.rs:4-21)\n```rust\nfn f(\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n    e: u8,\n…\n```\n";
        assert_eq!(
            render_node(&m, id, Fidelity::Preview).as_deref(),
            Some(want)
        );
        // A one-line body: no `…`. A multi-byte line is cut whole.
        let m = graph("const f = () => \"héllo ✓\";", "p.mjs");
        assert_eq!(
            render_node(&m, id, Fidelity::Preview).as_deref(),
            Some("### p::f (FUNCTION p.mjs:4-21)\n```\nconst f = () => \"héllo ✓\";\n```\n")
        );
    }

    #[test]
    fn a_body_holding_a_fence_gets_a_longer_one() {
        assert_eq!(fence_for(&["plain"]), "```");
        assert_eq!(fence_for(&["# doc", "```rust", "x", "```"]), "````");
        assert_eq!(fence_for(&["a ````` b"]), "``````");
    }

    #[test]
    fn links_skip_structure_and_dedupe() {
        let mut s = shop();
        // A duplicate CALLS edge (one in each list) and a CO_CHANGES edge.
        s.m.cross_edges
            .push(edge(s.place, s.price, edge_category::CALLS));
        s.m.cross_edges
            .push(edge(s.price, s.place, edge_category::CO_CHANGES));
        s.m.cross_edges
            .push(edge(s.module, s.place, edge_category::CALLS));
        let picks = [
            Pick {
                id: s.module,
                fidelity: Fidelity::Outline,
            },
            Pick {
                id: s.price,
                fidelity: Fidelity::Outline,
            },
            Pick {
                id: s.place,
                fidelity: Fidelity::Outline,
            },
            // A repeat of an id: the first pick wins.
            Pick {
                id: s.price,
                fidelity: Fidelity::Full,
            },
        ];
        let pack = render_pack(&s.m, "  two\n words ", &picks);
        assert_eq!(
            pack,
            "# two words\n\n## outline\n- shop::a (MODULE shop/a.py:1)\n- shop::a::price (FUNCTION shop/a.py:1)\n- shop::a::place (FUNCTION shop/a.py:8)\n\n## links\nshop::a -CALLS-> shop::a::place\nshop::a::place -CALLS-> shop::a::price\n"
        );
    }

    #[test]
    fn rungs_order_by_cost_and_name_themselves() {
        let names: Vec<&str> = Fidelity::LADDER.iter().map(|f| f.name()).collect();
        assert_eq!(names, ["qname", "outline", "preview", "full"]);
        assert!(Fidelity::LADDER.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(lang_word("a/b/c.TSX"), "typescript");
        assert_eq!(lang_word("Makefile"), "");
        assert_eq!(lang_word("x.hpp"), "cpp");
    }
}
