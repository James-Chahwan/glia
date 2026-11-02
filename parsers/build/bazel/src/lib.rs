//! glia-parser-bazel: the Bazel build-target parser (0.5.7, HT.5).
//!
//! It turns the rows a `bazel query` snapshot recorded
//! (`glia_build_domain::snapshot::TargetRow`, read through `read_targets`)
//! into the build-target domain's graph ([`BuildGraph`]). It never runs
//! bazel or any process: glia-snapshots' `targets_sync` does, and writes the
//! snapshot this crate reads. Parsers extract, the domain resolves: the graph
//! it returns is the domain's raw graph, which `BUILD_PASSES` (ROLE cells,
//! the determinism sort) then runs over. It sits under the hashed `parsers`
//! root, so a change here moves PARSER_STAMP.
//!
//! What a row becomes:
//! - a `rule` row is a TARGET, a `source_file` or `generated_file` row a
//!   SOURCE (a row of any other kind is ignored: it names nothing);
//! - every TARGET and SOURCE of a main-workspace label (`//pkg:name`) is
//!   IN_PACKAGE its PACKAGE (`//pkg`, the root package `//`);
//! - each label of an attribute [`ATTR_EDGES`] names is one edge of that
//!   attribute's category, its VIA cell the sorted attribute names that
//!   declared the pair (`deps` and `embed` on one pair are one edge, VIA
//!   `deps,embed`); a label attribute the table does not name is counted
//!   ([`ParseStats::other_label_attrs`]) and dropped, never guessed;
//! - a label's end: the row it names; else an `@` / `@@` label (any external
//!   repository, bzlmod canonical names included) is an EXTERNAL; else a
//!   main-workspace label in a HAS_SRC attribute is a SOURCE file
//!   (`<workspace>/<pkg>/<name>`: a narrowed `--expr` prints no source-file
//!   rows, but a `srcs` label is still a file); anything else is an EXTERNAL
//!   whose ATTRS is `{"dangling":true}` ([`ParseStats::dangling`]).
//!
//! Cells: LOCATION `{"file","line"}` (repo-relative, 0-based) on a TARGET or
//! SOURCE whose row recorded one, and on a SOURCE read from a `srcs` label
//! (the file itself, line 0); ATTRS `{"rule_class","tags","testonly",
//! "visibility","workspace"}` on a TARGET. Both are JSON with sorted keys and
//! sorted lists. The build domain has no EVIDENCE cell: an edge's site is its
//! source target's LOCATION.
//!
//! Determinism: the rows are re-sorted by (workspace, label) (the first of
//! two rows with one key wins), nodes are added in (kind, node key) order and
//! edges sorted canonically (`glia_core::canonical_edge_cmp`, the order
//! `BUILD_PASSES`' finalize sort keeps), so any order of one row set gives
//! one graph.

use std::collections::{BTreeMap, BTreeSet};

use glia_build_domain::BuildGraph;
use glia_build_domain::cell_type::{ATTRS, LOCATION};
use glia_build_domain::edge_category::{DEPENDS_ON, HAS_SRC, IN_PACKAGE, RUNTIME_DEP};
use glia_build_domain::node_key;
use glia_build_domain::node_kind::{EXTERNAL, PACKAGE, SOURCE, TARGET};
use glia_build_domain::snapshot::{KIND_GENERATED_FILE, KIND_RULE, KIND_SOURCE_FILE, TargetRow};
use glia_core::{Cell, CellPayload, EdgeCategoryId, NodeKindId, RepoId, canonical_edge_cmp};

/// The label attributes that are edges, and the category each one draws.
/// One table: an attribute not here is dropped and counted, never guessed.
pub const ATTR_EDGES: &[(&str, EdgeCategoryId)] = &[
    ("deps", DEPENDS_ON),
    ("embed", DEPENDS_ON),
    ("actual", DEPENDS_ON),
    ("exports", DEPENDS_ON),
    ("runtime_deps", DEPENDS_ON),
    ("implementation_deps", DEPENDS_ON),
    ("data", RUNTIME_DEP),
    ("srcs", HAS_SRC),
    ("hdrs", HAS_SRC),
    ("textual_hdrs", HAS_SRC),
];

/// What one parse read and made.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Distinct workspaces among the rows.
    pub workspaces: usize,
    /// TARGET nodes.
    pub targets: usize,
    /// PACKAGE nodes.
    pub packages: usize,
    /// SOURCE nodes (rows and `srcs` labels).
    pub sources: usize,
    /// EXTERNAL nodes, the dangling ones included.
    pub externals: usize,
    /// Edges of every category.
    pub edges: usize,
    /// EXTERNAL nodes of a main-workspace label naming no row.
    pub dangling: usize,
    /// (row, attribute) pairs of a label attribute [`ATTR_EDGES`] does not
    /// name, dropped: the signal to extend the table.
    pub other_label_attrs: usize,
}

/// A parse: the graph and what it counted.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub graph: BuildGraph,
    stats: ParseStats,
}

impl Parsed {
    /// What the parse read and made.
    pub fn stats(&self) -> &ParseStats {
        &self.stats
    }
}

/// The build-target graph of `rows`, its node ids hashing `repo`.
pub fn parse_targets(repo: RepoId, rows: &[TargetRow]) -> BuildGraph {
    parse(repo, rows).graph
}

/// The kind a row is: a rule a TARGET, a file a SOURCE, anything else none.
fn row_kind(row: &TargetRow) -> Option<NodeKindId> {
    match row.kind.as_str() {
        KIND_RULE => Some(TARGET),
        KIND_SOURCE_FILE | KIND_GENERATED_FILE => Some(SOURCE),
        _ => None,
    }
}

/// `//pkg:name` -> (`pkg`, `name`); `//pkg` (no `:`) is `//pkg:<last
/// segment of pkg>`. `None` for an external (`@..`) or relative label.
fn split_main(label: &str) -> Option<(&str, &str)> {
    let rest = label.strip_prefix("//")?;
    Some(match rest.split_once(':') {
        Some((pkg, name)) => (pkg, name),
        None => (rest, rest.rsplit('/').next().unwrap_or(rest)),
    })
}

/// `//pkg:name` in its full form (the form `bazel query` prints).
fn full_label(label: &str) -> String {
    match split_main(label) {
        Some((pkg, name)) if !label.contains(':') => format!("//{pkg}:{name}"),
        _ => label.to_string(),
    }
}

/// The repo-relative file a main-workspace source label names.
fn source_path(workspace: &str, pkg: &str, name: &str) -> String {
    [workspace, pkg, name]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// A JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON array of strings, sorted and deduplicated.
fn json_list(items: &[String]) -> String {
    let set: BTreeSet<&str> = items.iter().map(String::as_str).collect();
    let parts: Vec<String> = set.into_iter().map(json_str).collect();
    format!("[{}]", parts.join(","))
}

fn json_cell(kind: glia_core::CellTypeId, json: String) -> Cell {
    Cell {
        kind,
        payload: CellPayload::Json(json),
    }
}

fn location(file: &str, line: u32) -> Cell {
    json_cell(
        LOCATION,
        format!("{{\"file\":{},\"line\":{line}}}", json_str(file)),
    )
}

/// A row's own cells: LOCATION when it recorded one, ATTRS on a rule.
fn row_cells(row: &TargetRow, kind: NodeKindId) -> Vec<Cell> {
    let mut cells = Vec::new();
    if let Some(file) = &row.file {
        cells.push(location(file, row.line.unwrap_or(0)));
    }
    if kind == TARGET {
        cells.push(json_cell(
            ATTRS,
            format!(
                "{{\"rule_class\":{},\"tags\":{},\"testonly\":{},\"visibility\":{},\"workspace\":{}}}",
                json_str(row.rule_class.as_deref().unwrap_or("")),
                json_list(&row.tags),
                row.testonly,
                json_list(&row.visibility),
                json_str(&row.workspace),
            ),
        ));
    }
    cells
}

/// One node to add: its kind, workspace, label and cells.
struct Pending {
    kind: NodeKindId,
    workspace: String,
    label: String,
    cells: Vec<Cell>,
}

/// Nodes keyed (kind, node key), the first insert winning.
#[derive(Default)]
struct Nodes(BTreeMap<(u32, String), Pending>);

impl Nodes {
    fn insert(&mut self, kind: NodeKindId, workspace: &str, label: &str, cells: Vec<Cell>) {
        self.0
            .entry((kind.0, node_key(workspace, label)))
            .or_insert_with(|| Pending {
                kind,
                workspace: workspace.to_string(),
                label: label.to_string(),
                cells,
            });
    }
}

/// An edge end: (kind id, workspace, label).
type End = (u32, String, String);

/// The graph of `rows` and what the parse counted.
///
/// fired_on marker (stderr, once per parse): `[bazel] parsed workspaces=<w>
/// targets=<t> packages=<p> sources=<s> externals=<x> edges=<e> dangling=<d>
/// other_label_attrs=<o>`
pub fn parse(repo: RepoId, rows: &[TargetRow]) -> Parsed {
    let mut sorted: Vec<&TargetRow> = rows.iter().collect();
    sorted.sort_by(|a, b| (&a.workspace, &a.label).cmp(&(&b.workspace, &b.label)));
    sorted.dedup_by(|b, a| a.workspace == b.workspace && a.label == b.label);

    // (workspace, full label) -> the kind of the row naming it.
    let index: BTreeMap<(&str, String), NodeKindId> = sorted
        .iter()
        .filter_map(|r| Some(((r.workspace.as_str(), full_label(&r.label)), row_kind(r)?)))
        .collect();

    let mut stats = ParseStats {
        workspaces: sorted
            .iter()
            .map(|r| r.workspace.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        ..ParseStats::default()
    };
    let mut nodes = Nodes::default();
    let mut dangling: BTreeSet<(String, String)> = BTreeSet::new();
    // (from, to, category) -> the attributes naming it.
    let mut edges: BTreeMap<(End, End, u32), BTreeSet<&str>> = BTreeMap::new();

    for row in &sorted {
        let Some(kind) = row_kind(row) else {
            continue;
        };
        let ws = row.workspace.as_str();
        let label = full_label(&row.label);
        nodes.insert(kind, ws, &label, row_cells(row, kind));
        let from: End = (kind.0, ws.to_string(), label.clone());
        if let Some((pkg, _)) = split_main(&label) {
            let package = format!("//{pkg}");
            nodes.insert(PACKAGE, ws, &package, Vec::new());
            edges
                .entry((
                    from.clone(),
                    (PACKAGE.0, ws.to_string(), package),
                    IN_PACKAGE.0,
                ))
                .or_default();
        }
        for (attr, labels) in &row.labels {
            let Some(&(_, category)) = ATTR_EDGES.iter().find(|(a, _)| a == attr) else {
                stats.other_label_attrs += 1;
                continue;
            };
            for to in labels {
                let to_label = full_label(to);
                let to_kind = if let Some(&k) = index.get(&(ws, to_label.clone())) {
                    k
                } else if to_label.starts_with('@') {
                    nodes.insert(EXTERNAL, ws, &to_label, Vec::new());
                    EXTERNAL
                } else if let (true, Some((pkg, name))) =
                    (category == HAS_SRC, split_main(&to_label))
                {
                    let file = source_path(ws, pkg, name);
                    nodes.insert(SOURCE, ws, &to_label, vec![location(&file, 0)]);
                    let package = format!("//{pkg}");
                    nodes.insert(PACKAGE, ws, &package, Vec::new());
                    edges
                        .entry((
                            (SOURCE.0, ws.to_string(), to_label.clone()),
                            (PACKAGE.0, ws.to_string(), package),
                            IN_PACKAGE.0,
                        ))
                        .or_default();
                    SOURCE
                } else {
                    dangling.insert((ws.to_string(), to_label.clone()));
                    nodes.insert(
                        EXTERNAL,
                        ws,
                        &to_label,
                        vec![json_cell(ATTRS, "{\"dangling\":true}".to_string())],
                    );
                    EXTERNAL
                };
                edges
                    .entry((
                        from.clone(),
                        (to_kind.0, ws.to_string(), to_label),
                        category.0,
                    ))
                    .or_default()
                    .insert(attr.as_str());
            }
        }
    }

    let mut graph = BuildGraph::new(repo);
    for p in nodes.0.into_values() {
        match p.kind {
            k if k == TARGET => stats.targets += 1,
            k if k == PACKAGE => stats.packages += 1,
            k if k == SOURCE => stats.sources += 1,
            _ => stats.externals += 1,
        }
        graph.add_node(p.kind, &p.workspace, &p.label, p.cells);
    }
    for ((from, to, category), via) in edges {
        let from = glia_build_domain::node_id(repo, NodeKindId(from.0), &from.1, &from.2);
        let to = glia_build_domain::node_id(repo, NodeKindId(to.0), &to.1, &to.2);
        let via: Vec<&str> = via.into_iter().collect();
        graph.add_edge(from, to, EdgeCategoryId(category), &via);
    }
    graph.edges.sort_by(canonical_edge_cmp);
    stats.edges = graph.edges.len();
    stats.dangling = dangling.len();

    eprintln!(
        "[bazel] parsed workspaces={} targets={} packages={} sources={} externals={} edges={} dangling={} other_label_attrs={}",
        stats.workspaces,
        stats.targets,
        stats.packages,
        stats.sources,
        stats.externals,
        stats.edges,
        stats.dangling,
        stats.other_label_attrs,
    );
    Parsed { graph, stats }
}
