//! The toy-reel domain's profile: [`TOY_TABLES`] (the data every
//! domain-agnostic layer reads), [`TOY_PASSES`] (its build passes, run by
//! activation's `PassRegistry`) and [`TOY_PROFILE`] (the two together), plus
//! its activation hooks [`KindIs`] and [`ScreenTimeSummary`].
//!
//! The passes are the 2026-04-17 design memo's video primitives at toy scale:
//! `reidentify_objects` is its ObjectReidentificationResolver (the same
//! object seen in two shots), `screen_time` a derived cell over the resolved
//! graph, `sort_edges` the determinism sort.

use std::collections::HashMap;

use glia_activation::passes::{PassRegistry, PassSpec, Stage};
use glia_activation::profile::{
    ActivationPreset, DomainProfile, DomainTables, EntryRule, Registries,
};
use glia_activation::{ActivatedView, FilterPredicate, SynthCell, SynthHook};
use glia_core::{CellPayload, Confidence, Edge, NodeId, NodeKindId, canonical_edge_cmp};

use crate::reel::{GRAPH_TYPE, ToyGraph};
use crate::registry::cell_type::{self as ct, SCREEN_TIME};
use crate::registry::edge_category::{self as ec, CONTAINS_SHOT, FEATURES, NEXT_SHOT, SAME_OBJECT};
use crate::registry::node_kind::{self as nk, OBJECT, SCENE, SHOT};

/// The domain's tables. Scenes are the entrypoints (nodes have no names, so no
/// named rule); every edge carries reachability; the domain has no effects;
/// every edge groups communities (CD.1c), weighted as its activation weight:
/// the same object seen twice binds hardest, a scene's shot list loosest.
pub const TOY_TABLES: DomainTables = DomainTables {
    graph_type: GRAPH_TYPE,
    registries: Registries {
        node_kinds: nk::ALL,
        edge_categories: ec::ALL,
        cell_types: ct::ALL,
    },
    entry: EntryRule {
        kinds: &[SCENE],
        roles: &[],
        named: &[],
    },
    carry_edges: &[CONTAINS_SHOT, NEXT_SHOT, FEATURES, SAME_OBJECT],
    effect_sinks: &[],
    activation_weights: &[
        (NEXT_SHOT, 3.0),
        (FEATURES, 2.0),
        (SAME_OBJECT, 4.0),
        (CONTAINS_SHOT, 1.0),
    ],
    activation_presets: &[ActivationPreset {
        name: "objects",
        overrides: &[(SAME_OBJECT, 8.0)],
    }],
    community_weights: &[
        (NEXT_SHOT, 3),
        (FEATURES, 2),
        (SAME_OBJECT, 4),
        (CONTAINS_SHOT, 1),
    ],
};

/// The domain's build passes, over a built [`ToyGraph`], no build context.
pub const TOY_PASSES: PassRegistry<ToyGraph> = PassRegistry::new(&[
    PassSpec {
        name: "reidentify_objects",
        stage: Stage::Resolve,
        after: &[],
        populates: &[],
        run: reidentify_objects,
    },
    PassSpec {
        name: "screen_time",
        stage: Stage::Post,
        after: &["reidentify_objects"],
        populates: &[SCREEN_TIME],
        run: screen_time,
    },
    PassSpec {
        name: "sort_edges",
        stage: Stage::Finalize,
        after: &[],
        populates: &[],
        run: sort_edges,
    },
]);

/// The domain's whole profile.
pub static TOY_PROFILE: DomainProfile<ToyGraph> = DomainProfile {
    tables: TOY_TABLES,
    passes: TOY_PASSES,
};

/// Resolve: one SAME_OBJECT edge, lower `NodeId` to higher, for every pair of
/// objects with equal LABELs. An edge already present is not added again.
fn reidentify_objects(g: &mut ToyGraph, _: &()) {
    let mut objects: Vec<(NodeId, &str)> = g
        .ids_of(OBJECT)
        .into_iter()
        .filter_map(|id| g.label(id).map(|l| (id, l)))
        .collect();
    objects.sort_by_key(|(id, _)| id.0);
    let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
    for (i, (a, label)) in objects.iter().enumerate() {
        pairs.extend(
            objects[i + 1..]
                .iter()
                .filter(|(_, l)| l == label)
                .map(|(b, _)| (*a, *b)),
        );
    }
    for (a, b) in pairs {
        if !g.edges.iter().any(|e| e.key() == (a, b, SAME_OBJECT)) {
            g.edges
                .push(Edge::new(a, b, SAME_OBJECT, Confidence::Medium));
        }
    }
}

/// Post: every object's SCREEN_TIME, `{"ms":N,"shots":M}` - the distinct
/// shots FEATURING any object of its SAME_OBJECT group (the connected
/// component over SAME_OBJECT edges), and the sum of their TIMECODE
/// durations.
fn screen_time(g: &mut ToyGraph, _: &()) {
    let objects = g.ids_of(OBJECT);
    // Looked up, never iterated.
    let pos: HashMap<NodeId, usize> = objects.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut parent: Vec<usize> = (0..objects.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for e in g.edges.iter().filter(|e| e.category == SAME_OBJECT) {
        if let (Some(&a), Some(&b)) = (pos.get(&e.from), pos.get(&e.to)) {
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent[ra.max(rb)] = ra.min(rb);
        }
    }
    let mut shots: Vec<Vec<NodeId>> = vec![Vec::new(); objects.len()];
    for e in g
        .edges
        .iter()
        .filter(|e| e.category == FEATURES && g.nav.kind(e.from) == Some(SHOT))
    {
        if let Some(&o) = pos.get(&e.to) {
            shots[root(&mut parent, o)].push(e.from);
        }
    }
    for group in &mut shots {
        group.sort_by_key(|id| id.0);
        group.dedup();
    }
    for (i, &id) in objects.iter().enumerate() {
        let group = &shots[root(&mut parent, i)];
        let ms: u64 = group
            .iter()
            .filter_map(|s| g.timecode(*s))
            .map(|(start, end)| end.saturating_sub(start))
            .sum();
        let json = format!("{{\"ms\":{ms},\"shots\":{}}}", group.len());
        g.set_cell(id, SCREEN_TIME, CellPayload::Json(json));
    }
}

/// Finalize: edges in the canonical order (`from`, `to`, `category`, ...).
fn sort_edges(g: &mut ToyGraph, _: &()) {
    g.edges.sort_by(canonical_edge_cmp);
}

// ============================================================================
// Activation hooks
// ============================================================================

/// Keeps the nodes of one kind, read from the graph's [`crate::ReelNav`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindIs(pub NodeKindId);

impl FilterPredicate<ToyGraph> for KindIs {
    fn name(&self) -> &'static str {
        "kind_is"
    }

    fn keep(&self, g: &ToyGraph, id: NodeId, _score: f64) -> bool {
        g.nav.kind(id) == Some(self.0)
    }
}

/// One cell per OBJECT in the view, in view order:
/// `"<label>: <ms> ms across <n> shots"`. An object without a SCREEN_TIME
/// (the passes have not run) gets none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenTimeSummary;

impl SynthHook<ToyGraph> for ScreenTimeSummary {
    fn name(&self) -> &'static str {
        "screen_time_summary"
    }

    fn synth(&self, g: &ToyGraph, view: &ActivatedView) -> Vec<SynthCell> {
        let mut out = Vec::new();
        for &(id, score) in &view.scores {
            if g.nav.kind(id) != Some(OBJECT) {
                continue;
            }
            let (Some(label), Some((ms, shots)), Some(index)) =
                (g.label(id), g.screen_time(id), g.nav.index(id))
            else {
                continue;
            };
            out.push(SynthCell {
                hook: self.name(),
                id: u64::from(index),
                key: format!("object/{index}"),
                anchor: Some(id),
                text: format!("{label}: {ms} ms across {shots} shots"),
                score,
                attrs: vec![("ms", ms.to_string()), ("shots", shots.to_string())],
            });
        }
        out
    }
}
