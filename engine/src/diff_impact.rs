//! `diff_impact` (LE.2): changed nodes (a git rev's graph delta, or a pasted
//! diff) to one multi-seed, located, ranked blast radius with per-row seed
//! attribution: "what does my change affect", in one call.
//!
//! Two entry points share one answer, [`DiffImpact`]:
//!
//! - [`diff_impact_vs_rev`]: the working tree's change against a git rev,
//!   through LE.1b's `delta::graph_delta_vs_rev`. Both graphs come from that
//!   one call (nothing is rebuilt) and the radius runs over the working
//!   tree's graph;
//! - [`diff_impact_from_diff`]: a pasted unified diff (or a changed-file list)
//!   over a graph the caller already holds, resolved through
//!   `MergedGraph::resolve_signals`, left unchanged because repo-graph's
//!   `find` depends on it.
//!
//! # Seeds
//!
//! Rev mode seeds the added and moved nodes (their working-tree ids), the
//! modified nodes whose OWN text changed, and the surviving ends of every
//! carry edge (`CODE_PROFILE.tables.carry_edges`, what the radius walks)
//! gained or lost: a caller whose call was deleted has no edge left to find,
//! so the removed edge's ends are what put it in the answer. The own-text rule
//! is LE.3b's (`tests_for::CodeText`): a container's CODE cell is its whole
//! span, so an edit inside `price` marks its module modified too; the module
//! is a seed only when its text outside its children changed. A structural
//! edge (DEFINES, IMPORTS, CONTAINS) gained or lost is reported in the edge
//! lists and seeds nothing: the radius never walks it. Removed nodes are
//! never seeds: they are not in the working tree's graph, and their surviving
//! callers are seeded through the removed edges.
//!
//! Pasted mode seeds the narrowest node spanning each ADDED line of each file
//! (a changed-file list: every node of each file). Each file of the diff is
//! resolved as its own item of one batch, so every file that places no node
//! is named in `unresolved_diff_files`: a deletion-only hunk (the diff frames
//! carry added lines only, so this mode cannot seed a pure deletion; rev mode
//! can), a deleted file, or a file glia does not parse. "Nothing changed" and
//! "glia could not place this file" stay distinguishable. A file whose hunks
//! only delete is never resolved at all: resolved alone it would read as a
//! changed-file list and seed every node of the file.
//!
//! In both modes a MODULE seed is dropped when a finer node of the same file
//! is also a seed (a module has no carry out-edges of its own and only dilutes
//! the ranking); it stays in `changed`, with `seed: false`. Seeds are ordered
//! by (qname, id) and passed to LD.5's `blast_radius_seeded` as node ids,
//! never re-resolved by name (two per-language graphs can share a qname):
//! ONE walk and ONE PPR over the whole seed list, every row carrying the seed
//! whose wave reached it first, every seed its one-hop `linked_seeds`.
//!
//! # Rows
//!
//! `changed` lists rev mode's delta rows first, as LE.1b orders them (a
//! removed row located in the BEFORE graph, every other in the working
//! tree's), then the edge-endpoint seeds no node row names, by (qname, id);
//! pasted mode lists its hits in diff order. `edges_added` / `edges_removed`
//! are the delta's edge rows of every category, as LE.1b located them (empty
//! in pasted mode). Every line is 1-based (LD.1).
//!
//! With no seed the radius is not run: `impact` is empty and its absence
//! (LD.8a, reason `no_match`) says why: `no graph change vs <base>`, a change
//! that only removes nodes or touches structural edges, or `the diff resolved
//! to no node` naming the files it could not place.
//!
//! fired_on marker, one line per answer:
//! `[diff-impact] mode=<rev|diff> base=<rev|-> changed=<C> seeds=<S> impact=<I> edges +<a> -<r> unresolved_files=<U>`
//! — grep `^\[diff-impact\] mode=`.

use std::collections::HashSet;

use repo_graph_code_domain::node_kind;
use repo_graph_core::NodeId;
use repo_graph_graph::MergedGraph;

use crate::absence;
use crate::answers::{BlastOptions, BlastRadius, Located, Locator, blast_radius_seeded};
use crate::delta::{DeltaEdge, RevDelta, graph_delta_vs_rev};
use crate::profile::CODE_PROFILE;
use crate::tests_for::CodeText;

/// The primitive name in the `[absence]` marker.
const PRIMITIVE: &str = "diff_impact";

/// One changed node: identity, 1-based location, how it changed, and whether
/// it seeds the radius.
///
/// `change` is `added` | `removed` | `modified` | `moved` (rev mode, LE.1b's
/// delta rows), `edge_endpoint` (rev mode: a surviving end of a carry edge
/// gained or lost, whose own text did not change) or `diff_hit` (pasted
/// mode). A `removed` row is located in the base rev's graph and is never a
/// seed.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ChangedNode {
    pub id: u64,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    pub change: &'static str,
    pub seed: bool,
}

/// The answer (module docs): what changed, and the one radius around it.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct DiffImpact {
    /// The git rev the working tree was compared against (rev mode), as
    /// given; `None` in pasted mode.
    pub base: Option<String>,
    pub changed: Vec<ChangedNode>,
    pub edges_added: Vec<DeltaEdge>,
    pub edges_removed: Vec<DeltaEdge>,
    /// LD.5's multi-seed blast radius over the seeds: `seeds` (with
    /// `linked_seeds`), `results` (each with its `seed`), and `absence`
    /// exactly when `results` is empty.
    pub impact: BlastRadius,
    /// Pasted mode: the files of the diff that place no node (module docs).
    pub unresolved_diff_files: Vec<String>,
}

/// The impact of the working tree's change against git rev `base` (module
/// docs). `Err` on `delta::graph_delta_vs_rev`'s errors: not a directory,
/// not a git work tree, an unknown rev, a failed build. Like the delta, it
/// saves the working tree's parse-cache sidecar, never a layout.
pub fn diff_impact_vs_rev(repo_path: &str, base: &str, opts: &BlastOptions) -> Result<DiffImpact, String> {
    let rev = graph_delta_vs_rev(repo_path, base)?;
    let after = &rev.after.merged;
    let loc = Locator::new(after);
    let candidates = present(after, rev_candidates(&rev));
    let seeds = seed_list(&loc, &candidates);
    let is_seed: HashSet<NodeId> = seeds.iter().map(|&(_, id)| id).collect();

    let mut changed: Vec<ChangedNode> = rev
        .answer
        .nodes
        .iter()
        .map(|n| ChangedNode {
            id: n.id,
            qname: n.qname.clone(),
            kind: n.kind,
            file: n.file.clone(),
            line: n.line,
            change: n.change,
            seed: n.change != "removed" && is_seed.contains(&NodeId(n.id)),
        })
        .collect();
    let listed: HashSet<u64> = changed.iter().filter(|c| c.change != "removed").map(|c| c.id).collect();
    let mut endpoints: Vec<ChangedNode> = candidates
        .iter()
        .filter(|id| !listed.contains(&id.0))
        .map(|&id| row(loc.locate(id), "edge_endpoint", is_seed.contains(&id)))
        .collect();
    endpoints.sort_by(|a, b| (&a.qname, a.id).cmp(&(&b.qname, b.id)));
    changed.extend(endpoints);

    let edges = |change: &str| -> Vec<DeltaEdge> {
        rev.answer.edges.iter().filter(|e| e.change == change).cloned().collect()
    };
    let (edges_added, edges_removed) = (edges("added"), edges("removed"));
    let given = rev.answer.base.clone();
    let mut impact = if seeds.is_empty() {
        let counts = &rev.answer.counts;
        let note = if rev.answer.nodes.is_empty() && rev.answer.edges.is_empty() {
            format!("no graph change vs {given}")
        } else {
            format!(
                "the change vs {given} seeds no node of the working tree's graph: it removes {} {} and changes no node or carry edge a blast radius walks from",
                counts.nodes_removed,
                absence::plural(counts.nodes_removed, "node", "nodes")
            )
        };
        no_seed(after, &format!("rev {given}"), note)
    } else {
        blast_radius_seeded(after, &seeds, opts)
    };
    if let Some(a) = impact.absence.as_mut() {
        a.unparsed_files = rev.after.parse_errors.len();
    }
    let answer = DiffImpact {
        base: Some(given),
        changed,
        edges_added,
        edges_removed,
        impact,
        unresolved_diff_files: Vec::new(),
    };
    marker("rev", &answer, seeds.len());
    Ok(answer)
}

/// The impact of a pasted unified diff, or a changed-file list, over
/// `merged` (module docs): the graph the diff's new side describes.
pub fn diff_impact_from_diff(merged: &MergedGraph, diff_text: &str, opts: &BlastOptions) -> DiffImpact {
    let items = diff_items(diff_text);
    let queries: Vec<(&str, &str)> = items
        .iter()
        .filter_map(|i| i.text.as_deref().map(|t| (t, "diff")))
        .collect();
    let mut hits = if queries.is_empty() {
        Vec::new()
    } else {
        merged.resolve_signals(&queries)
    }
    .into_iter();

    let mut files: Vec<&str> = Vec::new();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut ids: Vec<NodeId> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for item in &items {
        let found = if item.text.is_some() { hits.next().unwrap_or_default() } else { Vec::new() };
        if !files.contains(&item.file.as_str()) {
            files.push(&item.file);
        }
        if !found.is_empty() {
            placed.insert(&item.file);
        }
        ids.extend(found.into_iter().filter(|id| seen.insert(*id)));
    }
    let unresolved_diff_files: Vec<String> =
        files.iter().filter(|f| !placed.contains(*f)).map(|f| f.to_string()).collect();

    let loc = Locator::new(merged);
    let seeds = seed_list(&loc, &ids);
    let is_seed: HashSet<NodeId> = seeds.iter().map(|&(_, id)| id).collect();
    let changed: Vec<ChangedNode> =
        ids.iter().map(|&id| row(loc.locate(id), "diff_hit", is_seed.contains(&id))).collect();
    let impact = if seeds.is_empty() {
        let query = if files.is_empty() { "diff".to_string() } else { format!("diff: {}", files.join(", ")) };
        let note = if unresolved_diff_files.is_empty() {
            "the diff resolved to no node".to_string()
        } else {
            format!(
                "the diff resolved to no node: no added line of {} sits in a node of this graph (a deletion-only hunk, a deleted file, or a file glia does not parse)",
                unresolved_diff_files.join(", ")
            )
        };
        no_seed(merged, &query, note)
    } else {
        blast_radius_seeded(merged, &seeds, opts)
    };
    let answer = DiffImpact {
        base: None,
        changed,
        edges_added: Vec::new(),
        edges_removed: Vec::new(),
        impact,
        unresolved_diff_files,
    };
    marker("diff", &answer, seeds.len());
    answer
}

/// Rev mode's seed candidates, as working-tree ids, before the presence
/// check and the MODULE rule (module docs).
fn rev_candidates(rev: &RevDelta) -> Vec<NodeId> {
    let d = &rev.delta;
    let (old, new) = (CodeText::new(&rev.before.merged), CodeText::new(&rev.after.merged));
    let moved_to: std::collections::HashMap<NodeId, NodeId> = d.moved_nodes.iter().map(|&(b, a)| (b, a)).collect();
    let was: std::collections::HashMap<NodeId, NodeId> = d.moved_nodes.iter().map(|&(b, a)| (a, b)).collect();
    let carries = |k: &&repo_graph_activation::algo::delta::EdgeKey| CODE_PROFILE.tables.carries(k.category);
    let mut ids: Vec<NodeId> = Vec::new();
    ids.extend(d.added_nodes.iter().copied());
    ids.extend(d.moved_nodes.iter().map(|&(_, a)| a));
    for &id in &d.modified_nodes {
        let prior = was.get(&id).copied().unwrap_or(id);
        if !new.is_container(id) || new.own_text(id) != old.own_text(prior) {
            ids.push(id);
        }
    }
    for k in d.added_edges.iter().filter(carries) {
        ids.extend([k.from, k.to]);
    }
    for k in d.removed_edges.iter().filter(carries) {
        for end in [k.from, k.to] {
            ids.push(moved_to.get(&end).copied().unwrap_or(end));
        }
    }
    ids
}

/// `ids` deduplicated, first occurrence kept, minus the ones no graph of
/// `merged` names (a removed edge's vanished end).
fn present(merged: &MergedGraph, ids: Vec<NodeId>) -> Vec<NodeId> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    ids.into_iter()
        .filter(|id| merged.graphs.iter().any(|g| g.nav.qname_by_id.contains_key(id)))
        .filter(|id| seen.insert(*id))
        .collect()
}

/// The seeds among `ids` as `(qname, id)`, ordered by (qname, id): every
/// node except a MODULE sharing its file with a finer seed.
fn seed_list(loc: &Locator<'_>, ids: &[NodeId]) -> Vec<(String, NodeId)> {
    let module = node_kind::name(node_kind::MODULE);
    let located: Vec<Located> = ids.iter().map(|&id| loc.locate(id)).collect();
    let finer: HashSet<&str> = located
        .iter()
        .filter(|at| at.kind != module)
        .filter_map(|at| at.file.as_deref())
        .collect();
    let mut seeds: Vec<(String, NodeId)> = located
        .iter()
        .filter(|at| !(at.kind == module && at.file.as_deref().is_some_and(|f| finer.contains(f))))
        .map(|at| (at.qname.clone(), NodeId(at.id)))
        .collect();
    seeds.sort_by(|a, b| (&a.0, a.1.0).cmp(&(&b.0, b.1.0)));
    seeds
}

fn row(at: Located, change: &'static str, seed: bool) -> ChangedNode {
    ChangedNode {
        id: at.id,
        qname: at.qname,
        kind: at.kind,
        file: at.file,
        line: at.line,
        change,
        seed,
    }
}

/// The empty radius of a change that seeds nothing: no walk, and an absence
/// saying why.
fn no_seed(merged: &MergedGraph, query: &str, note: String) -> BlastRadius {
    BlastRadius {
        seeds: Vec::new(),
        unresolved: Vec::new(),
        results: Vec::new(),
        absence: Some(absence::empty(merged, PRIMITIVE, query, "no_match", note, &[], None)),
    }
}

fn marker(mode: &str, a: &DiffImpact, seeds: usize) {
    eprintln!(
        "[diff-impact] mode={mode} base={} changed={} seeds={seeds} impact={} edges +{} -{} unresolved_files={}",
        a.base.as_deref().unwrap_or("-"),
        a.changed.len(),
        a.impact.results.len(),
        a.edges_added.len(),
        a.edges_removed.len(),
        a.unresolved_diff_files.len()
    );
}

/// One file of a pasted diff: its path and the text resolved for it, `None`
/// when it has no added line to place (a deletion-only hunk, a deleted file).
struct DiffItem {
    file: String,
    text: Option<String>,
}

/// A pasted diff split into one item per file (module docs). With no
/// `+++ ` header the text is a changed-file list: one item per line that
/// looks like a path, the lines `resolve_signal`'s file-list mode reads.
/// Headers are read as `resolve_signal`'s diff parser reads them: a `+++ `
/// line starts a file (`b/` stripped; `/dev/null` is a deleted file, named by
/// its `--- a/` line), and a `+` line after it is an added line.
fn diff_items(text: &str) -> Vec<DiffItem> {
    if !text.lines().any(|l| l.starts_with("+++ ")) {
        return text
            .lines()
            .map(str::trim)
            .filter(|p| !p.is_empty() && p.contains('.'))
            .map(|p| DiffItem { file: p.to_string(), text: Some(p.to_string()) })
            .collect();
    }
    let mut out: Vec<DiffItem> = Vec::new();
    // (file, text, added lines, deleted file) of the file being read.
    let mut cur: Option<(String, String, usize, bool)> = None;
    let mut minus: Option<String> = None;
    let flush = |cur: Option<(String, String, usize, bool)>, out: &mut Vec<DiffItem>| {
        if let Some((file, text, added, gone)) = cur.filter(|c| !c.0.is_empty()) {
            let text = (added > 0 && !gone).then_some(text);
            out.push(DiffItem { file, text });
        }
    };
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            flush(cur.take(), &mut out);
            let p = header_path(p, "b/");
            let gone = p == "/dev/null";
            let file = if gone { minus.take().unwrap_or_default() } else { p.to_string() };
            cur = Some((file, format!("{line}\n"), 0, gone));
            continue;
        }
        if let Some(p) = line.strip_prefix("--- ") {
            minus = Some(header_path(p, "a/").to_string());
        }
        if let Some((_, text, added, _)) = cur.as_mut() {
            text.push_str(line);
            text.push('\n');
            if line.starts_with('+') {
                *added += 1;
            }
        }
    }
    flush(cur, &mut out);
    out
}

/// A `--- ` / `+++ ` header's path: the text before a tab, trimmed, `prefix`
/// (`a/` or `b/`) stripped.
fn header_path<'a>(p: &'a str, prefix: &str) -> &'a str {
    let p = p.split('\t').next().unwrap_or(p).trim();
    p.strip_prefix(prefix).unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(items: &[DiffItem]) -> Vec<(&str, bool)> {
        items.iter().map(|i| (i.file.as_str(), i.text.is_some())).collect()
    }

    #[test]
    fn diff_items_split_per_file_and_mark_unplaceable_ones() {
        let diff = "diff --git a/shop/a.py b/shop/a.py\n--- a/shop/a.py\n+++ b/shop/a.py\n@@ -1,3 +1,3 @@\n def f():\n-    x\n+    y\n\
diff --git a/shop/b.py b/shop/b.py\n--- a/shop/b.py\n+++ b/shop/b.py\n@@ -1,3 +1,2 @@\n def g():\n-    a\n     b\n\
diff --git a/shop/c.py b/shop/c.py\ndeleted file mode 100644\n--- a/shop/c.py\n+++ /dev/null\n@@ -1 +0,0 @@\n-def h(): pass\n\
--- /dev/null\n+++ b/shop/d.py\t2026-09-19\n@@ -0,0 +1 @@\n+def k(): pass\n";
        let items = diff_items(diff);
        assert_eq!(
            shape(&items),
            [("shop/a.py", true), ("shop/b.py", false), ("shop/c.py", false), ("shop/d.py", true)]
        );
        let a = items[0].text.as_deref().unwrap_or("");
        assert!(a.starts_with("+++ b/shop/a.py\n@@ -1,3 +1,3 @@\n") && a.contains("+    y\n"), "{a}");
    }

    #[test]
    fn diff_items_read_a_changed_file_list_line_by_line() {
        let items = diff_items("shop/a.py\n\n  README\ndocs/x.md\n");
        assert_eq!(shape(&items), [("shop/a.py", true), ("docs/x.md", true)]);
    }
}
