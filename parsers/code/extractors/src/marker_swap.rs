//! The post-cache marker-node swap, shared by the engine's two const folds.
//!
//! A per-file extractor names some marker sites by an identifier it cannot
//! resolve: a lookup reads other files, and the extractor's output is cached by
//! the file's own hash (the `constants.rs` CACHE RULE). So the engine re-runs
//! the extractor after the parse cache with the repo const table as a
//! resolver, and when a site folded it swaps the file's marker nodes for the
//! re-run's. LA.4's queue topics were the first ([`crate::queues::replace_queue_nodes`]),
//! CB.3b's constant-keyed event sites the second
//! ([`crate::eventbus::replace_event_nodes`]); this is the kind-agnostic body
//! they share. The caller picks WHICH nodes are replaced (`old`); [`swap`] does
//! the rest the same way for every kind.

use std::collections::HashSet;

use glia_code_domain::{
    CodeNav, FileParse, attach_imports_cell, cell_type, edge_category, evidence,
};
use glia_core::{Cell, Confidence, Edge, Node, NodeId, RepoId};

use crate::anchor::{self, Anchor};

/// The re-emitted marker nodes of one file, every side of one extractor
/// concatenated in the order the per-file pass adds them (queues: consumers
/// then producers; events: emitters then handlers).
pub(crate) struct MarkerNodes {
    pub(crate) nodes: Vec<Node>,
    /// Edges the extractor emits with its nodes (the queue side's
    /// `module -> node` CONTAINS). Empty for an extractor whose nodes reach
    /// the module only through [`anchor::attach`] (events).
    pub(crate) edges: Vec<Edge>,
    /// One nav per side, merged in order.
    pub(crate) navs: Vec<CodeNav>,
    /// Every site the fresh nodes were read at, for [`anchor::attach`].
    pub(crate) anchors: Vec<Anchor>,
}

/// Swap the `old` marker nodes of one file's parse for `fresh`.
///
/// Removed: the `old` nodes, the `module -> node` CONTAINS edges into them,
/// their name / qname / kind / parent entries and their ids in
/// `children_of[module_id]`. The fresh nodes, edges and child ids go back IN
/// PLACE — at the index the first removed node, edge and child id held — so a
/// folded file's parse is laid out as the per-file pass lays out the same file
/// written with literals.
///
/// The router gave every node of a language-parser parse the raw G15 IMPORTS
/// cell before the cache stored it. When the removed nodes carried it, the
/// fresh ones get it too (the cell `attach_imports_cell` computes from the
/// file's imports), LAST, after [`anchor::attach`] has added a POSITION to a
/// node that had none, so the cell order is the per-file pass's (extractor
/// cells, POSITION, IMPORTS) and the engine's A16.4 filter then rewrites the
/// cell like every other node's. `lang` is the engine's language tag for it.
///
/// LE.4c: the removed nodes' owner edges (`function -USES-> outbound marker`,
/// `inbound marker -HANDLED_BY-> function`, emitted by the per-file anchor
/// pass) go too, and [`anchor::attach`] re-anchors the fresh nodes from their
/// own sites (`path` is the file they were read from). The edges it adds —
/// owner edges, and the module CONTAINS fallback for a marker no function
/// encloses — take the removed owner edges' place (the end, when there were
/// none) and are stamped `extractor:anchor` rule `const_fold`, the post-cache
/// counterpart of the per-file `extractor:anchor` stamp.
///
/// LA.33: an old id the fresh set does not re-emit is GONE: every edge that
/// touches it and every ref from it is dropped. `after_anchor` runs once the
/// fresh nodes are anchored and before the stamp, so what it adds (the queue
/// side's consumer-callback re-bind) lands with the new owner edges; it must
/// stamp its own edges, which the const-fold stamp then leaves be.
pub(crate) fn swap(
    fp: &mut FileParse,
    module_id: NodeId,
    lang: &str,
    old: &HashSet<NodeId>,
    fresh: MarkerNodes,
    path: &str,
    after_anchor: impl FnOnce(&mut FileParse),
) {
    let owned_edge = |e: &Edge| {
        e.from == module_id && e.category == edge_category::CONTAINS && old.contains(&e.to)
    };
    let had_imports = fp
        .nodes
        .iter()
        .any(|n| old.contains(&n.id) && n.cells.iter().any(|c| c.kind == cell_type::IMPORTS));

    let node_at = fp
        .nodes
        .iter()
        .position(|n| old.contains(&n.id))
        .unwrap_or(fp.nodes.len());
    fp.nodes.retain(|n| !old.contains(&n.id));
    let edge_at = fp
        .edges
        .iter()
        .position(&owned_edge)
        .unwrap_or(fp.edges.len());
    fp.edges.retain(|e| !owned_edge(e));
    for id in old {
        fp.nav.name_by_id.remove(id);
        fp.nav.qname_by_id.remove(id);
        fp.nav.kind_by_id.remove(id);
        fp.nav.parent_of.remove(id);
    }
    let child_at = match fp.nav.children_of.get_mut(&module_id) {
        Some(children) => {
            let at = children
                .iter()
                .position(|c| old.contains(c))
                .unwrap_or(children.len());
            children.retain(|c| !old.contains(c));
            at
        }
        None => 0,
    };

    let MarkerNodes {
        nodes,
        edges,
        navs,
        mut anchors,
    } = fresh;
    // LA.33: a node the fresh set re-emits comes back with the SAME id; only
    // the ids it did not re-emit (a queue sentinel whose sites all folded, an
    // event constant path that resolved) are gone, and nothing may keep
    // naming them.
    let fresh_ids: HashSet<NodeId> = nodes.iter().map(|n| n.id).collect();
    let gone: HashSet<NodeId> = old.difference(&fresh_ids).copied().collect();
    let imports = had_imports.then(|| imports_cell(fp, lang)).flatten();
    fp.nodes.splice(node_at..node_at, nodes);
    fp.edges.splice(edge_at..edge_at, edges);
    let mut child_at = child_at;
    for nav in navs {
        fp.nav.name_by_id.extend(nav.name_by_id);
        fp.nav.qname_by_id.extend(nav.qname_by_id);
        fp.nav.kind_by_id.extend(nav.kind_by_id);
        fp.nav.parent_of.extend(nav.parent_of);
        for (parent, ids) in nav.children_of {
            let dst = fp.nav.children_of.entry(parent).or_default();
            if parent == module_id {
                let at = child_at.min(dst.len());
                let added = ids.len();
                dst.splice(at..at, ids);
                child_at = at + added;
            } else {
                dst.extend(ids);
            }
        }
    }

    // LE.4c: re-anchor. The old owner edges name ids that may be gone, and a
    // folded site is a new owner.
    let old_owner = |e: &Edge| {
        (e.category == edge_category::USES && old.contains(&e.to))
            || (e.category == edge_category::HANDLED_BY && old.contains(&e.from))
    };
    let owner_at = fp
        .edges
        .iter()
        .position(&old_owner)
        .unwrap_or(fp.edges.len());
    fp.edges.retain(|e| !old_owner(e));
    // LA.33: the owner sweep above already took the old inbound markers'
    // HANDLED_BY edges; no other edge and no ref may name a gone id either.
    // A same-id node's refs stay (a re-bind dedupes against them).
    fp.edges
        .retain(|e| !gone.contains(&e.from) && !gone.contains(&e.to));
    fp.refs.retain(|r| !gone.contains(&r.from));
    let tail = fp.edges.len();
    anchor::attach(fp, path, module_id, &mut anchors);
    after_anchor(fp);
    if let Some(cell) = imports {
        for n in fp.nodes.iter_mut().filter(|n| fresh_ids.contains(&n.id)) {
            n.cells.push(cell.clone());
        }
    }
    let mut added = fp.edges.split_off(tail);
    let ev = evidence::Evidence::emitter("extractor:anchor").rule("const_fold");
    evidence::stamp_missing_with(&mut added, &ev);
    let at = owner_at.min(fp.edges.len());
    fp.edges.splice(at..at, added);
}

/// The IMPORTS cell the router attaches to every node of `fp`
/// (`attach_imports_cell`, computed once from `fp.imports`), built on a
/// one-node scratch parse so the live parse's nodes are not touched.
fn imports_cell(fp: &FileParse, lang: &str) -> Option<Cell> {
    let mut scratch = FileParse {
        nodes: vec![Node {
            id: NodeId(0),
            repo: RepoId(0),
            confidence: Confidence::Weak,
            cells: Vec::new(),
        }],
        imports: fp.imports.clone(),
        ..Default::default()
    };
    attach_imports_cell(&mut scratch, lang);
    scratch.nodes.pop().and_then(|mut n| n.cells.pop())
}
