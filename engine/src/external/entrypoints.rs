//! Declared entrypoints (LF.3b): the `.glia/overlay.toml` `[entrypoints]`
//! qname patterns become ENTRYPOINT cells on the nodes they name, and
//! liveness (`answers::entrypoint_reachable`) seeds from every node that
//! carries one, whatever its kind. A job a scheduler wires by a string, a
//! plugin hook loaded by reflection or a library's public API has no caller
//! in the graph; declaring it keeps it, and what it reaches, out of the dead
//! set.
//!
//! The declaration is persisted as a cell, not re-read from the config at
//! query time, so it survives the `.gmap` warm path and reaches every
//! consumer of the graph (Engram, neuropil) with the same meaning.
//!
//! PATTERNS come from [`RepoInputs::config`], loaded and validated once per
//! build (LF.1a / LF.2a): an exact qname, or `<prefix>::*`, which matches
//! every node whose qname starts with `<prefix>::` (the prefix node itself is
//! not matched). `[entrypoints]` is user config, not inference, so
//! `--no-overlay` does not switch it off.
//!
//! BINDING, as `gaps` (LF.2c) checks the same patterns for `orphaned_rule`,
//! so a pattern this stage leaves unmatched is exactly one `glia gaps`
//! lists: the pattern binds in its own repo's nodes first and, only when it
//! matches none there, in every repo of the build. An exact qname matches
//! every node carrying it (several kinds can share a qname, and every one of
//! them is the declared entry). Qnames are read from each graph's nodes in
//! order into a `BTreeMap`, so matching never iterates a `HashMap`.
//!
//! CELL, one per node: ENTRYPOINT Json
//! `{"decl":".glia/overlay.toml:<line>","pattern":"<pattern>","source":"config"}`,
//! `decl` being the line of the pattern in the file (`LoadedConfig::decl_of`).
//! Patterns apply in file order and the first to match a node wins: a node
//! that already carries an ENTRYPOINT cell (from an earlier pattern, or an
//! earlier repo's stage in a multi-repo build) is left as it is. Every
//! instance of a matched node (one id can sit in several language graphs)
//! gets the cell.
//!
//! fired_on marker, once per repo whose file declares a pattern:
//!   `[entrypoints] declared repo=<label> patterns=<p> matched_nodes=<m> unmatched_patterns=<u>`
//! with a trailing ` cross_repo=<c>` only when `c` patterns bound outside
//! their own repo. `matched_nodes` counts the nodes that gained a cell, so a
//! broad `::*` that marks a whole package live is visible. Then one line per
//! unmatched pattern, at most [`MAX_DETAIL_LINES`] per repo:
//!   `[entrypoints] unmatched pattern=<pattern> <decl>`.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_code_domain::cell_type;
use repo_graph_code_domain::glia_config::LoadedConfig;
use repo_graph_core::{Cell, CellPayload, NodeId, RepoId};
use repo_graph_graph::MergedGraph;
use serde_json::{Map, Value};

use super::RepoInputs;

/// Detail lines printed per repo before the rest are summarised.
const MAX_DETAIL_LINES: usize = 32;

/// The value of the ENTRYPOINT cell's `source` field for a config declaration.
const SOURCE: &str = "config";

/// qname -> every node carrying it, over one repo's graphs or every repo's.
/// Ids keep first-seen (graph, then node) order; a node sitting in several
/// graphs is recorded once.
struct QnameTable {
    by_qname: BTreeMap<String, Vec<NodeId>>,
}

impl QnameTable {
    fn build(merged: &MergedGraph, repo: Option<RepoId>) -> Self {
        let mut by_qname: BTreeMap<String, Vec<NodeId>> = BTreeMap::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for g in &merged.graphs {
            if repo.is_some_and(|r| r != g.repo) {
                continue;
            }
            for n in &g.nodes {
                if !seen.insert(n.id) {
                    continue;
                }
                if let Some(q) = g.nav.qname_by_id.get(&n.id) {
                    by_qname.entry(q.clone()).or_default().push(n.id);
                }
            }
        }
        QnameTable { by_qname }
    }

    /// The nodes `pattern` names: every node whose qname equals it, or, for
    /// `<prefix>::*`, every node whose qname starts with `<prefix>::`, in
    /// qname order.
    fn matches(&self, pattern: &str) -> Vec<NodeId> {
        match pattern.strip_suffix("::*") {
            Some(prefix) => {
                let head = format!("{prefix}::");
                self.by_qname
                    .range(head.clone()..)
                    .take_while(|(q, _)| q.starts_with(&head))
                    .flat_map(|(_, ids)| ids.iter().copied())
                    .collect()
            }
            None => self.by_qname.get(pattern).cloned().unwrap_or_default(),
        }
    }
}

/// Outcome counts of one repo's patterns.
#[derive(Debug, Default)]
struct Tally {
    patterns: usize,
    matched_nodes: usize,
    unmatched: usize,
    cross_repo: usize,
    details: Vec<String>,
}

/// The ENTRYPOINT payload for `pattern`, declared at `decl`.
fn payload(pattern: &str, decl: &str) -> CellPayload {
    let mut m = Map::new();
    m.insert("decl".to_string(), Value::String(decl.to_string()));
    m.insert("pattern".to_string(), Value::String(pattern.to_string()));
    m.insert("source".to_string(), Value::String(SOURCE.to_string()));
    CellPayload::Json(Value::Object(m).to_string())
}

/// Apply `input`'s `[entrypoints]` patterns to `merged`. True when a cell was
/// written.
pub(super) fn apply_declared_entrypoints(
    merged: &mut MergedGraph,
    input: &RepoInputs,
    cfg: &LoadedConfig,
) -> bool {
    let patterns = &cfg.config.entrypoints.qnames;
    if patterns.is_empty() {
        return false;
    }
    let own = QnameTable::build(merged, Some(input.repo));
    let mut all: Option<QnameTable> = None;
    // Every instance of every node, looked up by id only (NodeId is Hash, not Ord).
    let mut at: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
    let mut declared: HashSet<NodeId> = HashSet::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            at.entry(n.id).or_default().push((gi, ni));
            if n.cells.iter().any(|c| c.kind == cell_type::ENTRYPOINT) {
                declared.insert(n.id);
            }
        }
    }

    let mut t = Tally { patterns: patterns.len(), ..Tally::default() };
    for spanned in patterns {
        let pattern = spanned.get_ref().as_str();
        let decl = cfg.decl_of(spanned.span());
        let mut ids = own.matches(pattern);
        if ids.is_empty() {
            ids = all.get_or_insert_with(|| QnameTable::build(merged, None)).matches(pattern);
            if !ids.is_empty() {
                t.cross_repo += 1;
            }
        }
        if ids.is_empty() {
            t.unmatched += 1;
            t.details.push(format!("unmatched pattern={pattern} {decl}"));
            continue;
        }
        let cell = payload(pattern, &decl);
        for id in ids {
            if !declared.insert(id) {
                continue;
            }
            for &(gi, ni) in at.get(&id).map_or(&[][..], Vec::as_slice) {
                if let Some(node) = merged.graphs.get_mut(gi).and_then(|g| g.nodes.get_mut(ni)) {
                    node.cells.push(Cell { kind: cell_type::ENTRYPOINT, payload: cell.clone() });
                }
            }
            t.matched_nodes += 1;
        }
    }
    report(&input.label, &t);
    t.matched_nodes > 0
}

fn report(label: &str, t: &Tally) {
    let cross = if t.cross_repo > 0 { format!(" cross_repo={}", t.cross_repo) } else { String::new() };
    eprintln!(
        "[entrypoints] declared repo={label} patterns={} matched_nodes={} unmatched_patterns={}{cross}",
        t.patterns, t.matched_nodes, t.unmatched,
    );
    for line in t.details.iter().take(MAX_DETAIL_LINES) {
        eprintln!("[entrypoints] {line}");
    }
    if t.details.len() > MAX_DETAIL_LINES {
        eprintln!("[entrypoints] ... {} more detail lines", t.details.len() - MAX_DETAIL_LINES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{CodeNav, node_kind};
    use repo_graph_core::{Confidence, Node, NodeKindId};
    use repo_graph_graph::RepoGraph;

    fn graph(repo: RepoId, qnames: &[(NodeKindId, &str)]) -> RepoGraph {
        let mut nav = CodeNav::default();
        let mut nodes = Vec::new();
        for (kind, q) in qnames {
            let id = NodeId::from_parts("code", repo, *kind, q);
            nav.record(id, q.rsplit("::").next().unwrap_or(q), q, *kind, None);
            nodes.push(Node { id, repo, confidence: Confidence::Strong, cells: Vec::new() });
        }
        RepoGraph {
            repo,
            nodes,
            edges: Vec::new(),
            nav,
            symbols: Default::default(),
            unresolved_calls: Vec::new(),
            unresolved_refs: Vec::new(),
            properties: Default::default(),
        }
    }

    fn table(qnames: &[&str]) -> QnameTable {
        let repo = RepoId::from_canonical("test://entrypoints");
        let rows: Vec<(NodeKindId, &str)> = qnames.iter().map(|q| (node_kind::FUNCTION, *q)).collect();
        QnameTable::build(&MergedGraph::new(vec![graph(repo, &rows)]), None)
    }

    fn names(t: &QnameTable, pattern: &str) -> Vec<String> {
        let ids = t.matches(pattern);
        t.by_qname.iter().filter(|(_, v)| v.iter().any(|id| ids.contains(id))).map(|(q, _)| q.clone()).collect()
    }

    #[test]
    fn prefix_matches_descendants_not_the_prefix_node() {
        let t = table(&["app::jobs", "app::jobs::a", "app::jobs::b::c", "app::jobsx::d", "app::job"]);
        assert_eq!(names(&t, "app::jobs::*"), ["app::jobs::a", "app::jobs::b::c"]);
        assert_eq!(names(&t, "app::jobs"), ["app::jobs"]);
        assert!(t.matches("app::none::*").is_empty());
        assert!(t.matches("app::jobs::zz").is_empty());
    }

    #[test]
    fn exact_matches_every_node_sharing_the_qname() {
        let repo = RepoId::from_canonical("test://entrypoints");
        let g = graph(repo, &[(node_kind::CLASS, "ui::Page"), (node_kind::COMPONENT, "ui::Page")]);
        let t = QnameTable::build(&MergedGraph::new(vec![g]), Some(repo));
        assert_eq!(t.matches("ui::Page").len(), 2);
    }

    #[test]
    fn payload_is_sorted_json() {
        let CellPayload::Json(s) = payload("a::*", ".glia/overlay.toml:4") else {
            panic!("ENTRYPOINT is a Json cell");
        };
        assert_eq!(s, r#"{"decl":".glia/overlay.toml:4","pattern":"a::*","source":"config"}"#);
    }
}
