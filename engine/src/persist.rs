//! The one persist / load module shared by py and cli (LC.7): write a build's
//! `MergedGraph` plus its layout metadata to a sharded `.gmap` directory, and
//! load it back as a [`GenerateResult`] that answers like the fresh one.
//! Extended by LC.8 (`load_or_rebuild`), LC.9 and LC.10a.
//!
//! What survives the round trip, and where it lives:
//! - repo labels and roots, and the build's parse errors describe the LAYOUT
//!   (a multi-repo build has several repos and one error list), so they go in
//!   `manifest.json` ([`repo_graph_store::LayoutMeta`]);
//! - `RepoGraph.properties` is per-graph code state, so it goes in each
//!   shard's code section beside nav and symbols.
//!
//! A root is recorded RELATIVE to the layout dir (`../..` for the in-repo
//! `<repo>/.ai/repo-graph`), so a committed layout carries no absolute path
//! (no username) and still resolves after a clone; [`load_layout`] resolves it
//! against the dir it loads from.
//!
//! Markers, one line per call:
//! `[gmap] meta: repos=<r> labeled=<l> rooted=<o> parse_errors=<p> properties=<q> writer=<w>`
//! from [`persist_layout`] (the store's own `[gmap] layout` line follows it),
//! and `[gmap] loaded <dir>: repos=<r> labeled=<l> parse_errors=<p> properties=<q>`
//! from [`load_layout`].
//!
//! Module slot declared by L0.2: reached as `repo_graph_engine::persist::<item>`,
//! never flattened into the crate root.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use repo_graph_graph::MergedGraph;
use repo_graph_store::{
    LayoutMeta, RepoMeta, StoreError, read_merged_sharded_meta, write_merged_sharded_meta,
};

use crate::GenerateResult;

/// Why [`load_layout`] could not serve a layout. `needs_rebuild` is the
/// store's classification (`StoreError::needs_rebuild`): true when the bytes on
/// disk are missing, old, foreign or damaged and the fix is to regenerate;
/// false for a failure a rebuild would not fix (permissions, a full disk).
#[derive(Debug)]
#[non_exhaustive]
pub struct LoadError {
    pub reason: String,
    pub needs_rebuild: bool,
}

impl LoadError {
    fn from_store(dir: &Path, e: &StoreError) -> Self {
        let reason = e.rebuild_reason().unwrap_or_else(|| e.to_string());
        Self { reason: format!("{}: {reason}", dir.display()), needs_rebuild: e.needs_rebuild() }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.needs_rebuild {
            write!(f, "{} - rebuild the graph", self.reason)
        } else {
            f.write_str(&self.reason)
        }
    }
}

impl std::error::Error for LoadError {}

/// The layout metadata of one build, for a layout written to `dir`: one
/// [`RepoMeta`] per repo id in `labels` or `roots` (sorted by id), each root
/// made relative to `dir`, and `parse_errors` in build order. `dir` need not
/// exist yet.
pub fn layout_meta(
    labels: &BTreeMap<u64, String>,
    roots: &BTreeMap<u64, String>,
    parse_errors: &[String],
    dir: &Path,
) -> LayoutMeta {
    let mut ids: Vec<u64> = labels.keys().chain(roots.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();
    let repos = ids
        .into_iter()
        .map(|id| RepoMeta {
            id,
            label: labels.get(&id).cloned().unwrap_or_default(),
            root: roots.get(&id).map(|root| {
                let root = Path::new(root);
                relative_root(dir, root)
                    .unwrap_or_else(|| lenient_absolute(root).to_string_lossy().into_owned())
            }),
        })
        .collect();
    LayoutMeta { repos, parse_errors: parse_errors.to_vec() }
}

/// Write `merged` and `meta` as a sharded layout at `dir` (created if missing;
/// unchanged shards and an unchanged manifest are not rewritten). `writer`
/// names the caller (`py`, `cli`, ...) in the marker and in the error text.
pub fn persist_layout(
    merged: &MergedGraph,
    meta: &LayoutMeta,
    dir: &Path,
    writer: &str,
) -> Result<(), String> {
    let labeled = meta.repos.iter().filter(|r| !r.label.is_empty()).count();
    let rooted = meta.repos.iter().filter(|r| r.root.is_some()).count();
    eprintln!(
        "[gmap] meta: repos={} labeled={labeled} rooted={rooted} parse_errors={} properties={} writer={writer}",
        meta.repos.len(),
        meta.parse_errors.len(),
        property_count(merged),
    );
    write_merged_sharded_meta(merged, meta, dir)
        .map(|_| ())
        .map_err(|e| format!("{writer}: persist to {}: {e}", dir.display()))
}

/// Load the layout at `dir` as a [`GenerateResult`]: the merged graph (with
/// `properties`), `repo_labels` from the manifest's labels, `repo_roots` with
/// each relative root joined onto `dir` and canonicalised (as joined when the
/// path no longer exists), `parse_errors`, and the totals recomputed. A layout
/// written without metadata loads with empty labels, roots and errors.
pub fn load_layout(dir: &Path) -> Result<GenerateResult, LoadError> {
    let (merged, meta) =
        read_merged_sharded_meta(dir).map_err(|e| LoadError::from_store(dir, &e))?;
    let repo_labels: BTreeMap<u64, String> = meta
        .repos
        .iter()
        .filter(|r| !r.label.is_empty())
        .map(|r| (r.id, r.label.clone()))
        .collect();
    let repo_roots: BTreeMap<u64, String> = meta
        .repos
        .iter()
        .filter_map(|r| {
            let root = r.root.as_deref()?;
            let joined = dir.join(root);
            let resolved = std::fs::canonicalize(&joined).unwrap_or(joined);
            Some((r.id, resolved.to_string_lossy().into_owned()))
        })
        .collect();
    let total_nodes: usize = merged.graphs.iter().map(|g| g.nodes.len()).sum();
    let total_edges: usize = merged.graphs.iter().map(|g| g.edges.len()).sum::<usize>()
        + merged.cross_edges.len();
    eprintln!(
        "[gmap] loaded {}: repos={} labeled={} parse_errors={} properties={}",
        dir.display(),
        meta.repos.len(),
        repo_labels.len(),
        meta.parse_errors.len(),
        property_count(&merged),
    );
    Ok(GenerateResult {
        merged,
        total_nodes,
        total_edges,
        parse_errors: meta.parse_errors,
        repo_labels,
        repo_roots,
    })
}

fn property_count(merged: &MergedGraph) -> usize {
    merged.graphs.iter().map(|g| g.properties.len()).sum()
}

/// `root` relative to `dir`, `/`-separated: both made absolute and
/// canonicalised (leniently, so a layout dir that does not exist yet still
/// works), the common prefix stripped, one `..` per remaining `dir` component.
/// `.` when they are the same directory. `None` when no relative path exists
/// (different Windows drives) or a component is not UTF-8.
fn relative_root(dir: &Path, root: &Path) -> Option<String> {
    let dir = lenient_absolute(dir);
    let root = lenient_absolute(root);
    let dir_parts: Vec<Component<'_>> = dir.components().collect();
    let root_parts: Vec<Component<'_>> = root.components().collect();
    // Absolute paths start with their prefix / root: a mismatch there is
    // another drive, which no relative path reaches.
    if dir_parts.first() != root_parts.first() {
        return None;
    }
    let common = dir_parts.iter().zip(&root_parts).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = Vec::new();
    for c in &dir_parts[common..] {
        match c {
            Component::Normal(_) => out.push(".."),
            _ => return None,
        }
    }
    for c in &root_parts[common..] {
        match c {
            Component::Normal(s) => out.push(s.to_str()?),
            _ => return None,
        }
    }
    Some(if out.is_empty() { ".".to_string() } else { out.join("/") })
}

/// `p` absolute and canonical as far as it exists: the deepest existing
/// ancestor is canonicalised and the missing tail re-appended (lexically, `.`
/// dropped and `..` popped). Never fails: with nothing canonicalisable it is
/// `p` made absolute against the working directory, or `p` itself.
fn lenient_absolute(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail: Vec<Component<'_>> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let mut out = c;
            for comp in tail.iter().rev() {
                match comp {
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::CurDir => {}
                    other => out.push(other.as_os_str()),
                }
            }
            return out;
        }
        let Some(parent) = base.parent() else {
            return abs.clone();
        };
        if let Some(last) = base.components().next_back() {
            tail.push(last);
        }
        base = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_root_walks_up_then_down() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let dir = repo.join(".ai/repo-graph");
        assert_eq!(relative_root(&dir, &repo).as_deref(), Some("../.."));
        assert_eq!(relative_root(&repo, &repo).as_deref(), Some("."));
        assert_eq!(relative_root(&repo, &repo.join("src")).as_deref(), Some("src"));
        let sibling = tmp.path().join("other");
        assert_eq!(relative_root(&dir, &sibling).as_deref(), Some("../../../other"));
    }

    #[test]
    fn lenient_absolute_keeps_a_missing_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let p = tmp.path().join("a/./b/../c");
        assert_eq!(lenient_absolute(&p), base.join("a/c"));
    }
}
