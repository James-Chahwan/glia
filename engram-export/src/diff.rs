//! The G16 incremental export (LG.8): what changed between two v6 engram
//! gmaps of one repo, as an [`engram_core::GmapDiff`] — the base is the prior
//! export Engram last applied, the target is the current one.
//!
//! The base is the prior EXPORTED gmap, not a glia build. The diff is over
//! exported facts (after the noise filter, doc cleaning and the concept /
//! identity hints), none of which exist in a glia graph delta, and Engram's
//! base is whatever it last applied, which is generally not HEAD. Diffing the
//! two exports is exact for that base and needs no second build; the
//! `base_digest` check makes applying the diff to any other base impossible on
//! Engram's side.
//!
//! ## Matching nodes
//!
//! Four passes, each over the nodes every earlier pass left unmatched on both
//! sides. A node's slot is the `(file token, kind)` of its `identity_hint`
//! (`<file token>:<kind>:<ordinal>`); the file token is carried across a file
//! move along a `--since` chain, so a slot survives the move.
//!
//! 1. **Keys.** A target node whose key exists in the base is that base node.
//!    Keys go first because they are exact: a same-file reorder shifts
//!    `identity_hint` ordinals, and matching hints first would cross-pair
//!    swapped siblings.
//! 2. **Routes, by path.** A ROUTE Symbol pairs within its file token and
//!    verb (the METHOD of `<METHOD> <path>`, or the `page:` / `route:`
//!    prefix, read through `endpoint::split_owner` and `nav::nav_route_path`;
//!    the owner is not compared) with the route whose path is equal, else
//!    alike up to a leading-segment prefix (`/trades/:id` ~
//!    `/api/trades/:id`: a mount prefix gained or lost). A pair needs the two
//!    to be each other's only fit among the free routes of the group, equal
//!    paths before alike ones. Routes go before hints because one route
//!    inserted ahead in a file shifts every later route's ordinal: CB.23 gave
//!    a mounted Go route its own `/api` ROUTE, and the ordinal hint then
//!    paired each route of the file with its neighbour's new one.
//! 3. **Symbols, by name.** Any other Symbol pairs by `(file token, kind,
//!    name)` when that triple is unique on both sides (LB.6's move-stable
//!    shape): a function inserted ahead in a moved file shifts its siblings'
//!    ordinals, not their names.
//! 4. **Identity hints.** An `identity_hint` that is `Some` and unique on
//!    each side pairs a base node with a target node: a move or a rename.
//!    Never two ROUTEs of different verbs or of paths not alike (counted
//!    [`DiffStats::hint_refused`]): such a route is a new contract, so the
//!    base is removed and the target added, and Engram mints it a fresh
//!    FactId instead of handing it a neighbour's.
//!
//! A pair whose bincode bytes are equal is unchanged and not written.
//! Otherwise it is a [`NodeChange`] carrying the base key as `prior_key`, and
//! `location_only` when the bytes are still equal after clearing the fields a
//! move may touch: the key, `Symbol.qname`, the span of a Symbol or a
//! Proposition (bytes and lines), `concept_hint` and `identity_hint`.
//! Unmatched target nodes are `added`, in target order; unmatched base keys are
//! `removed`, sorted.
//!
//! Keys are unique within one glia export (`build_gmap` drops a repeated
//! qname). On a hand-built gmap that repeats a key, the first node carrying it
//! is the one matched by key, later ones fall through to passes 2-4, and a
//! key still held by a matched base node is never listed as removed.
//!
//! ## Edges
//!
//! Base edge endpoints are remapped through the rename map (each pair's base
//! key → target key) before the two edge sets are compared, so a pure move
//! causes no edge churn. An edge is `(from, kind, to, weight bits)`: a weight
//! change is one removal plus one addition, a NaN weight still compares equal
//! to itself, and two byte-identical edges in one gmap are one edge — Engram's
//! own `(FactId, EdgeKind, FactId)` edge identity. Both edge lists are sorted
//! by that identity and carry target keys, except that an edge of a removed
//! node keeps that node's base key (removing it after the fact is gone is a
//! no-op for Engram).
//!
//! Every collection that reaches the output is a `BTreeMap` / `BTreeSet` or a
//! `Vec` in input order, so the diff's bytes are a pure function of the two
//! gmaps and its `content_digest` is a content address.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use engram_core::{
    Content, EdgeKind, GMAP_FORMAT_VERSION, Gmap, GmapDiff, GmapEdge, GmapNode, NodeChange,
    SpanRef, content_digest,
};
use glia_code_domain::{endpoint, node_kind};
use glia_graph::nav;

use crate::write_atomic;

/// Counts of one [`diff_gmaps`] run, for the caller's report line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DiffStats {
    /// Target nodes with no base counterpart (`GmapDiff.added`).
    pub added: usize,
    /// Base keys with no target counterpart (`GmapDiff.removed`).
    pub removed: usize,
    /// Matched nodes whose bytes differ (`GmapDiff.modified`).
    pub modified: usize,
    /// Of `modified`: those whose key changed (`prior_key != node.key`).
    pub moved: usize,
    /// Of `modified`: those flagged `location_only`.
    pub location_only: usize,
    /// `GmapDiff.edges_added`.
    pub edges_added: usize,
    /// `GmapDiff.edges_removed`.
    pub edges_removed: usize,
    /// Pairs made by the route pass (2). Key pairs (pass 1) are not counted:
    /// on a glia export, whose keys are unique, every pass 2-4 pair changes
    /// the key, so `by_route + by_name + by_hint == moved`.
    pub by_route: usize,
    /// Pairs made by the name pass (3).
    pub by_name: usize,
    /// Pairs made by the identity-hint pass (4).
    pub by_hint: usize,
    /// Unique hint pairs the hint pass refused: two ROUTEs of different verbs
    /// or of paths not alike. Each leaves one removed and one added node.
    pub hint_refused: usize,
}

/// An edge's identity: `(from, kind, to, weight bits)`.
type EdgeId<'a> = (&'a str, EdgeKind, &'a str, Option<u32>);

/// The diff that takes a store seeded from `base` (whose serialized bytes
/// hash to `base_digest`) to `target` (`target_digest`). See the module docs
/// for the matching order and the edge identity.
pub fn diff_gmaps(
    base: &Gmap,
    base_digest: u64,
    target: &Gmap,
    target_digest: u64,
) -> (GmapDiff, DiffStats) {
    // pair[t] = Some(b): target node t is base node b.
    let mut pair: Vec<Option<usize>> = vec![None; target.nodes.len()];
    let mut taken = vec![false; base.nodes.len()];

    // Pass 1 — keys.
    let mut base_by_key: BTreeMap<&str, usize> = BTreeMap::new();
    for (b, n) in base.nodes.iter().enumerate() {
        base_by_key.entry(n.key.as_str()).or_insert(b);
    }
    for (t, n) in target.nodes.iter().enumerate() {
        if let Some(&b) = base_by_key.get(n.key.as_str())
            && !taken[b]
        {
            taken[b] = true;
            pair[t] = Some(b);
        }
    }

    // Pass 2 — routes by path within (file token, verb).
    let by_route = pair_routes(base, target, &mut pair, &mut taken);
    // Pass 3 — other Symbols by (file token, kind, name).
    let by_name = pair_names(base, target, &mut pair, &mut taken);
    let mut stats = DiffStats {
        by_route,
        by_name,
        ..DiffStats::default()
    };

    // Pass 4 — identity hints unique on both sides among the unmatched, never
    // two routes of different verbs or unalike paths.
    let mut base_hints: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (b, n) in base.nodes.iter().enumerate() {
        if let (false, Some(h)) = (taken[b], n.identity_hint.as_deref()) {
            base_hints.entry(h).or_default().push(b);
        }
    }
    let mut target_hints: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (t, n) in target.nodes.iter().enumerate() {
        if let (None, Some(h)) = (pair[t], n.identity_hint.as_deref()) {
            target_hints.entry(h).or_default().push(t);
        }
    }
    for (hint, ts) in &target_hints {
        if let ([t], Some([b])) = (ts.as_slice(), base_hints.get(hint).map(Vec::as_slice)) {
            if !route_pair_ok(&base.nodes[*b], &target.nodes[*t]) {
                stats.hint_refused += 1;
                continue;
            }
            taken[*b] = true;
            pair[*t] = Some(*b);
            stats.by_hint += 1;
        }
    }

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut rename: BTreeMap<&str, &str> = BTreeMap::new();
    for (t, node) in target.nodes.iter().enumerate() {
        let Some(b) = pair[t] else {
            added.push(node.clone());
            continue;
        };
        let prior = &base.nodes[b];
        let moved = prior.key != node.key;
        if moved {
            rename.insert(prior.key.as_str(), node.key.as_str());
        }
        if same_bytes(prior, node) {
            continue;
        }
        let location_only = same_bytes(&without_location(prior), &without_location(node));
        stats.moved += usize::from(moved);
        stats.location_only += usize::from(location_only);
        modified.push(NodeChange {
            prior_key: prior.key.clone(),
            node: node.clone(),
            location_only,
        });
    }

    let kept: BTreeSet<&str> = base
        .nodes
        .iter()
        .zip(&taken)
        .filter_map(|(n, t)| t.then_some(n.key.as_str()))
        .collect();
    let removed: Vec<String> = base
        .nodes
        .iter()
        .zip(&taken)
        .filter_map(|(n, t)| (!t).then_some(n.key.as_str()))
        .filter(|k| !kept.contains(k))
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .map(str::to_string)
        .collect();

    let base_edges: BTreeSet<EdgeId> = base
        .edges
        .iter()
        .map(|e| {
            let from = rename
                .get(e.from.as_str())
                .copied()
                .unwrap_or(e.from.as_str());
            let to = rename.get(e.to.as_str()).copied().unwrap_or(e.to.as_str());
            (from, e.kind, to, e.weight.map(f32::to_bits))
        })
        .collect();
    let target_edges: BTreeSet<EdgeId> = target
        .edges
        .iter()
        .map(|e| {
            (
                e.from.as_str(),
                e.kind,
                e.to.as_str(),
                e.weight.map(f32::to_bits),
            )
        })
        .collect();
    let edges_removed: Vec<GmapEdge> = base_edges.difference(&target_edges).map(edge).collect();
    let edges_added: Vec<GmapEdge> = target_edges.difference(&base_edges).map(edge).collect();

    stats.added = added.len();
    stats.removed = removed.len();
    stats.modified = modified.len();
    stats.edges_added = edges_added.len();
    stats.edges_removed = edges_removed.len();
    let diff = GmapDiff {
        format_version: GMAP_FORMAT_VERSION,
        base_digest,
        target_digest,
        added,
        removed,
        modified,
        edges_added,
        edges_removed,
        files: target.files.clone(),
    };
    (diff, stats)
}

/// `(file token, kind)` of a `<file token>:<kind>:<ordinal>` identity hint,
/// split from the right as [`crate::prior_tokens`] does (a token holding `:`
/// survives). `None` for an unhinted node and for a hint whose ordinal is not
/// all digits or whose kind or token is empty.
fn hint_slot(n: &GmapNode) -> Option<(&str, &str)> {
    let mut parts = n.identity_hint.as_deref()?.rsplitn(3, ':');
    let (Some(ordinal), Some(kind), Some(token)) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let ordinal_ok = !ordinal.is_empty() && ordinal.bytes().all(|b| b.is_ascii_digit());
    (ordinal_ok && !kind.is_empty() && !token.is_empty()).then_some((token, kind))
}

/// The hint kind of a ROUTE: the decimal `node_kind::ROUTE` id
/// `build_identity_hints` writes.
fn is_route_kind(kind: &str) -> bool {
    kind.parse::<u32>() == Ok(node_kind::ROUTE.0)
}

/// A ROUTE: a Symbol whose hint slot names the ROUTE kind. Propositions (doc
/// sections, NatSpec facts) never are.
fn is_route(n: &GmapNode) -> bool {
    matches!(n.content, Content::Symbol { .. })
        && hint_slot(n).is_some_and(|(_, kind)| is_route_kind(kind))
}

/// `(verb, path)` of a ROUTE key, in every shape the tree emits: the owner
/// (` @<project>`) is split off with `endpoint::split_owner`, then
/// `nav::nav_route_path` reads `page:<p>` (client-router pages), `route:<p>`
/// (go / ts_routes) or `<METHOD> <p>`; the verb is what precedes the path —
/// `page:`, `route:` or the METHOD. `None` for any other key.
fn route_of(key: &str) -> Option<(&str, &str)> {
    let (q, _owner) = endpoint::split_owner(key);
    let path = nav::nav_route_path(q)?;
    let verb = q.strip_suffix(path)?.trim_end();
    Some((verb, path))
}

/// Two route paths are alike when equal, or when the shorter is `/`-rooted,
/// is not `/`, and ends the longer: a leading-segment prefix gained or lost
/// (`/trades/:id` ~ `/api/trades/:id`). The shorter starting with `/` puts
/// the match on a segment boundary (`/xtrades/:id` !~ `/trades/:id`). `/` is
/// alike only to itself: as a suffix it would fit every route of a file.
fn alike_paths(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.starts_with('/') && short != "/" && long.ends_with(short)
}

/// Whether the hint pass may pair `b` with `t`: always, unless both are
/// ROUTEs whose keys parse ([`route_of`]); then only with the same verb and
/// [`alike_paths`]. A ROUTE key in no known shape keeps the plain hint pair.
fn route_pair_ok(b: &GmapNode, t: &GmapNode) -> bool {
    if !(is_route(b) && is_route(t)) {
        return true;
    }
    match (route_of(&b.key), route_of(&t.key)) {
        (Some((bv, bp)), Some((tv, tp))) => bv == tv && alike_paths(bp, tp),
        _ => true,
    }
}

/// `(file token, verb, path)` of a ROUTE whose key parses.
fn route_slot(n: &GmapNode) -> Option<(&str, &str, &str)> {
    if !is_route(n) {
        return None;
    }
    let (token, _) = hint_slot(n)?;
    let (verb, path) = route_of(&n.key)?;
    Some((token, verb, path))
}

/// Pass 2: the unmatched ROUTEs of each `(file token, verb)` group pair by
/// path, equal paths first, then [`alike_paths`]. Within a tier a base and a
/// target pair only when each is the other's ONLY fit among the group's free
/// routes (snapshotted per tier), so the pairs do not depend on iteration
/// order; an ambiguous tail (`/offers` fits `/api/offers` and
/// `/api/user/offers`) stays unmatched for the guarded hint pass. Returns
/// the pairs made.
fn pair_routes(
    base: &Gmap,
    target: &Gmap,
    pair: &mut [Option<usize>],
    taken: &mut [bool],
) -> usize {
    type Side<'a> = Vec<(usize, &'a str)>;
    let mut groups: BTreeMap<(&str, &str), (Side, Side)> = BTreeMap::new();
    for (b, n) in base.nodes.iter().enumerate() {
        if !taken[b]
            && let Some((token, verb, path)) = route_slot(n)
        {
            groups.entry((token, verb)).or_default().0.push((b, path));
        }
    }
    for (t, n) in target.nodes.iter().enumerate() {
        if pair[t].is_none()
            && let Some((token, verb, path)) = route_slot(n)
        {
            groups.entry((token, verb)).or_default().1.push((t, path));
        }
    }
    let equal: fn(&str, &str) -> bool = |a, b| a == b;
    let mut made = 0;
    for (bs, ts) in groups.values() {
        if bs.is_empty() || ts.is_empty() {
            continue;
        }
        for fits in [equal, alike_paths] {
            let free_b: Side = bs.iter().filter(|(b, _)| !taken[*b]).copied().collect();
            let free_t: Side = ts
                .iter()
                .filter(|(t, _)| pair[*t].is_none())
                .copied()
                .collect();
            let mut pairs = Vec::new();
            for &(b, bp) in &free_b {
                let mut fit = free_t.iter().filter(|(_, tp)| fits(bp, tp));
                let (Some(&(t, tp)), None) = (fit.next(), fit.next()) else {
                    continue;
                };
                if free_b.iter().filter(|(_, p)| fits(p, tp)).count() == 1 {
                    pairs.push((b, t));
                }
            }
            for (b, t) in pairs {
                taken[b] = true;
                pair[t] = Some(b);
                made += 1;
            }
        }
    }
    made
}

/// `(file token, kind, name)` of a Symbol that is not a ROUTE.
fn name_slot(n: &GmapNode) -> Option<(&str, &str, &str)> {
    let Content::Symbol { name, .. } = &n.content else {
        return None;
    };
    let (token, kind) = hint_slot(n)?;
    (!is_route_kind(kind) && !name.is_empty()).then_some((token, kind, name.as_str()))
}

/// Pass 3: an unmatched non-ROUTE Symbol pairs with the one of its file
/// token, kind and name when that triple names exactly one node on each side
/// (two `get` methods of one file pair by neither). Returns the pairs made.
fn pair_names(base: &Gmap, target: &Gmap, pair: &mut [Option<usize>], taken: &mut [bool]) -> usize {
    type Slot<'a> = (&'a str, &'a str, &'a str);
    let mut groups: BTreeMap<Slot, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (b, n) in base.nodes.iter().enumerate() {
        if !taken[b]
            && let Some(slot) = name_slot(n)
        {
            groups.entry(slot).or_default().0.push(b);
        }
    }
    for (t, n) in target.nodes.iter().enumerate() {
        if pair[t].is_none()
            && let Some(slot) = name_slot(n)
        {
            groups.entry(slot).or_default().1.push(t);
        }
    }
    let mut made = 0;
    for (bs, ts) in groups.values() {
        if let ([b], [t]) = (bs.as_slice(), ts.as_slice()) {
            taken[*b] = true;
            pair[*t] = Some(*b);
            made += 1;
        }
    }
    made
}

/// Load a full engram gmap and the [`content_digest`] of the bytes read —
/// the digest a diff built on it names as `base_digest`. Refuses any
/// `format_version` but [`GMAP_FORMAT_VERSION`] before decoding (bincode is
/// positional, so a shape mismatch would decode to garbage), peeking the
/// first 4 bytes as Engram's own loader does; a decode error is `InvalidData`.
pub fn read_gmap(path: &Path) -> io::Result<(Gmap, u64)> {
    let bytes = std::fs::read(path)?;
    let Some(head) = bytes.first_chunk::<4>() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: too short to carry a gmap format_version",
                path.display()
            ),
        ));
    };
    let v = u32::from_le_bytes(*head);
    if v != GMAP_FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "prior gmap format_version {v} != {GMAP_FORMAT_VERSION}: a --since chain cannot \
                 cross a contract bump - run a full export and re-seed Engram"
            ),
        ));
    }
    let gmap: Gmap =
        bincode::deserialize(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((gmap, content_digest(&bytes)))
}

/// Write `diff` as bincode to `path` (tmp-then-rename) and return the
/// [`content_digest`] of the bytes written.
pub fn write_diff(path: &Path, diff: &GmapDiff) -> io::Result<u64> {
    let bytes =
        bincode::serialize(diff).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(path, &bytes)?;
    Ok(content_digest(&bytes))
}

/// `<out>.diff` — the diff lives beside the full target gmap it reaches.
pub fn diff_path(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".diff");
    PathBuf::from(s)
}

/// bincode equality. A node that fails to encode never compares equal, so it
/// is shipped as a (non-location-only) change rather than silently dropped.
fn same_bytes(a: &GmapNode, b: &GmapNode) -> bool {
    match (bincode::serialize(a), bincode::serialize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// `n` with every field a location-only change may touch cleared: the key,
/// `Symbol.qname`, the Symbol / Proposition span, `concept_hint` and
/// `identity_hint`.
fn without_location(n: &GmapNode) -> GmapNode {
    let mut n = n.clone();
    n.key.clear();
    n.concept_hint = None;
    n.identity_hint = None;
    match &mut n.content {
        Content::Symbol { qname, span, .. } => {
            *qname = None;
            *span = SpanRef::NONE;
        }
        Content::Proposition { span, .. } => *span = None,
        Content::Vector(_) => {}
    }
    n
}

fn edge(&(from, kind, to, weight): &EdgeId) -> GmapEdge {
    GmapEdge {
        from: from.to_string(),
        kind,
        to: to.to_string(),
        weight: weight.map(f32::from_bits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(file: u32, start_line: u32, end_line: u32) -> SpanRef {
        SpanRef {
            file,
            start: start_line * 10,
            end: end_line * 10 + 9,
            start_line,
            end_line,
        }
    }

    fn sym(key: &str, file: u32, line: u32, doc: &str, hint: &str) -> GmapNode {
        GmapNode {
            key: key.to_string(),
            content: Content::Symbol {
                name: key.rsplit("::").next().unwrap_or(key).to_string(),
                span: span(file, line, line + 2),
                qname: Some(key.to_string()),
                doc: (!doc.is_empty()).then(|| doc.to_string()),
                imports: Some(vec!["json".to_string()]),
            },
            provenance: None,
            concept_hint: key.rsplit_once("::").map(|(h, _)| h.to_string()),
            identity_hint: Some(hint.to_string()),
        }
    }

    fn prose(key: &str, text: &str, line: u32) -> GmapNode {
        GmapNode {
            key: key.to_string(),
            content: Content::Proposition {
                text: text.to_string(),
                span: Some(span(9, line, line + 3)),
            },
            provenance: None,
            concept_hint: Some("docs::README".to_string()),
            identity_hint: Some(format!("docs/README.md:section:{line}")),
        }
    }

    fn e(from: &str, kind: EdgeKind, to: &str, weight: Option<f32>) -> GmapEdge {
        GmapEdge {
            from: from.to_string(),
            kind,
            to: to.to_string(),
            weight,
        }
    }

    fn gmap(nodes: Vec<GmapNode>, edges: Vec<GmapEdge>) -> Gmap {
        Gmap {
            format_version: GMAP_FORMAT_VERSION,
            nodes,
            edges,
            files: BTreeMap::from([
                (1, "svc/users.py".to_string()),
                (9, "docs/README.md".to_string()),
            ]),
        }
    }

    fn keys(nodes: &[GmapNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.key.as_str()).collect()
    }

    /// A ROUTE Symbol keyed `key` at `ordinal` among the ROUTEs of `file`
    /// (its hint token): the hint `build_identity_hints` writes, and the key
    /// minus its owner as the name.
    fn route(key: &str, file: &str, ordinal: u32) -> GmapNode {
        GmapNode {
            key: key.to_string(),
            content: Content::Symbol {
                name: endpoint::split_owner(key).0.to_string(),
                span: span(1, 10 + 5 * ordinal, 12 + 5 * ordinal),
                qname: Some(key.to_string()),
                doc: None,
                imports: None,
            },
            provenance: None,
            concept_hint: None,
            identity_hint: Some(format!("{file}:{}:{ordinal}", node_kind::ROUTE.0)),
        }
    }

    /// `(prior_key, key)` of every modified entry, in diff order.
    fn moves(diff: &GmapDiff) -> Vec<(&str, &str)> {
        diff.modified
            .iter()
            .map(|c| (c.prior_key.as_str(), c.node.key.as_str()))
            .collect()
    }

    fn edge_ids(edges: &[GmapEdge]) -> Vec<(&str, EdgeKind, &str, Option<f32>)> {
        edges
            .iter()
            .map(|e| (e.from.as_str(), e.kind, e.to.as_str(), e.weight))
            .collect()
    }

    /// A small service: a module, a class, two methods and a doc section.
    fn service() -> Gmap {
        gmap(
            vec![
                sym("svc::users", 1, 1, "", "svc/users.py:module:0"),
                sym(
                    "svc::users::Users",
                    1,
                    3,
                    "User store.",
                    "svc/users.py:class:0",
                ),
                sym(
                    "svc::users::Users::load",
                    1,
                    5,
                    "Load one user.",
                    "svc/users.py:method:0",
                ),
                sym("svc::users::Users::save", 1, 9, "", "svc/users.py:method:1"),
                prose("docs::README::users", "Users are loaded on demand.", 1),
            ],
            vec![
                e(
                    "svc::users",
                    EdgeKind::Contains,
                    "svc::users::Users",
                    Some(1.0),
                ),
                e(
                    "svc::users::Users",
                    EdgeKind::Contains,
                    "svc::users::Users::load",
                    Some(1.0),
                ),
                e(
                    "svc::users::Users",
                    EdgeKind::Contains,
                    "svc::users::Users::save",
                    Some(1.0),
                ),
                e(
                    "svc::users::Users::save",
                    EdgeKind::Calls,
                    "svc::users::Users::load",
                    Some(0.8),
                ),
                e(
                    "docs::README::users",
                    EdgeKind::Documents,
                    "svc::users::Users::load",
                    Some(0.3),
                ),
            ],
        )
    }

    #[test]
    fn identical_is_empty() {
        let g = service();
        let (diff, stats) = diff_gmaps(&g, 11, &g.clone(), 11);
        assert_eq!(stats, DiffStats::default());
        assert!(diff.added.is_empty() && diff.removed.is_empty() && diff.modified.is_empty());
        assert!(diff.edges_added.is_empty() && diff.edges_removed.is_empty());
        assert_eq!(diff.format_version, GMAP_FORMAT_VERSION);
        assert_eq!((diff.base_digest, diff.target_digest), (11, 11));
        assert_eq!(diff.files, g.files);
    }

    #[test]
    fn add_remove_modify() {
        let base = service();
        let mut target = service();
        // save removed (with its edges); load's doc edited; the README section
        // shifted down two lines, text unchanged; a new `delete` method added.
        target.nodes.retain(|n| n.key != "svc::users::Users::save");
        target
            .edges
            .retain(|e| e.from != "svc::users::Users::save" && e.to != "svc::users::Users::save");
        target.nodes[2] = sym(
            "svc::users::Users::load",
            1,
            5,
            "Load one user by id.",
            "svc/users.py:method:0",
        );
        target.nodes[3] = prose("docs::README::users", "Users are loaded on demand.", 3);
        target.nodes.push(sym(
            "svc::users::Users::delete",
            1,
            9,
            "",
            "svc/users.py:method:2",
        ));
        target.edges.push(e(
            "svc::users::Users",
            EdgeKind::Contains,
            "svc::users::Users::delete",
            Some(1.0),
        ));
        target.files.insert(2, "svc/extra.py".to_string());

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert_eq!(keys(&diff.added), ["svc::users::Users::delete"]);
        assert_eq!(diff.removed, ["svc::users::Users::save"]);
        let changes: Vec<(&str, &str, bool)> = diff
            .modified
            .iter()
            .map(|c| (c.prior_key.as_str(), c.node.key.as_str(), c.location_only))
            .collect();
        assert_eq!(
            changes,
            [
                ("svc::users::Users::load", "svc::users::Users::load", false),
                ("docs::README::users", "docs::README::users", true),
            ]
        );
        assert_eq!(
            edge_ids(&diff.edges_removed),
            [
                (
                    "svc::users::Users",
                    EdgeKind::Contains,
                    "svc::users::Users::save",
                    Some(1.0)
                ),
                (
                    "svc::users::Users::save",
                    EdgeKind::Calls,
                    "svc::users::Users::load",
                    Some(0.8)
                ),
            ]
        );
        assert_eq!(
            edge_ids(&diff.edges_added),
            [(
                "svc::users::Users",
                EdgeKind::Contains,
                "svc::users::Users::delete",
                Some(1.0)
            )]
        );
        assert_eq!(diff.files, target.files);
        assert_eq!(
            stats,
            DiffStats {
                added: 1,
                removed: 1,
                modified: 2,
                moved: 0,
                location_only: 1,
                edges_added: 1,
                edges_removed: 2,
                by_route: 0,
                by_name: 0,
                by_hint: 0,
                hint_refused: 0,
            }
        );
    }

    #[test]
    fn move_by_hint() {
        let base = service();
        let mut target = service();
        // Users::load moved to another file: key, qname, span (file and lines)
        // and concept_hint change; the identity hint is carried across the move.
        target.nodes[2] = sym(
            "svc::store::Users::load",
            2,
            40,
            "Load one user.",
            "svc/users.py:method:0",
        );
        for edge in &mut target.edges {
            for end in [&mut edge.from, &mut edge.to] {
                if *end == "svc::users::Users::load" {
                    *end = "svc::store::Users::load".to_string();
                }
            }
        }

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert!(diff.added.is_empty() && diff.removed.is_empty());
        assert_eq!(diff.modified.len(), 1);
        let change = &diff.modified[0];
        assert_eq!(change.prior_key, "svc::users::Users::load");
        assert_eq!(change.node.key, "svc::store::Users::load");
        assert!(change.location_only);
        // Every edge into the moved node remaps onto its new key: no churn.
        assert!(diff.edges_added.is_empty() && diff.edges_removed.is_empty());
        assert_eq!(
            (stats.modified, stats.moved, stats.location_only),
            (1, 1, 1)
        );
        // A moved symbol whose name holds pairs by name, before its hint.
        assert_eq!((stats.by_name, stats.by_hint), (1, 0));
    }

    #[test]
    fn swapped_siblings_match_by_key() {
        let base = service();
        let mut target = service();
        // save now precedes load: ordinals, hints and spans swap between them.
        target.nodes[2] = sym(
            "svc::users::Users::load",
            1,
            9,
            "Load one user.",
            "svc/users.py:method:1",
        );
        target.nodes[3] = sym("svc::users::Users::save", 1, 5, "", "svc/users.py:method:0");

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert!(diff.added.is_empty() && diff.removed.is_empty());
        let changes: Vec<(&str, &str, bool)> = diff
            .modified
            .iter()
            .map(|c| (c.prior_key.as_str(), c.node.key.as_str(), c.location_only))
            .collect();
        assert_eq!(
            changes,
            [
                ("svc::users::Users::load", "svc::users::Users::load", true),
                ("svc::users::Users::save", "svc::users::Users::save", true),
            ]
        );
        assert!(diff.edges_added.is_empty() && diff.edges_removed.is_empty());
        assert_eq!((stats.moved, stats.location_only), (0, 2));
    }

    #[test]
    fn weight_change_is_remove_plus_add() {
        let mut base = service();
        let mut target = service();
        target.edges[3].weight = Some(0.5);
        // NaN is compared by bits, so an unchanged NaN weight is no churn.
        base.edges.push(e(
            "svc::users",
            EdgeKind::Cooccurs,
            "docs::README::users",
            Some(f32::NAN),
        ));
        target.edges.push(e(
            "svc::users",
            EdgeKind::Cooccurs,
            "docs::README::users",
            Some(f32::NAN),
        ));

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert!(diff.modified.is_empty());
        assert_eq!(
            edge_ids(&diff.edges_removed),
            [(
                "svc::users::Users::save",
                EdgeKind::Calls,
                "svc::users::Users::load",
                Some(0.8)
            )]
        );
        assert_eq!(
            edge_ids(&diff.edges_added),
            [(
                "svc::users::Users::save",
                EdgeKind::Calls,
                "svc::users::Users::load",
                Some(0.5)
            )]
        );
        assert_eq!((stats.edges_removed, stats.edges_added), (1, 1));
    }

    #[test]
    fn removed_node_edges_keep_base_keys() {
        let base = service();
        let mut target = service();
        // save is deleted and load is renamed (same hint): the removed edge
        // save -> load keeps save's base key and carries load's TARGET key.
        target.nodes.retain(|n| n.key != "svc::users::Users::save");
        target.nodes[2] = sym(
            "svc::users::Users::fetch",
            1,
            5,
            "Load one user.",
            "svc/users.py:method:0",
        );
        target.edges = vec![
            e(
                "svc::users",
                EdgeKind::Contains,
                "svc::users::Users",
                Some(1.0),
            ),
            e(
                "svc::users::Users",
                EdgeKind::Contains,
                "svc::users::Users::fetch",
                Some(1.0),
            ),
            e(
                "docs::README::users",
                EdgeKind::Documents,
                "svc::users::Users::fetch",
                Some(0.3),
            ),
        ];

        let (diff, _) = diff_gmaps(&base, 1, &target, 2);
        assert_eq!(diff.removed, ["svc::users::Users::save"]);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.modified[0].prior_key, "svc::users::Users::load");
        assert_eq!(diff.modified[0].node.key, "svc::users::Users::fetch");
        // The name changed with the key, so it is not location-only.
        assert!(!diff.modified[0].location_only);
        assert_eq!(
            edge_ids(&diff.edges_removed),
            [
                (
                    "svc::users::Users",
                    EdgeKind::Contains,
                    "svc::users::Users::save",
                    Some(1.0)
                ),
                (
                    "svc::users::Users::save",
                    EdgeKind::Calls,
                    "svc::users::Users::fetch",
                    Some(0.8)
                ),
            ]
        );
        assert!(diff.edges_added.is_empty());
    }

    #[test]
    fn diff_bytes_deterministic() {
        let base = service();
        let mut target = service();
        target.nodes.reverse();
        target.nodes.retain(|n| n.key != "svc::users");
        target.nodes.push(sym(
            "svc::users::Users::delete",
            1,
            12,
            "",
            "svc/users.py:method:2",
        ));
        target.nodes.push(sym(
            "svc::users::Users::audit",
            1,
            15,
            "",
            "svc/users.py:method:3",
        ));
        target.edges.reverse();
        target.edges.retain(|e| e.from != "svc::users");
        for edge in &mut target.edges {
            if edge.kind == EdgeKind::Documents {
                edge.weight = Some(0.4);
            }
        }
        target.edges.push(e(
            "svc::users::Users::delete",
            EdgeKind::Calls,
            "svc::users::Users::save",
            Some(0.8),
        ));
        target.edges.push(e(
            "svc::users::Users::audit",
            EdgeKind::Calls,
            "svc::users::Users::load",
            None,
        ));

        let (first, _) = diff_gmaps(&base, 7, &target, 8);
        let (second, _) = diff_gmaps(&base, 7, &target, 8);
        let a = bincode::serialize(&first).unwrap();
        assert_eq!(a, bincode::serialize(&second).unwrap());
        // added keeps target order; removed and both edge lists are sorted.
        assert_eq!(
            keys(&first.added),
            ["svc::users::Users::delete", "svc::users::Users::audit"]
        );
        assert_eq!(first.removed, ["svc::users"]);
        assert_eq!(
            edge_ids(&first.edges_removed),
            [
                (
                    "docs::README::users",
                    EdgeKind::Documents,
                    "svc::users::Users::load",
                    Some(0.3)
                ),
                (
                    "svc::users",
                    EdgeKind::Contains,
                    "svc::users::Users",
                    Some(1.0)
                ),
            ]
        );
        assert_eq!(
            edge_ids(&first.edges_added),
            [
                (
                    "docs::README::users",
                    EdgeKind::Documents,
                    "svc::users::Users::load",
                    Some(0.4)
                ),
                (
                    "svc::users::Users::audit",
                    EdgeKind::Calls,
                    "svc::users::Users::load",
                    None
                ),
                (
                    "svc::users::Users::delete",
                    EdgeKind::Calls,
                    "svc::users::Users::save",
                    Some(0.8)
                ),
            ]
        );

        let dir =
            std::env::temp_dir().join(format!("glia-lg8-{}-deterministic", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = diff_path(&dir.join("svc.engram-gmap"));
        assert_eq!(
            out.file_name().and_then(|n| n.to_str()),
            Some("svc.engram-gmap.diff")
        );
        let digest = write_diff(&out, &first).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), a);
        assert_eq!(digest, content_digest(&a));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mounted_routes_pair_by_path_not_ordinal() {
        // CB.23's case: the file's routes gain a mount prefix and a route that
        // used to share its qname with a test file's twin becomes its own
        // ROUTE at ordinal 1, shifting every later ordinal by one. The ordinal
        // hint alone pairs fund -> `POST /api/trades`, confirm -> fund and
        // proposals -> confirm.
        let f = "h/trades.go";
        let base = gmap(
            vec![
                route("GET /trades/history @api", f, 0),
                route("POST /trades/:id/fund @api", f, 1),
                route("POST /trades/:id/confirm @api", f, 2),
                route("GET /trades/:id/dispute/proposals @api", f, 3),
            ],
            vec![],
        );
        let target = gmap(
            vec![
                route("GET /api/trades/history @api", f, 0),
                route("POST /api/trades @api", f, 1),
                route("POST /api/trades/:id/fund @api", f, 2),
                route("POST /api/trades/:id/confirm @api", f, 3),
                route("GET /api/trades/:id/dispute/proposals @api", f, 4),
            ],
            vec![],
        );

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert_eq!(
            moves(&diff),
            [
                ("GET /trades/history @api", "GET /api/trades/history @api"),
                (
                    "POST /trades/:id/fund @api",
                    "POST /api/trades/:id/fund @api"
                ),
                (
                    "POST /trades/:id/confirm @api",
                    "POST /api/trades/:id/confirm @api"
                ),
                (
                    "GET /trades/:id/dispute/proposals @api",
                    "GET /api/trades/:id/dispute/proposals @api"
                ),
            ]
        );
        assert_eq!(keys(&diff.added), ["POST /api/trades @api"]);
        assert!(diff.removed.is_empty(), "{:?}", diff.removed);
        assert_eq!(
            (
                stats.by_route,
                stats.by_name,
                stats.by_hint,
                stats.hint_refused,
                stats.moved
            ),
            (4, 0, 0, 0, 4)
        );
    }

    #[test]
    fn hint_pass_refuses_unalike_routes() {
        // Each base route's hint is unique on both sides and names a route of
        // another path or verb: a new contract, never a move.
        let f = "f.go";
        let base = gmap(
            vec![
                route("GET /reports @a", f, 0),
                route("POST /orders @a", f, 1),
                route("GET / @a", f, 2),
                route("page:/kyc @a", f, 3),
            ],
            vec![],
        );
        let target = gmap(
            vec![
                route("GET /exports @a", f, 0),
                route("PUT /orders @a", f, 1),
                route("GET /api @a", f, 2),
                route("page:/admin @a", f, 3),
            ],
            vec![],
        );

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert!(diff.modified.is_empty(), "{:?}", moves(&diff));
        assert_eq!(
            diff.removed,
            [
                "GET / @a",
                "GET /reports @a",
                "POST /orders @a",
                "page:/kyc @a"
            ]
        );
        assert_eq!(
            keys(&diff.added),
            [
                "GET /exports @a",
                "PUT /orders @a",
                "GET /api @a",
                "page:/admin @a"
            ]
        );
        assert_eq!(
            (stats.hint_refused, stats.by_route, stats.by_hint),
            (4, 0, 0)
        );

        assert_eq!(route_of("GET /a/b @svc"), Some(("GET", "/a/b")));
        assert_eq!(route_of("page:/kyc @web"), Some(("page:", "/kyc")));
        assert_eq!(route_of("route:/x"), Some(("route:", "/x")));
        assert_eq!(route_of("orders::list"), None);
        assert!(alike_paths("/trades/:id/fund", "/api/trades/:id/fund"));
        assert!(!alike_paths("/xtrades/:id", "/trades/:id"));
        assert!(!alike_paths("/", "/api"));
        assert!(!alike_paths(
            "/trades/:id/dispute",
            "/trades/:id/dispute/propose"
        ));
    }

    #[test]
    fn ambiguous_route_tails_fall_back_to_the_guarded_hint() {
        // `/offers` ends both target paths, so the route pass pairs neither;
        // the hint pass pairs both by ordinal, and both pairs are alike.
        let f = "h/offers.go";
        let base = gmap(
            vec![
                route("GET /offers @a", f, 0),
                route("GET /user/offers @a", f, 1),
            ],
            vec![],
        );
        let target = gmap(
            vec![
                route("GET /api/offers @a", f, 0),
                route("GET /api/user/offers @a", f, 1),
            ],
            vec![],
        );

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert_eq!(
            moves(&diff),
            [
                ("GET /offers @a", "GET /api/offers @a"),
                ("GET /user/offers @a", "GET /api/user/offers @a"),
            ]
        );
        assert!(diff.added.is_empty() && diff.removed.is_empty());
        assert_eq!(
            (stats.by_route, stats.by_hint, stats.hint_refused),
            (0, 2, 0)
        );
    }

    #[test]
    fn moved_file_with_an_insert_pairs_by_name() {
        // svc/users.py moved to svc/store.py (its token carried along the
        // --since chain) and gained `create` ahead of `load`: every key
        // changes and every ordinal shifts by one. The ordinal hint alone
        // pairs load -> create and save -> load.
        let hint = |ordinal: u32| format!("svc/users.py:{}:{ordinal}", node_kind::FUNCTION.0);
        let base = gmap(
            vec![
                sym("svc::users::load", 1, 3, "", &hint(0)),
                sym("svc::users::save", 1, 8, "", &hint(1)),
            ],
            vec![],
        );
        let mut target = gmap(
            vec![
                sym("svc::store::create", 2, 3, "", &hint(0)),
                sym("svc::store::load", 2, 8, "", &hint(1)),
                sym("svc::store::save", 2, 13, "", &hint(2)),
            ],
            vec![],
        );
        target.files.insert(2, "svc/store.py".to_string());

        let (diff, stats) = diff_gmaps(&base, 1, &target, 2);
        assert_eq!(
            moves(&diff),
            [
                ("svc::users::load", "svc::store::load"),
                ("svc::users::save", "svc::store::save"),
            ]
        );
        assert_eq!(keys(&diff.added), ["svc::store::create"]);
        assert!(diff.removed.is_empty(), "{:?}", diff.removed);
        assert_eq!((stats.by_name, stats.by_hint), (2, 0));
    }

    #[test]
    fn read_gmap_refuses_other_versions() {
        let dir = std::env::temp_dir().join(format!("glia-lg8-{}-versions", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("v5.engram-gmap");
        let mut bytes = 5u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 32]);
        std::fs::write(&old, &bytes).unwrap();
        let err = read_gmap(&old).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let msg = err.to_string();
        assert!(msg.contains("format_version 5"), "{msg}");
        assert!(msg.contains("full export"), "{msg}");

        let short = dir.join("short.engram-gmap");
        std::fs::write(&short, [6u8, 0]).unwrap();
        assert_eq!(
            read_gmap(&short).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let garbled = dir.join("garbled.engram-gmap");
        let mut bytes = GMAP_FORMAT_VERSION.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xff; 12]);
        std::fs::write(&garbled, &bytes).unwrap();
        assert_eq!(
            read_gmap(&garbled).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        // A current-version gmap loads, digested over exactly the bytes read.
        let good = dir.join("svc.engram-gmap");
        let g = service();
        let bytes = bincode::serialize(&g).unwrap();
        std::fs::write(&good, &bytes).unwrap();
        let (back, digest) = read_gmap(&good).unwrap();
        assert_eq!(digest, content_digest(&bytes));
        assert_eq!(bincode::serialize(&back).unwrap(), bytes);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
