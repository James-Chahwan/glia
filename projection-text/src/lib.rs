//! glia-projection-text — dense text projection of a `RepoGraph` or
//! `MergedGraph`, following the sigil notation in `reference_format_spec.md`.
//!
//! v0.4.7 compression:
//! - `[SCOPES]` — common qname prefixes abbreviated (e.g. `SC = Server::Controllers`)
//! - `[DEFAULTS]` — majority kind/confidence declared once, nodes only emit deviations
//! - Module file collapse — multi-file modules render `:files` instead of N×`:code`+`:position`

pub mod composition;

/// The fidelity ladder (CC.4a): one node at full / preview / outline / qname,
/// a pack renderer over `(node, fidelity)` picks, and `code_text`, the CODE
/// text accessor the ladder and `render_prose` share.
pub mod ladder;

/// The synth passes as `activation::plan::SynthHook`s (LD.12b): the access-path
/// hook is ungated, the research-only ones sit behind `research`.
pub mod hooks;

#[cfg(feature = "research")]
pub mod driver_utils;

#[cfg(feature = "research")]
pub mod passes;

/// Refactored synth pass entry points (called both by the bins and
/// in-process by glia-3d's Inject scene).
#[cfg(feature = "research")]
pub mod synth_callsite_argflow;

/// Research synth passes as `SynthHook`s whose inputs (issue text, test
/// patch, seeds file) a research driver fills in: key symbols (LD.12d).
#[cfg(feature = "research")]
pub mod research;

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{
    CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId,
};
use glia_graph::roles::roles_in;
use glia_graph::{MergedGraph, RepoGraph};

const LEGEND: &str = "\
[LEGEND]
> depends    * entry point    @ external";

/// JSON string-body escaper for the hand-rolled projections (audit 2026-06-10
/// #16). Covers everything RFC 8259 requires: quote, backslash, and EVERY
/// control character below 0x20 — the four-`replace` version in `py` missed
/// 0x00-0x08, 0x0B, 0x0C and 0x0E-0x1F, any one of which makes `json.loads`
/// raise `Invalid control character` and takes the whole graph down with it.
///
/// Returns the escaped *body* only: the caller supplies the surrounding quotes.
pub fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // Writing to a String is infallible, but the Result must be
            // consumed and CODE_RULES forbids unwrap() in non-test code.
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Render a single-repo graph. Code cells are one-line previews.
pub fn render_repo_graph(g: &RepoGraph) -> String {
    let slice: &[&RepoGraph] = &[g];
    render(slice, &[], false)
}

/// Render a single-repo graph with full cell bodies (no one-line truncation).
/// Heavier output but preserves actual source — callers doing LLM context
/// construction want this; human-readable dumps want the default.
pub fn render_repo_graph_full(g: &RepoGraph) -> String {
    let slice: &[&RepoGraph] = &[g];
    render(slice, &[], true)
}

/// Render a merged multi-repo graph with cross-repo edges (HTTP_CALLS, etc.).
pub fn render_merged(m: &MergedGraph) -> String {
    let graphs: Vec<&RepoGraph> = m.graphs.iter().collect();
    render(&graphs, &m.cross_edges, false)
}

/// Same as [`render_merged`] but preserves full cell bodies.
pub fn render_merged_full(m: &MergedGraph) -> String {
    let graphs: Vec<&RepoGraph> = m.graphs.iter().collect();
    render(&graphs, &m.cross_edges, true)
}

/// Human-readable prose projection (WP-C / GR-3): one short block per node —
/// `KIND qname [file:start-end]` followed by up to 3 lines of its doc (or code
/// preview). Pair with [`glia_graph::MergedGraph::subset`] to render a
/// ranked slice from `activate` as a primed prose anchor instead of the whole
/// graph.
pub fn render_prose(m: &MergedGraph) -> String {
    let mut out = String::new();
    for g in &m.graphs {
        for n in &g.nodes {
            let qname = g.nav.qname_by_id.get(&n.id).map(|s| s.as_str()).unwrap_or("");
            let kind = g
                .nav
                .kind_by_id
                .get(&n.id)
                .map(|k| node_kind::name(*k))
                .unwrap_or("NODE");
            let loc = node_position(n)
                .map(|p| format!(" [{}:{}-{}]", p.file, p.start_line + 1, p.end_line + 1))
                .unwrap_or_default();
            let _ = writeln!(out, "{kind} {qname}{loc}");
            if let Some(text) = node_doc_or_code(n) {
                for line in text.lines().take(3) {
                    let _ = writeln!(out, "    {}", line.trim_end());
                }
            }
            out.push('\n');
        }
    }
    out
}

/// Doc text for a node if it has one (its first `Text` DOC cell), else its
/// code, read through [`ladder::code_text`] (so a CD.7c `code_span` payload is
/// no code here).
fn node_doc_or_code(node: &glia_core::Node) -> Option<&str> {
    ladder::doc_text(node).or_else(|| ladder::code_text(node))
}

fn render(graphs: &[&RepoGraph], cross_edges: &[Edge], full_bodies: bool) -> String {
    let scopes = build_scopes(graphs);
    let defaults = compute_defaults(graphs);

    let mut out = String::new();
    out.push_str(LEGEND);
    out.push('\n');

    if !scopes.is_empty() {
        out.push('\n');
        out.push_str("[SCOPES]\n");
        for (alias, prefix) in &scopes {
            let _ = writeln!(out, "{alias} = {prefix}");
        }
    }

    if defaults.kind.is_some() || defaults.confidence.is_some() {
        out.push('\n');
        out.push_str("[DEFAULTS]\n");
        if let Some(k) = defaults.kind {
            let _ = writeln!(out, ":kind       {}", kind_name(k));
        }
        if let Some(c) = defaults.confidence {
            let _ = writeln!(out, ":confidence {}", confidence_name(c));
        }
    }

    let entries = entry_nodes(graphs);
    out.push('\n');
    render_topology(&mut out, graphs, cross_edges, &scopes, &entries);
    out.push('\n');
    render_nodes(&mut out, graphs, &scopes, &defaults, full_bodies);
    out
}

/// The nodes the `*` sigil marks: the code domain's ONE entrypoint set
/// (LD.6), `CODE_TABLES.entry` over each node's kind, name and roles (read
/// through `roles_in`, so an `@Component` CLASS is marked like a COMPONENT) —
/// the same rule liveness seeds from, so `*` never disagrees with a `live`
/// flag's roots. A node carrying an ENTRYPOINT cell (LF.3b, declared in
/// `.glia/overlay.toml` `[entrypoints]`) is marked whatever its kind, as
/// liveness seeds from it too. Before LD.6 the sigil kept its own `ROUTE | ENDPOINT`
/// list: an ENDPOINT is the CLIENT side of an HTTP call (outbound, where this
/// code makes the request), never an entry, and every other inbound handler
/// went unmarked.
fn entry_nodes(graphs: &[&RepoGraph]) -> HashSet<NodeId> {
    graphs
        .iter()
        .flat_map(|g| g.nodes.iter().filter(|n| is_entry_node(g, n)).map(|n| n.id))
        .collect()
}

/// Is `n` (a node of `g`) an entrypoint? See [`entry_nodes`].
fn is_entry_node(g: &RepoGraph, n: &Node) -> bool {
    let kind = g.nav.kind_by_id.get(&n.id).copied();
    let name = g.nav.name_by_id.get(&n.id).map(String::as_str).unwrap_or("");
    CODE_TABLES.entry.is_entry(kind, name, &roles_in(kind, &n.cells))
        || n.cells.iter().any(|c| c.kind == cell_type::ENTRYPOINT)
}

// ============================================================================
// Scopes — common qname prefix abbreviation
// ============================================================================

const MAX_SCOPES: usize = 25;
const MIN_SCOPE_USES: usize = 3;
const MIN_SCOPE_LEN: usize = 10;

fn build_scopes(graphs: &[&RepoGraph]) -> Vec<(String, String)> {
    let mut prefix_counts: HashMap<&str, usize> = HashMap::new();

    for g in graphs {
        for q in g.nav.qname_by_id.values() {
            if let Some(idx) = q.rfind("::") {
                let prefix = &q[..idx];
                if prefix.len() >= MIN_SCOPE_LEN {
                    *prefix_counts.entry(prefix).or_default() += 1;
                }
            }
        }
    }

    let mut candidates: Vec<(&str, usize)> = prefix_counts
        .into_iter()
        .filter(|(_, c)| *c >= MIN_SCOPE_USES)
        .collect();

    // Tiebreak on the prefix string: candidates arrive in HashMap order
    // (per-process seed), and a stable sort on savings alone let score ties
    // flap which prefixes make the MAX_SCOPES cut — and the alias-collision
    // numbering — across processes (audit 2026-06-10).
    candidates.sort_by_key(|&(p, c)| {
        let seg_count = p.matches("::").count() + 1;
        let alias_len = if seg_count == 1 { 2 } else { seg_count };
        let legend_cost = p.len() + alias_len + 3;
        let gross = c * (p.len() - alias_len);
        (std::cmp::Reverse(gross.saturating_sub(legend_cost)), p)
    });

    let mut used: HashSet<String> = HashSet::new();
    let mut scopes = Vec::new();

    for (prefix, _) in candidates.iter().take(MAX_SCOPES) {
        let alias = make_alias(prefix, &used);
        used.insert(alias.clone());
        scopes.push((alias, prefix.to_string()));
    }

    // Longer prefixes first so abbreviate() matches greedily; tiebreak on the
    // prefix itself so equal lengths are ordered deterministically.
    scopes.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.1.cmp(&b.1)));
    scopes
}

fn make_alias(prefix: &str, used: &HashSet<String>) -> String {
    let segments: Vec<&str> = prefix.split("::").collect();

    let base: String = if segments.len() == 1 {
        let s = segments[0];
        let mut chars = s.chars();
        let first = chars.next().unwrap_or('X').to_ascii_uppercase();
        let second = chars.next().unwrap_or('x').to_ascii_lowercase();
        format!("{first}{second}")
    } else {
        segments
            .iter()
            .filter_map(|s| s.chars().next())
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };

    if !used.contains(&base) {
        return base;
    }

    if let Some(last) = segments.last()
        && let Some(c2) = last.chars().nth(1)
    {
        let extended = format!("{base}{}", c2.to_ascii_lowercase());
        if !used.contains(&extended) {
            return extended;
        }
    }

    for i in 2..=99 {
        let numbered = format!("{base}{i}");
        if !used.contains(&numbered) {
            return numbered;
        }
    }

    base
}

fn abbreviate(qname: &str, scopes: &[(String, String)]) -> String {
    for (alias, prefix) in scopes {
        if let Some(rest) = qname.strip_prefix(prefix.as_str()) {
            if rest.starts_with("::") {
                return format!("{alias}{rest}");
            }
            if rest.is_empty() {
                return alias.clone();
            }
        }
    }
    qname.to_string()
}

// ============================================================================
// Defaults — majority kind/confidence declared once
// ============================================================================

struct Defaults {
    kind: Option<NodeKindId>,
    confidence: Option<Confidence>,
}

fn compute_defaults(graphs: &[&RepoGraph]) -> Defaults {
    let mut kind_counts: HashMap<NodeKindId, usize> = HashMap::new();
    let mut conf_counts: HashMap<Confidence, usize> = HashMap::new();
    let mut total = 0usize;

    for g in graphs {
        for n in &g.nodes {
            total += 1;
            if let Some(k) = g.nav.kind_by_id.get(&n.id) {
                *kind_counts.entry(*k).or_default() += 1;
            }
            *conf_counts.entry(n.confidence).or_default() += 1;
        }
    }

    let threshold = total / 2;

    Defaults {
        kind: kind_counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .filter(|(_, c)| *c > threshold)
            .map(|(k, _)| k),
        confidence: conf_counts
            .into_iter()
            .max_by_key(|(_, c)| *c)
            .filter(|(_, c)| *c > threshold)
            .map(|(c, _)| c),
    }
}

// ============================================================================
// Topology block
// ============================================================================

fn render_topology(
    out: &mut String,
    graphs: &[&RepoGraph],
    cross_edges: &[Edge],
    scopes: &[(String, String)],
    entries: &HashSet<NodeId>,
) {
    out.push_str("[TOPOLOGY]\n");
    let mut lines: Vec<String> = Vec::new();

    for g in graphs {
        for e in &g.edges {
            if !is_depends_category(e.category) {
                continue;
            }
            if let Some(line) = edge_line(graphs, g, e, scopes, entries) {
                lines.push(line);
            }
        }
    }

    for e in cross_edges {
        if !is_depends_category(e.category) {
            continue;
        }
        let Some(from_g) = find_owning_graph(graphs, e.from) else {
            continue;
        };
        if let Some(line) = edge_line(graphs, from_g, e, scopes, entries) {
            lines.push(line);
        }
    }

    lines.sort();
    lines.dedup();
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
}

fn edge_line(
    graphs: &[&RepoGraph],
    src_g: &RepoGraph,
    e: &Edge,
    scopes: &[(String, String)],
    entries: &HashSet<NodeId>,
) -> Option<String> {
    let src_qname = src_g.nav.qname_by_id.get(&e.from)?.as_str();
    let dst_qname = lookup_qname(graphs, e.to);

    let mut line = String::new();
    line.push_str(&abbreviate(src_qname, scopes));
    if entries.contains(&e.from) {
        line.push_str(" *");
    }
    line.push_str(" > ");
    match dst_qname {
        Some(q) => line.push_str(&abbreviate(q, scopes)),
        None => {
            let _ = write!(&mut line, "@unresolved#{:x}", e.to.0);
        }
    }
    Some(line)
}

fn lookup_qname<'a>(graphs: &'a [&RepoGraph], id: NodeId) -> Option<&'a str> {
    for g in graphs {
        if let Some(q) = g.nav.qname_by_id.get(&id) {
            return Some(q.as_str());
        }
    }
    None
}

fn find_owning_graph<'a>(graphs: &'a [&'a RepoGraph], id: NodeId) -> Option<&'a RepoGraph> {
    graphs
        .iter()
        .copied()
        .find(|g| g.nav.qname_by_id.contains_key(&id))
}

fn is_depends_category(c: EdgeCategoryId) -> bool {
    c == edge_category::CALLS
        || c == edge_category::HANDLED_BY
        || c == edge_category::HTTP_CALLS
}

// ============================================================================
// Node blocks
// ============================================================================

fn render_nodes(
    out: &mut String,
    graphs: &[&RepoGraph],
    scopes: &[(String, String)],
    defaults: &Defaults,
    full_bodies: bool,
) {
    let mut items: Vec<(&str, &Node, &RepoGraph)> = Vec::new();
    for g in graphs {
        for n in &g.nodes {
            if let Some(q) = g.nav.qname_by_id.get(&n.id) {
                items.push((q.as_str(), n, *g));
            }
        }
    }
    items.sort_by_key(|(q, _, _)| *q);

    for (qname, node, g) in items {
        render_node_block(out, qname, node, g, scopes, defaults, full_bodies);
        out.push('\n');
    }
}

fn render_node_block(
    out: &mut String,
    qname: &str,
    n: &Node,
    g: &RepoGraph,
    scopes: &[(String, String)],
    defaults: &Defaults,
    full_bodies: bool,
) {
    out.push('[');
    out.push_str(&abbreviate(qname, scopes));
    out.push(']');
    let kind = g.nav.kind_by_id.get(&n.id).copied();
    if is_entry_node(g, n) {
        out.push_str(" *");
    }
    out.push('\n');

    if let Some(k) = kind
        && defaults.kind != Some(k)
    {
        let _ = writeln!(out, ":kind       {}", kind_name(k));
    }
    if defaults.confidence != Some(n.confidence) {
        let _ = writeln!(out, ":confidence {}", confidence_name(n.confidence));
    }

    if kind == Some(node_kind::MODULE) && has_multi_file_code(n) {
        render_module_files(out, n, full_bodies);
    } else {
        for cell in &n.cells {
            render_cell(out, cell, full_bodies);
        }
    }
}

fn has_multi_file_code(n: &Node) -> bool {
    n.cells
        .iter()
        .filter(|c| c.kind == cell_type::CODE)
        .count()
        > 1
}

fn render_module_files(out: &mut String, n: &Node, full_bodies: bool) {
    let mut files: Vec<String> = Vec::new();
    let mut other_cells: Vec<&glia_core::Cell> = Vec::new();

    for cell in &n.cells {
        if cell.kind == cell_type::POSITION
            && let CellPayload::Json(j) = &cell.payload
            && let Some(file) = extract_filename(j)
        {
            files.push(file);
            continue;
        }
        if cell.kind == cell_type::CODE {
            continue;
        }
        other_cells.push(cell);
    }

    if !files.is_empty() {
        let _ = writeln!(out, ":files      {}", files.join(", "));
    }
    for cell in other_cells {
        render_cell(out, cell, full_bodies);
    }
}

fn extract_filename(json: &str) -> Option<String> {
    let marker = "\"file\":\"";
    let start = json.find(marker)? + marker.len();
    let end = json[start..].find('"')? + start;
    let path = &json[start..end];
    path.rsplit('/').next().map(|s| s.to_string())
}

fn render_cell(out: &mut String, cell: &glia_core::Cell, full_bodies: bool) {
    let label = cell_label(cell.kind);
    if cell.kind == cell_type::POSITION
        && let CellPayload::Json(j) = &cell.payload
    {
        let _ = writeln!(out, ":{:<10} {}", label, compact_position(j));
        return;
    }
    match &cell.payload {
        CellPayload::Text(t) => {
            if full_bodies {
                write_cell_block(out, label, t);
            } else {
                let _ = writeln!(out, ":{:<10} {}", label, one_line_preview(t));
            }
        }
        CellPayload::Json(j) => {
            if full_bodies {
                write_cell_block(out, label, j);
            } else {
                let _ = writeln!(out, ":{:<10} {}", label, one_line_preview(j));
            }
        }
        CellPayload::Bytes(b) => {
            let _ = writeln!(out, ":{:<10} <{} bytes>", label, b.len());
        }
    }
}

fn write_cell_block(out: &mut String, label: &str, body: &str) {
    // Multi-line block: header line, indented body, blank terminator.
    // Keeps the label discoverable; body stays verbatim so downstream LLMs
    // see actual source, not a truncated signature.
    let _ = writeln!(out, ":{label}");
    for line in body.lines() {
        let _ = writeln!(out, "  {line}");
    }
}

fn compact_position(json: &str) -> String {
    let file = extract_json_str(json, "file").unwrap_or_default();
    let start = extract_json_num(json, "start_line").unwrap_or(0);
    let end = extract_json_num(json, "end_line").unwrap_or(0);
    if end > start {
        format!("{file}:{start}-{end}")
    } else {
        format!("{file}:{start}")
    }
}

/// A node's source location, parsed from its POSITION cell. `start_line` /
/// `end_line` are 0-based tree-sitter rows exactly as stored — callers that
/// want 1-based (e.g. the pyo3 `nodes_json` surface, GR-1) add 1.
pub struct NodePosition {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Parse the POSITION cell off a node into a typed [`NodePosition`], or `None`
/// for nodes with no source span (synthetic / cross-stack endpoints). Shared so
/// every consumer reads spans the same way instead of re-scraping JSON.
pub fn node_position(node: &glia_core::Node) -> Option<NodePosition> {
    let json = node.cells.iter().find_map(|c| match &c.payload {
        glia_core::CellPayload::Json(j) if c.kind == cell_type::POSITION => {
            Some(j.as_str())
        }
        _ => None,
    })?;
    Some(NodePosition {
        file: extract_json_str(json, "file")?.to_string(),
        start_line: extract_json_num(json, "start_line")?,
        end_line: extract_json_num(json, "end_line")?,
    })
}

fn extract_json_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\":\"");
    let start = json.find(&marker)? + marker.len();
    let end = json[start..].find('"')? + start;
    Some(&json[start..end])
}

fn extract_json_num(json: &str, key: &str) -> Option<u32> {
    let marker = format!("\"{key}\":");
    let start = json.find(&marker)? + marker.len();
    let num_str: String = json[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    num_str.parse().ok()
}

// ============================================================================
// Label / name helpers
// ============================================================================

fn cell_label(c: CellTypeId) -> &'static str {
    match c.0 {
        1 => "code",
        2 => "doc",
        3 => "position",
        4 => "intent",
        5 => "method",
        6 => "hit",
        7 => "test",
        8 => "attn",
        9 => "fail",
        10 => "constraint",
        11 => "decision",
        12 => "env",
        13 => "conv",
        14 => "vector",
        // A12.1 — cell_type::MESSAGE_TYPE, the payload type on a queue node.
        // ORIGIN / IMPORTS (15 / 16) deliberately stay on the `cell` arm.
        17 => "msgtype",
        // LF.6c — cell_type::COVERAGE, lcov line counts {"hit","lines"}.
        23 => "coverage",
        // LF.3b — cell_type::ENTRYPOINT, a `.glia/overlay.toml` [entrypoints]
        // declaration {"source","pattern","decl"}.
        24 => "entry",
        _ => "cell",
    }
}

fn kind_name(k: NodeKindId) -> &'static str {
    match k.0 {
        1 => "Module",
        2 => "Class",
        3 => "Function",
        4 => "Method",
        5 => "Route",
        6 => "Package",
        7 => "Interface",
        8 => "Struct",
        9 => "Endpoint",
        // Was a flat "Node" for every id >9 — stale for components, services,
        // data entities, regions, doc sections, state vars, etc. Fall back to
        // the canonical code-domain name (WP-I) so new kinds never read "Node".
        _ => node_kind::name(k),
    }
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

fn one_line_preview(s: &str) -> String {
    const MAX: usize = 120;
    let first = s.lines().next().unwrap_or("");
    let truncated: String = first.chars().take(MAX).collect();
    let multi_line = s.contains('\n');
    let over_len = first.chars().count() > MAX;
    if multi_line || over_len {
        format!("{truncated}…")
    } else {
        truncated
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::{Cell, RepoId};

    /// L0.1 guard — `cell_label_arms_are_registered`. `cell_label` matches raw
    /// cell numbers, so an arm written from a stale packet number (the leap
    /// texts proposed COVERAGE 27 / ENTRYPOINT 28; L0.1 allocated 23 / 24)
    /// would label an id nothing emits. Every arm that is not the `cell`
    /// fallback must name a registered `cell_type`.
    #[test]
    fn cell_label_arms_are_registered() {
        for id in 0..=255u32 {
            let c = CellTypeId(id);
            if cell_label(c) != "cell" {
                assert_ne!(
                    cell_type::name(c),
                    "UNKNOWN",
                    "cell_label has an arm for unregistered cell id {id}"
                );
            }
        }
    }

    fn mini_graph() -> RepoGraph {
        let repo = RepoId::from_canonical("test://mini");
        let mod_id = NodeId::from_parts("code", repo, node_kind::MODULE, "m::a");
        let fn_id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "m::a::f");

        let mut g = RepoGraph {
            repo,
            nodes: vec![
                Node {
                    id: mod_id,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![],
                },
                Node {
                    id: fn_id,
                    repo,
                    confidence: Confidence::Medium,
                    cells: vec![Cell {
                        kind: CellTypeId(1),
                        payload: CellPayload::Text("fn f() {}".into()),
                    }],
                },
            ],
            edges: vec![Edge {
                from: mod_id,
                to: fn_id,
                category: edge_category::CALLS,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            }],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        g.nav.record(mod_id, "a", "m::a", node_kind::MODULE, None);
        g.nav
            .record(fn_id, "f", "m::a::f", node_kind::FUNCTION, Some(mod_id));
        g
    }

    #[test]
    fn node_position_parses_position_cell() {
        let repo = RepoId::from_canonical("test://pos");
        let id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "m::f");
        let with_pos = Node {
            id,
            repo,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(
                    r#"{"file":"src/a.ts","start_line":41,"end_line":87}"#.into(),
                ),
            }],
        };
        let p = node_position(&with_pos).expect("position parsed");
        assert_eq!(p.file, "src/a.ts");
        assert_eq!(p.start_line, 41); // 0-based as stored; pyo3 emits +1 = 42
        assert_eq!(p.end_line, 87);

        // No POSITION cell → None (synthetic / cross-stack endpoints).
        let no_pos = Node { id, repo, confidence: Confidence::Strong, cells: vec![] };
        assert!(node_position(&no_pos).is_none());
    }

    #[test]
    fn render_has_legend_topology_and_node_blocks() {
        let g = mini_graph();
        let s = render_repo_graph(&g);
        assert!(s.contains("[LEGEND]"));
        assert!(s.contains("[TOPOLOGY]"));
        assert!(s.contains("m::a > m::a::f"));
        assert!(s.contains("[m::a]"));
        assert!(s.contains("[m::a::f]"));
        assert!(s.contains(":kind       Module"));
        assert!(s.contains(":kind       Function"));
        assert!(s.contains(":confidence strong"));
        assert!(s.contains(":confidence medium"));
        assert!(s.contains(":code       fn f() {}"));
    }

    #[test]
    fn entry_kind_gets_star_sigil() {
        let repo = RepoId::from_canonical("test://entry");
        let route_id = NodeId::from_parts("code", repo, node_kind::ROUTE, "route:/x");
        let fn_id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "m::h");

        let mut g = RepoGraph {
            repo,
            nodes: vec![
                Node {
                    id: route_id,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![],
                },
                Node {
                    id: fn_id,
                    repo,
                    confidence: Confidence::Strong,
                    cells: vec![],
                },
            ],
            edges: vec![Edge {
                from: route_id,
                to: fn_id,
                category: edge_category::HANDLED_BY,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            }],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        g.nav
            .record(route_id, "/x", "route:/x", node_kind::ROUTE, None);
        g.nav
            .record(fn_id, "h", "m::h", node_kind::FUNCTION, None);

        let s = render_repo_graph(&g);
        assert!(
            s.contains("route:/x * > m::h"),
            "topology missing star sigil on route: {s}"
        );
        assert!(s.contains("[route:/x] *"), "node block missing star: {s}");
    }

    #[test]
    fn unresolved_target_renders_as_external() {
        let repo = RepoId::from_canonical("test://ext");
        let fn_id = NodeId::from_parts("code", repo, node_kind::FUNCTION, "m::caller");
        let ghost = NodeId(0xDEAD);

        let mut g = RepoGraph {
            repo,
            nodes: vec![Node {
                id: fn_id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![Edge {
                from: fn_id,
                to: ghost,
                category: edge_category::CALLS,
                confidence: Confidence::Weak,
                cells: Vec::new(),
            }],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        g.nav
            .record(fn_id, "caller", "m::caller", node_kind::FUNCTION, None);

        let s = render_repo_graph(&g);
        assert!(
            s.contains("m::caller > @unresolved#dead"),
            "external target not rendered: {s}"
        );
    }

    #[test]
    fn merged_graph_renders_cross_repo_edges() {
        let be_repo = RepoId::from_canonical("test://be");
        let fe_repo = RepoId::from_canonical("test://fe");
        let route_id = NodeId::from_parts("code", be_repo, node_kind::ROUTE, "route:/api/x");
        let endpoint_id = NodeId::from_parts(
            "code",
            fe_repo,
            node_kind::ENDPOINT,
            "endpoint:GET:/api/x",
        );

        let mut be = RepoGraph {
            repo: be_repo,
            nodes: vec![Node {
                id: route_id,
                repo: be_repo,
                confidence: Confidence::Strong,
                cells: vec![],
            }],
            edges: vec![],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        be.nav
            .record(route_id, "/api/x", "route:/api/x", node_kind::ROUTE, None);

        let mut fe = RepoGraph {
            repo: fe_repo,
            nodes: vec![Node {
                id: endpoint_id,
                repo: fe_repo,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        fe.nav.record(
            endpoint_id,
            "/api/x",
            "endpoint:GET:/api/x",
            node_kind::ENDPOINT,
            None,
        );

        let mut merged = MergedGraph::new(vec![be, fe]);
        merged.cross_edges.push(Edge {
            from: endpoint_id,
            to: route_id,
            category: edge_category::HTTP_CALLS,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });

        let s = render_merged(&merged);
        // LD.6: the sigil marks the one entrypoint set; an ENDPOINT is the
        // client (outbound) side of the call, not an entry.
        assert!(
            s.contains("endpoint:GET:/api/x > route:/api/x"),
            "missing cross-repo topology line:\n{s}"
        );
        assert!(s.contains("[route:/api/x] *"));
        assert!(s.contains("[endpoint:GET:/api/x]\n"), "an ENDPOINT carries no star:\n{s}");
    }

    /// LD.6: the `*` sigil reads `CODE_TABLES.entry`, the rule liveness seeds
    /// from — every inbound handler kind, `main` / `test*` functions and an
    /// `@Component` CLASS (its ROLE cell) are marked; a plain function and a
    /// plain class are not.
    #[test]
    fn star_sigil_is_the_entry_table() {
        let repo = RepoId::from_canonical("test://entry-star");
        let mut nodes = Vec::new();
        let mut nav = glia_code_domain::CodeNav::default();
        let mut add = |kind, qname: &str, cells: Vec<Cell>| {
            let id = NodeId::from_parts("code", repo, kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(id, name, qname, kind, None);
            nodes.push(Node { id, repo, confidence: Confidence::Strong, cells });
        };
        let role = Cell {
            kind: cell_type::ROLE,
            payload: CellPayload::Json(r#"{"roles":["COMPONENT"]}"#.into()),
        };
        add(node_kind::QUEUE_CONSUMER, "queue_consumer:orders", vec![]);
        add(node_kind::GRAPHQL_RESOLVER, "graphql:Query.user", vec![]);
        add(node_kind::CRON_JOB, "cron:nightly", vec![]);
        add(node_kind::GRPC_SERVER, "grpc_server:Greeter", vec![]);
        add(node_kind::RPC_PROCEDURE, "rpc:eliza.Say", vec![]);
        add(node_kind::FUNCTION, "m::main", vec![]);
        add(node_kind::FUNCTION, "m::test_login", vec![]);
        add(node_kind::CLASS, "ui::Page", vec![role]);
        add(node_kind::FUNCTION, "m::helper", vec![]);
        add(node_kind::CLASS, "m::Plain", vec![]);
        let g = RepoGraph {
            repo,
            nodes,
            edges: vec![],
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        let s = render_repo_graph(&g);
        for q in [
            "queue_consumer:orders",
            "graphql:Query.user",
            "cron:nightly",
            "grpc_server:Greeter",
            "rpc:eliza.Say",
            "m::main",
            "m::test_login",
            "ui::Page",
        ] {
            assert!(s.contains(&format!("[{q}] *\n")), "{q} carries the entry star:\n{s}");
        }
        for q in ["m::helper", "m::Plain"] {
            assert!(s.contains(&format!("[{q}]\n")), "{q} carries no star:\n{s}");
        }
    }

    #[test]
    fn one_line_preview_truncates_multiline() {
        let p = one_line_preview("first\nsecond");
        assert!(p.ends_with('…'));
        assert!(p.starts_with("first"));
    }

    // --- v0.4.7 compression tests ---

    #[test]
    fn scopes_abbreviate_common_prefixes() {
        let repo = RepoId::from_canonical("test://scopes");
        let parent = NodeId::from_parts("code", repo, node_kind::MODULE, "Server::Controllers");
        let ids: Vec<NodeId> = (0..4)
            .map(|i| {
                NodeId::from_parts(
                    "code",
                    repo,
                    node_kind::FUNCTION,
                    &format!("Server::Controllers::handler_{i}"),
                )
            })
            .collect();

        let mut g = RepoGraph {
            repo,
            nodes: std::iter::once(Node {
                id: parent,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            })
            .chain(ids.iter().map(|id| Node {
                id: *id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            }))
            .collect(),
            edges: ids
                .iter()
                .map(|id| Edge {
                    from: parent,
                    to: *id,
                    category: edge_category::CALLS,
                    confidence: Confidence::Strong,
                    cells: Vec::new(),
                })
                .collect(),
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        g.nav.record(
            parent,
            "Controllers",
            "Server::Controllers",
            node_kind::MODULE,
            None,
        );
        for (i, id) in ids.iter().enumerate() {
            g.nav.record(
                *id,
                &format!("handler_{i}"),
                &format!("Server::Controllers::handler_{i}"),
                node_kind::FUNCTION,
                Some(parent),
            );
        }

        let s = render_repo_graph(&g);
        assert!(s.contains("[SCOPES]"), "missing scopes section:\n{s}");
        assert!(
            s.contains("SC = Server::Controllers"),
            "missing scope alias:\n{s}"
        );
        assert!(
            s.contains("SC::handler_0"),
            "topology not abbreviated:\n{s}"
        );
        assert!(
            s.contains("[SC::handler_0]"),
            "node block not abbreviated:\n{s}"
        );
        assert!(
            !s.contains("[Server::Controllers::handler_0]"),
            "full qname should be abbreviated:\n{s}"
        );
    }

    #[test]
    fn defaults_omit_majority_kind_and_confidence() {
        let repo = RepoId::from_canonical("test://defaults");
        let mut g = RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: vec![],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };

        // 5 strong Functions + 1 medium Module → Function and strong are defaults
        for i in 0..5 {
            let id = NodeId::from_parts("code", repo, node_kind::FUNCTION, &format!("f{i}"));
            g.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![],
            });
            g.nav
                .record(id, &format!("f{i}"), &format!("f{i}"), node_kind::FUNCTION, None);
        }
        let mod_id = NodeId::from_parts("code", repo, node_kind::MODULE, "mod");
        g.nodes.push(Node {
            id: mod_id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![],
        });
        g.nav
            .record(mod_id, "mod", "mod", node_kind::MODULE, None);

        let s = render_repo_graph(&g);
        assert!(s.contains("[DEFAULTS]"), "missing defaults:\n{s}");
        assert!(
            s.contains("[DEFAULTS]\n:kind       Function\n:confidence strong"),
            "defaults wrong:\n{s}"
        );
        // The Module node should still emit its kind (differs from default)
        assert!(
            s.contains(":kind       Module"),
            "non-default kind missing:\n{s}"
        );
        assert!(
            s.contains(":confidence medium"),
            "non-default confidence missing:\n{s}"
        );
        // Function nodes should NOT emit :kind or :confidence
        let f0_block = s.split("[f0]").nth(1).unwrap_or("");
        let f0_end = f0_block.find("\n\n").unwrap_or(f0_block.len());
        let f0_section = &f0_block[..f0_end];
        assert!(
            !f0_section.contains(":kind"),
            "default-kind function should omit :kind:\n{f0_section}"
        );
        assert!(
            !f0_section.contains(":confidence"),
            "default-confidence function should omit :confidence:\n{f0_section}"
        );
    }

    #[test]
    fn module_file_collapse() {
        let repo = RepoId::from_canonical("test://collapse");
        let mod_id = NodeId::from_parts("code", repo, node_kind::MODULE, "pkg");

        let g = RepoGraph {
            repo,
            nodes: vec![Node {
                id: mod_id,
                repo,
                confidence: Confidence::Strong,
                cells: vec![
                    Cell {
                        kind: cell_type::CODE,
                        payload: CellPayload::Text("package pkg".into()),
                    },
                    Cell {
                        kind: cell_type::POSITION,
                        payload: CellPayload::Json(
                            r#"{"file":"pkg/alpha.go","start_line":1,"end_line":50}"#.into(),
                        ),
                    },
                    Cell {
                        kind: cell_type::CODE,
                        payload: CellPayload::Text("package pkg".into()),
                    },
                    Cell {
                        kind: cell_type::POSITION,
                        payload: CellPayload::Json(
                            r#"{"file":"pkg/beta.go","start_line":1,"end_line":30}"#.into(),
                        ),
                    },
                ],
            }],
            edges: vec![],
            nav: Default::default(),
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: HashSet::new(),
        };
        let mut gg = g;
        gg.nav
            .record(mod_id, "pkg", "pkg", node_kind::MODULE, None);

        let s = render_repo_graph(&gg);
        assert!(
            s.contains(":files      alpha.go, beta.go"),
            "module files not collapsed:\n{s}"
        );
        assert!(
            !s.contains(":code       package pkg"),
            "repeated code lines should be collapsed:\n{s}"
        );
    }

    #[test]
    fn scope_alias_collision_resolved() {
        let used: HashSet<String> = ["SC".to_string()].into_iter().collect();
        let alias = make_alias("Services::chat", &used);
        assert_ne!(alias, "SC", "should not collide with existing SC");
        assert!(
            alias.starts_with("SC"),
            "should extend from initials: {alias}"
        );
    }

    #[test]
    fn coverage_cell_renders_as_coverage() {
        // LF.6c: the lcov counts read `:coverage`, not the generic `:cell`.
        assert_eq!(cell_label(cell_type::COVERAGE), "coverage");
    }

    #[test]
    fn entrypoint_cell_renders_as_entry_and_marks_the_node() {
        // LF.3b: a declared entrypoint reads `:entry`, not the generic
        // `:cell`, and its node takes the `*` sigil whatever its kind.
        assert_eq!(cell_label(cell_type::ENTRYPOINT), "entry");
        let mut g = mini_graph();
        let f = g.nodes[1].id;
        assert!(!is_entry_node(&g, &g.nodes[1]), "m::a::f is no entry by kind or name");
        g.nodes[1].cells.push(Cell {
            kind: cell_type::ENTRYPOINT,
            payload: CellPayload::Json(
                r#"{"decl":".glia/overlay.toml:4","pattern":"m::a::f","source":"config"}"#.into(),
            ),
        });
        assert!(is_entry_node(&g, &g.nodes[1]));
        assert!(entry_nodes(&[&g]).contains(&f));
        let mut out = String::new();
        render_cell(&mut out, &g.nodes[1].cells[1], false);
        assert!(out.starts_with(":entry "), "{out}");
    }

    #[test]
    fn message_type_cell_renders_as_msgtype() {
        // A12.1: without its own arm the queue payload type read `:cell`.
        assert_eq!(cell_label(cell_type::MESSAGE_TYPE), "msgtype");
        assert_eq!(cell_label(cell_type::ORIGIN), "cell");
        let cell = Cell {
            kind: cell_type::MESSAGE_TYPE,
            payload: CellPayload::Json(r#"{"type":"OrderCreated"}"#.into()),
        };
        let mut out = String::new();
        render_cell(&mut out, &cell, false);
        assert!(out.starts_with(":msgtype "), "{out:?}");
    }
}

// ============================================================================
// escape_json_string — audit 2026-06-10 #16
// ============================================================================

#[cfg(test)]
mod escape_json_string_tests {
    use super::escape_json_string;

    /// The defect: the four-`replace` escaper in `py` covered only backslash,
    /// quote, \n, \r and \t, so 0x00-0x08, 0x0B, 0x0C and 0x0E-0x1F went
    /// through raw and `json.loads` raised `Invalid control character` on the
    /// whole graph.
    #[test]
    fn escapes_all_control_characters() {
        assert_eq!(escape_json_string("a\u{1}b\u{1f}c"), r"a\u0001b\u001fc");

        // Every codepoint below 0x20 must leave as an escape, never raw.
        for c in 0u32..0x20 {
            let raw = char::from_u32(c).unwrap_or('?').to_string();
            let out = escape_json_string(&raw);
            assert!(
                out.starts_with('\\'),
                "0x{c:02x} escaped to {out:?}, expected a backslash escape"
            );
            assert!(
                !out.chars().any(|ch| (ch as u32) < 0x20),
                "0x{c:02x} left a raw control character in {out:?}"
            );
        }
    }

    #[test]
    fn escapes_quote_backslash_and_named_whitespace() {
        assert_eq!(escape_json_string("\""), r#"\""#);
        assert_eq!(escape_json_string("\\"), r"\\");
        assert_eq!(escape_json_string("\n"), r"\n");
        assert_eq!(escape_json_string("\r"), r"\r");
        assert_eq!(escape_json_string("\t"), r"\t");
        assert_eq!(escape_json_string("\u{08}"), r"\b");
        assert_eq!(escape_json_string("\u{0c}"), r"\f");
        // Combined, in a shape a real qname could take.
        assert_eq!(escape_json_string("a\"b\\c\nd"), r#"a\"b\\c\nd"#);
    }

    #[test]
    fn leaves_printable_and_non_ascii_untouched() {
        assert_eq!(escape_json_string("héllo ✓"), "héllo ✓");
        assert_eq!(
            escape_json_string("Server::Controllers::get"),
            "Server::Controllers::get"
        );
        assert_eq!(escape_json_string(""), "");
        // 0x7f (DEL) is not a JSON control character — RFC 8259 only requires
        // escaping below 0x20 — so it passes through.
        assert_eq!(escape_json_string("\u{7f}"), "\u{7f}");
    }
}
