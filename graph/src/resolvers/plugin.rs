//! The plugin-host pairing resolver (L3, lua-support): joins the Lua-side
//! PLUGIN_TOOL registrations to their PLUGIN manifests and to the host source
//! that loads them. The rules, in the tally's order:
//!
//! 1. **`same_dir`** (FACT). A PLUGIN's qname is `plugin:<dir> @<host>`, a
//!    PLUGIN_TOOL's `plugin_tool:<name> @<host>/<dir>` — the owner tail is
//!    the pair key. Every tool whose tail is exactly a plugin's tail in the
//!    same repo gets `PLUGIN -LOADS-> PLUGIN_TOOL`: the second hop of the
//!    load chain (the loader loads the plugin, the plugin provides the
//!    tool), on the carried LOADS category, so trace and blast radius
//!    cross both hops under one name (lua-support ruling 1; the REGISTERS
//!    alternative is recorded on `edge_category::LOADS`). A tool whose
//!    tail matches no plugin (a registration outside any manifest
//!    directory) pairs nothing and counts `ambiguous=`.
//! 2. **`manifest_literal`** (DERIVED). The host side is a runtime directory
//!    scan, so the honest anchor is the manifest *name* in source: a
//!    FUNCTION / STATE_VAR / MODULE CODE cell of a repo that holds at least
//!    one PLUGIN, containing a shipped host row's `manifest` filename
//!    (table order), `-LOADS->` every PLUGIN of that host in the repo
//!    (host → plugin, so impact flows from the loader outward). The loader
//!    loads *all* discovered plugins of its host, so loader→every-plugin is
//!    semantically true, not a guess; the tier is DERIVED because the
//!    connection is through a runtime scan the graph cannot see.
//!
//!    Anchor breadth is accepted, not accidental (lua-support ruling 2,
//!    2026-10-27). Measured on maki: 32 anchors × 30 plugins = 960 edges,
//!    ~10 runtime machinery, ~8 docs generators, ~14 tests. The edges serve
//!    both walk directions, and the directions are not equally valuable:
//!    *in-edges* from a manifest are true impact on anything that reads
//!    manifests — a permissions test fails when a manifest changes, a docs
//!    generator's output changes — so the wide net is what makes
//!    "manifest changed → what validates/consumes it" complete, and those
//!    rows rank mid-list, not buried. *Out-edges* from a peripheral anchor
//!    (docs_render changed → 30 plugins) are the noise direction: rare to
//!    edit, and ranked low by fan-out dilution plus co-change ordering.
//!    Excluding test-stamped anchors (the DT.6a precedent) was declined
//!    because its cost scales with test quality — it deletes the
//!    manifest→test rows exactly in the repos that test their contracts —
//!    while the wide net's cost scales with docs volume: low-ranked,
//!    gate-irrelevant noise. A host node (anchor → HOST → plugins, one
//!    edge per anchor) stays the structural fix if per-plugin fan-out ever
//!    matters at scale; it compresses but does not improve precision.
//!    Re-measure when a second host row ships: blast-radius from one
//!    PLUGIN seed, count the depth-1 non-PLUGIN_TOOL rows.
//! 3. **`include_str`** (FACT). A CODE cell of the same three kinds holding
//!    `include_str!("…<path>.lua")` whose path resolves, file-relative, to an
//!    existing `.lua` MODULE of the repo → `IMPORTS` from the containing
//!    file's MODULE to that MODULE (the host embedding a plugin file's
//!    source), at the include's own row.
//!
//! Same-repo only: a loader loads the plugins discovered under its own tree,
//! so a host in one repo never loads another repo's plugins. All edges are
//! cross-stack edges (`merged.cross_edges`), the resolver convention.
//! fired_on, once per resolve with any edge:
//! `[plugin] pairs=<p> same_dir=<s> loads=<l> include_str=<i> ambiguous=<a>`
//! plus the shared `[evidence-rules] resolver=plugin ..` line.
//!
//! **Detection (L4, report-only).** The parser records every call of a known
//! registration shape whose callee matched no shipped host family as a
//! `NavFact::Registration` candidate on its MODULE. This resolver groups
//! them by callee family (`maki.api.register_tool` -> `maki.api`), counts
//! them, and marks the families the graph's own PLUGIN / PLUGIN_TOOL owner
//! segments already name. It mints nothing — the report is the zero-config
//! front door: HEURISTIC by nature (content sniffing, advisory until a
//! `.glia/plugin_families.toml` row declares it), and what a future
//! `[[plugin_family]]` proposal reads. Marker (once per resolve, silent with
//! no candidates):
//!
//! ```text
//! [plugin-detect] candidates=<c> families=<f> reported=<r> singles=<s>
//! [plugin-detect] family=<f> shape=<table|positional> calls=<n> named=<m> files=<k> host=<name|->
//! ```

use std::collections::HashSet;

use glia_code_domain::{NavFact, cell_type, edge_category, evidence, node_kind, plugin_families};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, RepoId};

use super::{CrossGraphResolver, RuleTally};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

pub struct PluginResolver;

/// The `(host, dir)` pair key of a PLUGIN / PLUGIN_TOOL qname:
/// `plugin:<dir> @<host>` -> `(host, dir)` (the dir is the plugin's identity
/// segment); `plugin_tool:<name> @<host>/<dir>` -> `(host, dir)`. `None` for
/// a qname of another shape — it never pairs.
fn plugin_key<'a>(qname: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let rest = qname.strip_prefix(prefix)?;
    let (name, owner) = rest.split_once(" @")?;
    if prefix == "plugin:" {
        return Some((owner, name));
    }
    let (host, dir) = owner.split_once('/')?;
    Some((host, dir))
}

/// The scan kinds `manifest_literal` and `include_str` read: where a host's
/// source names its manifest (a loader fn, the manifest-name constant, the
/// module that owns both).
const SCAN_KINDS: [glia_core::NodeKindId; 3] =
    [node_kind::FUNCTION, node_kind::STATE_VAR, node_kind::MODULE];

impl CrossGraphResolver for PluginResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut tally = RuleTally::new("plugin", &["same_dir", "manifest_literal", "include_str"]);
        let mut edges: Vec<Edge> = Vec::new();
        let (mut same_dir, mut loads, mut include_str, mut ambiguous) =
            (0usize, 0usize, 0usize, 0usize);

        // One repo's bridge, across its language graphs: the PLUGIN nodes
        // live in the plugin-manifest graph, the PLUGIN_TOOL registrations in
        // the Lua graph, the loader's scan kinds in the host's source graph —
        // one RepoId, three graphs, so the per-repo view groups them.
        let mut repos: Vec<RepoView> = Vec::new();
        for g in &merged.graphs {
            let view = match repos.iter().position(|v| v.repo == g.repo) {
                Some(i) => &mut repos[i],
                None => {
                    repos.push(RepoView {
                        repo: g.repo,
                        plugins: Vec::new(),
                        tools: Vec::new(),
                        graphs: Vec::new(),
                        modules: std::collections::HashMap::new(),
                    });
                    let last = repos.len() - 1;
                    &mut repos[last]
                }
            };
            view.graphs.push(g);
            for n in &g.nodes {
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE) {
                    view.modules.insert(qname.clone(), n.id);
                }
                match g.nav.kind_by_id.get(&n.id) {
                    Some(&node_kind::PLUGIN) => {
                        if let Some(key) = plugin_key(qname, "plugin:") {
                            view.plugins.push((n.id, key));
                        }
                    }
                    Some(&node_kind::PLUGIN_TOOL) => {
                        if let Some(key) = plugin_key(qname, "plugin_tool:") {
                            view.tools.push((n.id, key));
                        }
                    }
                    _ => {}
                }
            }
        }

        for view in &repos {
            // 1. same_dir: the manifest's directory names the tool set
            // (ruling 1: LOADS, the load chain's second hop). A tool whose
            // key matches no plugin counts ambiguous.
            for (tool, tool_key) in &view.tools {
                let Some(plugin) = view
                    .plugins
                    .iter()
                    .find(|(_, pk)| pk == tool_key)
                    .map(|(p, _)| *p)
                else {
                    ambiguous += 1;
                    continue;
                };
                edges.push(
                    Edge::new(plugin, *tool, edge_category::LOADS, Confidence::Strong)
                        .with_cell(tally.cell("same_dir")),
                );
                same_dir += 1;
            }

            // 2 + 3. the host-source scan, over every graph of the repo,
            // deduped per (node, target): one CODE cell naming the manifest
            // twice loads each plugin once. The manifest filenames come from
            // the graph itself — the `NavFact::PluginManifest` facts the
            // manifest route recorded — so no table is read here.
            // `manifest_literal` reads only a repo that holds a PLUGIN (the
            // host side of the bridge); `include_str` is a structural fact of
            // the source alone, so it reads every repo.
            let mut manifests: Vec<(&str, &str)> = Vec::new();
            for g in &view.graphs {
                for facts in g.nav.nav_facts.values() {
                    for fact in facts {
                        let NavFact::PluginManifest { family, manifest } = fact else {
                            continue;
                        };
                        if !manifests.iter().any(|(f, m)| f == family && m == manifest) {
                            manifests.push((family, manifest));
                        }
                    }
                }
            }
            manifests.sort();
            let mut seen_loads: HashSet<(NodeId, NodeId)> = HashSet::new();
            let mut seen_includes: HashSet<(NodeId, NodeId)> = HashSet::new();
            for g in &view.graphs {
                for n in &g.nodes {
                    let Some(kind) = g.nav.kind_by_id.get(&n.id).copied() else {
                        continue;
                    };
                    if !SCAN_KINDS.contains(&kind) {
                        continue;
                    }
                    let Some(code) = code_text(&n.cells) else {
                        continue;
                    };

                    if !view.plugins.is_empty() {
                        for (family, manifest) in &manifests {
                            if !code.contains(manifest) {
                                continue;
                            }
                            for (plugin, (host_of, _)) in &view.plugins {
                                if host_of != family || !seen_loads.insert((n.id, *plugin)) {
                                    continue;
                                }
                                edges.push(
                                    Edge::new(
                                        n.id,
                                        *plugin,
                                        edge_category::LOADS,
                                        Confidence::Strong,
                                    )
                                    .with_cell(tally.cell("manifest_literal")),
                                );
                                loads += 1;
                            }
                        }
                    }

                    let Some((path, line)) = include_str_lua(code) else {
                        continue;
                    };
                    let file = evidence::locate(&n.cells).map(|(f, _)| f);
                    let Some(target) = resolve_include_target(view, n.id, &path) else {
                        continue;
                    };
                    let Some(from) = containing_module(g, n.id) else {
                        continue;
                    };
                    if !seen_includes.insert((from, target)) {
                        continue;
                    }
                    let ev = match file {
                        Some(file) => tally.evidence("include_str").at(file, line),
                        None => tally.evidence("include_str").line(line),
                    };
                    edges.push(
                        Edge::new(from, target, edge_category::IMPORTS, Confidence::Strong)
                            .with_cell(ev.to_cell()),
                    );
                    tally.add("include_str", 1);
                    include_str += 1;
                }
            }
        }

        merged.cross_edges.extend(edges);
        tally.report();
        let pairs = same_dir + loads + include_str;
        if pairs > 0 || ambiguous > 0 {
            eprintln!(
                "[plugin] pairs={pairs} same_dir={same_dir} loads={loads} include_str={include_str} ambiguous={ambiguous}"
            );
        }
        for line in detection_report(merged) {
            eprintln!("{line}");
        }
    }
}

/// One detected registration family, aggregated over the whole build (the
/// candidates are family-level signals, not per-repo facts).
struct Family {
    family: String,
    shape: plugin_families::ShapeTag,
    calls: usize,
    named: usize,
    files: HashSet<NodeId>,
    host: Option<String>,
}

/// The detection report (L4, report-only): the parser's
/// `NavFact::Registration` candidates grouped by callee family (the dotted
/// callee minus its last segment, `maki.api.register_tool` -> `maki.api`),
/// each marked with the family name the graph's own PLUGIN / PLUGIN_TOOL
/// owner segments name, when the candidate family's first dotted segment
/// matches one (`maki.api` beside `plugin:x @maki` -> `host=maki`), else
/// `-`. Families with a single candidate are summarized, not listed; the
/// rest sort by calls descending, ties by family then shape, capped at 12.
/// The first line is the summary; empty with no candidates. Pure, so tests
/// read the lines and `resolve` only prints them.
fn detection_report(merged: &MergedGraph) -> Vec<String> {
    // The owner names the graph itself declares (`@maki`, `@maki/question`):
    // the families the build already knows, read off its qnames.
    let mut owners: Vec<String> = merged
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.values())
        .filter_map(|q| q.split_once(" @").map(|(_, owner)| owner))
        .map(|owner| owner.split('/').next().unwrap_or(owner).to_string())
        .collect();
    owners.sort();
    owners.dedup();
    let mut candidates = 0usize;
    let mut groups: Vec<Family> = Vec::new();
    for g in &merged.graphs {
        for (scope, facts) in &g.nav.nav_facts {
            for fact in facts {
                let NavFact::Registration {
                    callee,
                    shape,
                    named,
                } = fact
                else {
                    continue;
                };
                candidates += 1;
                let family = match callee.rsplit_once('.') {
                    Some((prefix, _)) => prefix.to_string(),
                    None => callee.clone(),
                };
                let host = owners
                    .iter()
                    .find(|owner| family.split('.').next().is_some_and(|head| head == *owner))
                    .cloned();
                match groups
                    .iter_mut()
                    .find(|f| f.family == family && f.shape == *shape)
                {
                    Some(f) => {
                        f.calls += 1;
                        f.named += *named as usize;
                        f.files.insert(*scope);
                    }
                    None => groups.push(Family {
                        family,
                        shape: *shape,
                        calls: 1,
                        named: *named as usize,
                        files: HashSet::from([*scope]),
                        host,
                    }),
                }
            }
        }
    }
    if candidates == 0 {
        return Vec::new();
    }
    let singles = groups.iter().filter(|f| f.calls < 2).count();
    let mut reported: Vec<&Family> = groups.iter().filter(|f| f.calls >= 2).collect();
    reported.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| a.family.cmp(&b.family))
            .then_with(|| a.shape.name().cmp(b.shape.name()))
    });
    reported.truncate(12);
    let mut lines = vec![format!(
        "[plugin-detect] candidates={candidates} families={} reported={} singles={singles}",
        groups.len(),
        reported.len(),
    )];
    for f in reported {
        lines.push(format!(
            "[plugin-detect] family={} shape={} calls={} named={} files={} host={}",
            f.family,
            f.shape.name(),
            f.calls,
            f.named,
            f.files.len(),
            f.host.clone().unwrap_or_else(|| "-".to_string())
        ));
    }
    lines
}

/// One repo's bridge half: the PLUGIN and PLUGIN_TOOL nodes found across its
/// language graphs, and the graphs the host-source scan reads. Collected in
/// `merged.graphs` order, so a view's nodes and graphs are deterministic.
struct RepoView<'a> {
    repo: RepoId,
    plugins: Vec<(NodeId, (&'a str, &'a str))>,
    tools: Vec<(NodeId, (&'a str, &'a str))>,
    graphs: Vec<&'a RepoGraph>,
    /// Every MODULE qname of the repo -> its id, across the language graphs
    /// (`build_symbol_table`'s table is per graph; an include in the host's
    /// Rust graph names a `.lua` MODULE of the Lua graph).
    modules: std::collections::HashMap<String, NodeId>,
}

/// The Text CODE cell of a node, if it carries one (the source slice; a JSON
/// CODE cell is a structured payload, not source).
fn code_text(cells: &[Cell]) -> Option<&str> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Text(t) if c.kind == cell_type::CODE => Some(t.as_str()),
        _ => None,
    })
}

/// The first `include_str!("…<path>.lua")` of a source text, its path and
/// 0-based row. Only a plain literal: a `concat!` / `env!` argument is not a
/// path the graph can resolve.
fn include_str_lua(code: &str) -> Option<(String, u32)> {
    let mut scanned = 0usize;
    while let Some(at) = code[scanned..].find("include_str!") {
        let at = scanned + at;
        let after = &code[at + "include_str!".len()..];
        let paren = after.find('(')?;
        let arg = &after[paren + 1..];
        let quote = arg.find('"')?;
        let path = &arg[quote + 1..];
        let end = path.find('"')?;
        let path = &path[..end];
        if path.ends_with(".lua") {
            let line = u32::try_from(code[..at].bytes().filter(|b| *b == b'\n').count())
                .unwrap_or(u32::MAX);
            return Some((path.to_string(), line));
        }
        scanned = at + "include_str!".len();
    }
    None
}

/// The `.lua` MODULE a file-relative include path names: resolved against the
/// containing node's file directory (never escaping the repo root), the
/// result's MODULE qname looked up across the repo's graphs, and required to
/// be a `.lua` file.
fn resolve_include_target(view: &RepoView, node: NodeId, path: &str) -> Option<NodeId> {
    if path.starts_with('/') {
        return None;
    }
    let cells = view.cells(node)?;
    let (file, _) = evidence::locate(cells)?;
    let dir = match file.rsplit_once('/') {
        Some((d, _)) => d,
        None => "",
    };
    let resolved = resolve_relative(dir, path)?;
    let qname = resolved.strip_suffix(".lua")?.replace('/', "::");
    let target = view.modules.get(&qname).copied()?;
    is_lua_module(view, target).then_some(target)
}

impl<'a> RepoView<'a> {
    /// The cells a node carries, in whichever of the repo's graphs holds it.
    fn cells(&self, node: NodeId) -> Option<&'a [Cell]> {
        self.graphs
            .iter()
            .find_map(|g| Some(g.nodes.iter().find(|n| n.id == node)?.cells.as_slice()))
    }
}

/// `dir + "./../x.lua"` without the segments that cancel: `..` past the root
/// gives up (an include that escapes the repo names nothing here).
fn resolve_relative(dir: &str, path: &str) -> Option<String> {
    let joined = format!("{dir}/{path}");
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// The MODULE node that contains `node`: itself when it is a MODULE, else the
/// MODULE its POSITION file names. `None` when neither exists — an include
/// with no container anchors nothing.
fn containing_module(g: &RepoGraph, node: NodeId) -> Option<NodeId> {
    if g.nav.kind_by_id.get(&node) == Some(&node_kind::MODULE) {
        return Some(node);
    }
    // The container is the node's own graph: a file's MODULE and its
    // functions live in one language graph.
    let cells = g.nodes.iter().find(|n| n.id == node)?.cells.as_slice();
    let (file, _) = evidence::locate(cells)?;
    let (stem, _) = file.rsplit_once('.')?;
    g.symbols
        .module_by_qname
        .get(&stem.replace('/', "::"))
        .copied()
}

/// True when `id`'s POSITION file is a `.lua` file — the guard that keeps a
/// same-qname MODULE of another language from answering an include.
fn is_lua_module(view: &RepoView, id: NodeId) -> bool {
    match view.cells(id).and_then(evidence::locate) {
        Some((f, _)) => f
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("lua")),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SymbolTable;
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Node, RepoId};

    fn graph(tag: &str) -> RepoGraph {
        let repo = RepoId::from_canonical(&format!("test://{tag}"));
        RepoGraph {
            repo,
            nodes: Vec::new(),
            edges: Vec::new(),
            nav: CodeNav::default(),
            symbols: SymbolTable::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        }
    }

    /// Record `(kind, qname)` in `g`, nav name `name`, with cells. A MODULE
    /// qname also enters `module_by_qname`, as `build_symbol_table` would.
    fn add(
        g: &mut RepoGraph,
        kind: glia_core::NodeKindId,
        qname: &str,
        name: &str,
        parent: Option<NodeId>,
        cells: Vec<Cell>,
    ) -> NodeId {
        let id = NodeId::from_parts(GRAPH_TYPE, g.repo, kind, qname);
        g.nav.record(id, name, qname, kind, parent);
        g.nodes.push(Node {
            id,
            repo: g.repo,
            confidence: Confidence::Strong,
            cells,
        });
        if kind == node_kind::MODULE {
            g.symbols.module_by_qname.insert(qname.to_string(), id);
        }
        id
    }

    fn position(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":0,"end_line":9}}"#
            )),
        }
    }

    fn code(text: &str) -> Cell {
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text.to_string()),
        }
    }

    fn resolve(graphs: Vec<RepoGraph>) -> Vec<Edge> {
        let mut merged = MergedGraph::new(graphs);
        PluginResolver.resolve(&mut merged);
        merged.cross_edges
    }

    fn edges_of(edges: &[Edge], category: glia_core::EdgeCategoryId) -> Vec<(NodeId, NodeId)> {
        edges
            .iter()
            .filter(|e| e.category == category)
            .map(|e| (e.from, e.to))
            .collect()
    }

    fn evidence_rule(edges: &[Edge], category: glia_core::EdgeCategoryId) -> Vec<String> {
        edges
            .iter()
            .filter(|e| e.category == category)
            .filter_map(|e| evidence::Evidence::read(&e.cells).and_then(|ev| ev.rule))
            .collect()
    }

    #[test]
    fn same_dir_pairs_a_plugin_with_its_tools() {
        let mut g = graph("maki");
        let plugin = add(
            &mut g,
            node_kind::PLUGIN,
            "plugin:question @maki",
            "question",
            None,
            vec![position("plugins/question/plugin.toml")],
        );
        let tool = add(
            &mut g,
            node_kind::PLUGIN_TOOL,
            "plugin_tool:ask @maki/question",
            "ask",
            None,
            vec![],
        );
        let edges = resolve(vec![g]);
        assert_eq!(edges_of(&edges, edge_category::LOADS), vec![(plugin, tool)]);
        assert_eq!(
            evidence_rule(&edges, edge_category::LOADS),
            vec!["same_dir"]
        );
    }

    #[test]
    fn a_tool_outside_any_manifest_counts_ambiguous() {
        let mut g = graph("maki");
        add(
            &mut g,
            node_kind::PLUGIN_TOOL,
            "plugin_tool:lonely @maki/orphan",
            "lonely",
            None,
            vec![],
        );
        let edges = resolve(vec![g]);
        assert!(edges.is_empty(), "an unmatched tool pairs nothing");
    }

    #[test]
    fn same_basename_tools_of_two_hosts_stay_apart() {
        // Two hosts' plugins with one directory basename: the owner segment
        // is what keeps the tails distinct.
        let mut maki = graph("maki");
        let maki_plugin = add(
            &mut maki,
            node_kind::PLUGIN,
            "plugin:x @maki",
            "x",
            None,
            vec![],
        );
        let maki_tool = add(
            &mut maki,
            node_kind::PLUGIN_TOOL,
            "plugin_tool:t @maki/x",
            "t",
            None,
            vec![],
        );
        let mut toy = graph("toy");
        let toy_plugin = add(
            &mut toy,
            node_kind::PLUGIN,
            "plugin:x @toy",
            "x",
            None,
            vec![],
        );
        let edges = resolve(vec![maki, toy]);
        let loads = edges_of(&edges, edge_category::LOADS);
        assert_eq!(loads.len(), 1, "the toy host has no tool of its tail");
        assert!(loads.contains(&(maki_plugin, maki_tool)));
        assert!(!loads.contains(&(toy_plugin, maki_tool)));
    }

    #[test]
    fn manifest_literal_loads_every_plugin_of_the_host() {
        let mut g = graph("maki");
        let loader = add(
            &mut g,
            node_kind::FUNCTION,
            "maki::load",
            "load",
            None,
            vec![code("let manifest = dir.join(\"plugin.toml\");")],
        );
        let bash = add(
            &mut g,
            node_kind::PLUGIN,
            "plugin:bash @maki",
            "bash",
            None,
            vec![],
        );
        let glob = add(
            &mut g,
            node_kind::PLUGIN,
            "plugin:glob @maki",
            "glob",
            None,
            vec![],
        );
        // The manifest-name census the route's `NavFact::PluginManifest` facts
        // carry: the family and the basename its host source names.
        g.nav.nav_facts.insert(
            loader,
            vec![NavFact::PluginManifest {
                family: "maki".to_string(),
                manifest: "plugin.toml".to_string(),
            }],
        );
        let edges = resolve(vec![g]);
        let loads = edges_of(&edges, edge_category::LOADS);
        assert_eq!(loads.len(), 2, "the loader loads every discovered plugin");
        assert!(loads.contains(&(loader, bash)) && loads.contains(&(loader, glob)));
        assert_eq!(
            evidence_rule(&edges, edge_category::LOADS),
            vec!["manifest_literal", "manifest_literal"]
        );
    }

    #[test]
    fn manifest_literal_needs_a_plugin_in_the_repo() {
        // A repo whose source names the manifest but holds no PLUGIN: no scan
        // result, no edge.
        let mut g = graph("plain");
        add(
            &mut g,
            node_kind::FUNCTION,
            "app::load",
            "load",
            None,
            vec![code("let p = dir.join(\"plugin.toml\");")],
        );
        assert!(resolve(vec![g]).is_empty());
    }

    #[test]
    fn include_str_binds_the_embedded_plugin_file() {
        let mut g = graph("maki");
        let loader_module = add(
            &mut g,
            node_kind::MODULE,
            "src::loader",
            "loader",
            None,
            vec![position("src/loader.rs")],
        );
        add(
            &mut g,
            node_kind::FUNCTION,
            "src::loader::load_all",
            "load_all",
            Some(loader_module),
            vec![
                position("src/loader.rs"),
                code("const EXAMPLE: &str = include_str!(\"../mytool/init.lua\");"),
            ],
        );
        let init = add(
            &mut g,
            node_kind::MODULE,
            "mytool::init",
            "init",
            None,
            vec![position("mytool/init.lua")],
        );
        let edges = resolve(vec![g]);
        assert_eq!(
            edges_of(&edges, edge_category::IMPORTS),
            vec![(loader_module, init)]
        );
        assert_eq!(
            evidence_rule(&edges, edge_category::IMPORTS),
            vec!["include_str"]
        );
        // The include's own row is the evidence line.
        let ev = edges
            .iter()
            .find(|e| e.category == edge_category::IMPORTS)
            .and_then(|e| evidence::Evidence::read(&e.cells))
            .unwrap();
        assert_eq!(ev.line, Some(0));
        assert_eq!(ev.basis, evidence::Basis::Site);
    }

    #[test]
    fn include_str_skips_non_lua_and_unresolvable_paths() {
        let mut g = graph("maki");
        let module = add(
            &mut g,
            node_kind::MODULE,
            "src::loader",
            "loader",
            None,
            vec![position("src/loader.rs")],
        );
        add(
            &mut g,
            node_kind::STATE_VAR,
            "src::loader::A",
            "A",
            Some(module),
            vec![code("const A: &str = include_str!(\"../x/data.txt\");")],
        );
        add(
            &mut g,
            node_kind::STATE_VAR,
            "src::loader::B",
            "B",
            Some(module),
            vec![code(
                "const B: &str = include_str!(\"../missing/init.lua\");",
            )],
        );
        // `..` escaping the repo root names nothing.
        add(
            &mut g,
            node_kind::STATE_VAR,
            "src::loader::C",
            "C",
            Some(module),
            vec![code(
                "const C: &str = include_str!(\"../../../etc/init.lua\");",
            )],
        );
        assert!(
            resolve(vec![g]).is_empty(),
            "only a resolvable .lua path binds"
        );
    }

    #[test]
    fn include_str_never_answers_through_a_same_qname_non_lua_module() {
        let mut g = graph("maki");
        let loader_module = add(
            &mut g,
            node_kind::MODULE,
            "src::loader",
            "loader",
            None,
            vec![position("src/loader.rs")],
        );
        add(
            &mut g,
            node_kind::FUNCTION,
            "src::loader::f",
            "f",
            Some(loader_module),
            vec![code("include_str!(\"../x/init.lua\")")],
        );
        // `x::init` exists but is a `.py` file.
        add(
            &mut g,
            node_kind::MODULE,
            "x::init",
            "init",
            None,
            vec![position("x/init.py")],
        );
        assert!(resolve(vec![g]).is_empty());
    }

    #[test]
    fn detection_groups_candidates_by_family_and_marks_known_hosts() {
        let mut g = graph("maki");
        // The family the build itself knows: one PLUGIN node whose owner
        // segment names it — the graph's own vocabulary, not a table.
        add(
            &mut g,
            node_kind::PLUGIN,
            "plugin:memory @maki",
            "memory",
            None,
            vec![],
        );
        let m1 = add(
            &mut g,
            node_kind::MODULE,
            "plugins::memory::init",
            "init",
            None,
            vec![],
        );
        let m2 = add(
            &mut g,
            node_kind::MODULE,
            "plugins::edit::init",
            "init",
            None,
            vec![],
        );
        let m3 = add(
            &mut g,
            node_kind::MODULE,
            "scripts::gen",
            "gen",
            None,
            vec![],
        );
        // A call whose family a shipped row covers (a prefix-relative miss:
        // `maki.api.register` lacks the underscore), twice across two files.
        g.nav.nav_facts.insert(
            m1,
            vec![NavFact::Registration {
                callee: "maki.api.register".into(),
                shape: plugin_families::ShapeTag::Table,
                named: true,
            }],
        );
        g.nav.nav_facts.insert(
            m2,
            vec![NavFact::Registration {
                callee: "maki.api.register".into(),
                shape: plugin_families::ShapeTag::Table,
                named: true,
            }],
        );
        g.nav.nav_facts.insert(
            m3,
            vec![
                NavFact::Registration {
                    callee: "vim.api.nvim_create_user_command".into(),
                    shape: plugin_families::ShapeTag::Positional,
                    named: true,
                },
                NavFact::Registration {
                    callee: "vim.api.nvim_create_autocmd".into(),
                    shape: plugin_families::ShapeTag::Positional,
                    named: true,
                },
                NavFact::Registration {
                    callee: "wezterm.on".into(),
                    shape: plugin_families::ShapeTag::Positional,
                    named: true,
                },
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        assert_eq!(
            detection_report(&merged),
            vec![
                "[plugin-detect] candidates=5 families=3 reported=2 singles=1",
                "[plugin-detect] family=maki.api shape=table calls=2 named=2 files=2 host=maki",
                "[plugin-detect] family=vim.api shape=positional calls=2 named=2 files=1 host=-",
            ],
            "known families name their host; unknown families wait; singletons summarize"
        );
    }

    #[test]
    fn detection_is_silent_with_no_candidates() {
        let merged = MergedGraph::new(vec![graph("plain")]);
        assert!(detection_report(&merged).is_empty());
    }
}
