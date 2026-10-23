//! The candidate file space every 0.5.3 file ranking indexes (EA.3, item 2):
//! rank_eval's `index_graph` rule. A candidate is the located file of a
//! MODULE node with no ORIGIN cell containing `generated`, whose extension
//! ([`ext_of`]) is non-empty and not in [`NOT_CODE_EXTS`]. Files sort by
//! (repo label, path), so one repo's order is rank_eval's `paths.sort()` and
//! a tie that goes to the smaller index goes to the smaller path (`better`).
//!
//! Co-change, text, IDR and the calibration replay all index files through
//! one [`FileIndex`]; its lookups are map lookups, never iterated.
//!
//! Fired_on, stderr (printed by `RankCache::files`): `[rank-files]
//! holder=<h> repos=<r> files=<f> generated=<g> not_code=<x> ms=<m>`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::{cell_type, node_kind};
use glia_core::{CellPayload, NodeId};
use glia_graph::MergedGraph;

use crate::answers::Locator;

/// Extensions a MODULE node may sit on that are not code (docs, data,
/// lockfiles, manifests): rank_eval.rs `NOT_CODE`, verbatim and in its order.
pub const NOT_CODE_EXTS: [&str; 36] = [
    "md", "mdx", "markdown", "rst", "txt", "json", "jsonc", "lock", "yaml", "yml", "toml", "csv", "tsv",
    "map", "svg", "png", "jpg", "gif", "ico", "xml", "ini", "cfg", "conf", "env", "example", "sum", "mod",
    "gitkeep", "pdf", "snap", "golden", "out", "log", "html", "css", "scss",
];

/// rank_eval's `ext_of`: the basename's text after its last '.', when the
/// stem before it is non-empty, ASCII-lowercased; else empty.
pub(crate) fn ext_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn payload_text(p: &CellPayload) -> &str {
    match p {
        CellPayload::Text(s) | CellPayload::Json(s) => s,
        CellPayload::Bytes(_) => "",
    }
}

/// The label a repo id sorts and prints by: the given label, else
/// `repo#<id % 1000>` (engine/src/identity.rs's fallback).
fn label_of(labels: &BTreeMap<u64, String>, repo: u64) -> String {
    labels.get(&repo).cloned().unwrap_or_else(|| format!("repo#{}", repo % 1000))
}

/// The candidate files of one graph, each with its nodes and degree.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct FileIndex {
    /// Per file: (repo id, path), sorted by (repo label, path).
    files: Vec<(u64, String)>,
    nodes: Vec<Vec<NodeId>>,
    degree: Vec<f64>,
    by_key: HashMap<(u64, String), u32>,
    by_node: HashMap<NodeId, u32>,
    repos: usize,
    generated: usize,
    not_code: usize,
}

impl FileIndex {
    /// The file space of `merged`; `labels` (repo id -> label) order files
    /// across repos.
    pub fn of(merged: &MergedGraph, labels: &BTreeMap<u64, String>) -> FileIndex {
        let loc = Locator::new(merged);
        let mut seen: BTreeSet<u64> = BTreeSet::new();
        // (node, repo, located file) in walk order.
        let mut walked: Vec<(NodeId, u64, Option<String>)> = Vec::new();
        let mut module_files: BTreeSet<(u64, String)> = BTreeSet::new();
        let mut generated: BTreeSet<(u64, String)> = BTreeSet::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if !seen.insert(n.id.0) {
                    continue;
                }
                let file = loc.locate(n.id).file;
                if g.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::MODULE)
                    && let Some(f) = &file
                {
                    let gen_origin = n
                        .cells
                        .iter()
                        .any(|c| c.kind == cell_type::ORIGIN && payload_text(&c.payload).contains("generated"));
                    if gen_origin {
                        generated.insert((g.repo.0, f.clone()));
                    } else {
                        module_files.insert((g.repo.0, f.clone()));
                    }
                }
                walked.push((n.id, g.repo.0, file));
            }
        }
        let mut not_code = 0usize;
        let mut keyed: Vec<(String, String, u64)> = Vec::new();
        for (repo, f) in &module_files {
            if generated.contains(&(*repo, f.clone())) {
                continue;
            }
            let e = ext_of(f);
            if e.is_empty() || NOT_CODE_EXTS.contains(&e.as_str()) {
                not_code += 1;
            } else {
                keyed.push((label_of(labels, *repo), f.clone(), *repo));
            }
        }
        keyed.sort();
        let files: Vec<(u64, String)> = keyed.into_iter().map(|(_, p, r)| (r, p)).collect();
        let by_key: HashMap<(u64, String), u32> =
            files.iter().enumerate().map(|(i, k)| (k.clone(), i as u32)).collect();
        let mut nodes: Vec<Vec<NodeId>> = vec![Vec::new(); files.len()];
        let mut by_node: HashMap<NodeId, u32> = HashMap::new();
        for (id, repo, file) in walked {
            if let Some(f) = file.and_then(|p| by_key.get(&(repo, p)).copied()) {
                nodes[f as usize].push(id);
                by_node.insert(id, f);
            }
        }
        // rank_eval's `deg_all`: every edge's endpoints, every category.
        let mut deg_node: HashMap<NodeId, f64> = HashMap::with_capacity(by_node.len());
        for e in merged.all_edges() {
            if by_node.contains_key(&e.from) {
                *deg_node.entry(e.from).or_default() += 1.0;
            }
            if by_node.contains_key(&e.to) {
                *deg_node.entry(e.to).or_default() += 1.0;
            }
        }
        let degree: Vec<f64> = nodes
            .iter()
            .map(|ns| ns.iter().map(|n| deg_node.get(n).copied().unwrap_or(0.0)).sum())
            .collect();
        let repos = files.iter().map(|(r, _)| *r).collect::<BTreeSet<u64>>().len();
        FileIndex { files, nodes, degree, by_key, by_node, repos, generated: generated.len(), not_code }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// File `f`'s path, relative to its repo root.
    pub fn path(&self, f: u32) -> &str {
        &self.files[f as usize].1
    }

    /// File `f`'s repo id.
    pub fn repo(&self, f: u32) -> u64 {
        self.files[f as usize].0
    }

    /// The index of `path` in `repo`, when it is a candidate.
    pub fn index_of(&self, repo: u64, path: &str) -> Option<u32> {
        self.by_key.get(&(repo, path.to_string())).copied()
    }

    /// The candidate file `id` is located in.
    pub fn file_of_node(&self, id: NodeId) -> Option<u32> {
        self.by_node.get(&id).copied()
    }

    /// Every node located in `f`, in the graph walk order.
    pub fn nodes(&self, f: u32) -> &[NodeId] {
        &self.nodes[f as usize]
    }

    /// The sum over `nodes(f)` of each node's edge endpoint count.
    pub fn degree(&self, f: u32) -> f64 {
        self.degree[f as usize]
    }

    /// Repos holding at least one candidate.
    pub fn repos(&self) -> usize {
        self.repos
    }

    /// MODULE files left out as generated.
    pub fn generated(&self) -> usize {
        self.generated
    }

    /// MODULE files left out for an empty or NOT_CODE extension.
    pub fn not_code(&self) -> usize {
        self.not_code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_of_is_the_harness_rule() {
        assert_eq!(ext_of("a/b/c.PY"), "py");
        assert_eq!(ext_of("a/.gitkeep"), "");
        assert_eq!(ext_of("a.b/Makefile"), "");
        assert_eq!(ext_of("x.tar.gz"), "gz");
    }
}
