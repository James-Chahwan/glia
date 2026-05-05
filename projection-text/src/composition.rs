//! A+ composition cell synthesis.
//!
//! Given an activated set (typically the top-K of a PPR run) and a `RepoGraph`,
//! enumerate multi-hop access paths from activated *source endpoints* (methods)
//! to activated *target attributes*. Produce text cells suitable for appending
//! to a `summaries.json` pool where they will later be vectorised and injected
//! through the latent-embedding path alongside atomic node summaries.
//!
//! **Domain-general.** This module contains no codebase-specific strings,
//! hardcoded class names, or per-library hooks. It walks:
//!
//!   * `HAS_ATTRIBUTE`   — class → attribute (used as the final hop and as
//!                         the anchor of the target side)
//!   * `INHERITS_FROM`   — class → superclass (MRO-style attribute resolution)
//!   * `DEFINES`         — class → method (to identify intermediate callable
//!                         hops when the method carries a return-type edge)
//!   * `RETURNS_TYPE`    — method → class (explicit return-type annotations,
//!                         typically emitted from `def m() -> T:` signatures)
//!
//! Additionally a **general** docstring-fallback heuristic is applied: when a
//! method has a `DOC` cell whose first sentence contains backtick-quoted
//! identifiers, each such identifier is treated as a candidate return class.
//! This is a conservative, widely-used Python convention (see PEP 257 examples
//! and most reStructuredText-style docstrings) — nothing library-specific.
//!
//! The output is not persisted into `.gmap`; paths are a pure function of
//! `(graph, activated_set)` computed per query. See `composition_cells` memory
//! for the activation-time-only design choice.

use std::collections::{HashMap, HashSet, VecDeque};

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{CellPayload, NodeId};
use repo_graph_graph::RepoGraph;

// ============================================================================
// Public types
// ============================================================================

/// How a single access hop was derived. Drives the human-readable annotation
/// in the cell text; no behavioural effect on BFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopKind {
    /// `x.m` where `m` is a method/property whose return class was annotated
    /// explicitly via `-> T:`.
    TypedReturn,
    /// `x.m` where `m` is a method/property whose return class was inferred
    /// from a backtick-quoted identifier in the first sentence of its
    /// docstring.
    DocstringReturn,
    /// `x.a` where `a` is a data attribute defined on the current class.
    /// Always the terminal hop of a path.
    DataAttribute,
    /// Attribute lookup resolved via an inherited superclass (MRO). Does not
    /// consume a textual hop — kept for derivation rendering.
    InheritedLookup,
}

/// One hop in a synthesised access path.
#[derive(Debug, Clone)]
pub struct Hop {
    /// Class the accessor was resolved on (may differ from the enclosing
    /// class of the starting method when inheritance was walked).
    pub owning_class: NodeId,
    /// The method or attribute node being accessed.
    pub via: NodeId,
    /// Short identifier for text rendering (e.g. `"root"`, `"opts"`).
    pub name: String,
    /// Derivation tag for text annotation.
    pub kind: HopKind,
    /// Next class the accessor "returns" (for method hops) or the class that
    /// actually owned `via` (for inherited data-attribute hops). `None` for
    /// terminal data-attribute hops.
    pub returns: Option<NodeId>,
}

/// A full synthesised path from a source class (receiver of `self`) to a
/// target attribute.
#[derive(Debug, Clone)]
pub struct AccessPath {
    /// Enclosing class of the source method — where `self` is typed.
    pub start_class: NodeId,
    /// Final activated attribute node the path reaches.
    pub target_attr: NodeId,
    /// Ordered hops. Last hop is always `HopKind::DataAttribute` (the target).
    pub hops: Vec<Hop>,
    /// e.g. `"self.root.opts"`.
    pub expression: String,
    /// Human-readable derivation annotation, one line per hop.
    /// e.g. `"Field.root (@property, docstring says Schema) → Schema"`.
    pub derivation: String,
    /// Pre-computed blended (src+tgt)/2 activation score. Tgt falls back
    /// from the target attr (rarely activated) to its parent class, then
    /// via MRO-propagation to any activated descendant of that class.
    /// Lets downstream selection rank gold-tree paths above structural noise.
    pub score: f64,
}

/// A rendered composition cell ready to serialise into the summaries pool.
/// Field shape mirrors the atomic-node summary shape consumed downstream by
/// the Path B latent-injection pipeline.
#[derive(Debug, Clone)]
pub struct CompositionCell {
    /// Synthetic qname so downstream dedup by qname still works.
    /// Form: `"synth::AccessPath::<expression>"`.
    pub qname: String,
    /// Free-text cell body — the pooled vector is computed from this string.
    pub summary: String,
    /// Blended score from the source and target activation ranks. Callers
    /// using a straight top-K cutoff can use this directly; callers doing
    /// category-based budgets can ignore it.
    pub score: f64,
    /// Synthetic `u64` id stable per (expression, start_class, target_attr) —
    /// same shape the downstream `run_pathB` consumer expects.
    pub id: u64,
}

// ============================================================================
// Synthesis entry point
// ============================================================================

/// Enumerate synthesised access paths within the activated set.
///
/// `activated` is the PPR top-K (order does not matter; membership does).
/// `scores` optionally carries the per-node activation score, used to blend
/// per-cell `score`. Pass an empty slice to fall back to a neutral 0.5 score.
/// `max_hops` caps path length including the final data-attribute hop.
pub fn synth_paths(
    activated: &[NodeId],
    scores: &[(NodeId, f64)],
    graph: &RepoGraph,
    max_hops: usize,
) -> Vec<AccessPath> {
    if max_hops < 2 {
        // At minimum we need one traversal hop + one terminal attr hop.
        return Vec::new();
    }
    let activated_set: HashSet<NodeId> = activated.iter().copied().collect();
    let score_by_id: HashMap<NodeId, f64> = scores.iter().copied().collect();
    let _ = &score_by_id; // silence if future callers swap signature

    let idx = EdgeIndex::build(graph);

    // Expand activated CLASSes to include their MRO ancestors: Python attribute
    // lookup walks inheritance, so if `Schema` is activated then attrs defined
    // on `BaseSchema` (its superclass) are part of `Schema`'s surface.
    let mut classes_in_scope: HashSet<NodeId> = HashSet::new();
    // MRO-propagated class scores: each ancestor inherits max score among all
    // activated descendants that reach it. Lets target attrs on non-activated
    // ancestor classes (e.g. `BaseSchema.opts` reached via activated `Schema`)
    // carry the descendant's activation into path scoring.
    let mut class_eff: HashMap<NodeId, f64> = HashMap::new();
    for &id in activated {
        if graph.nav.kind_by_id.get(&id).copied() == Some(node_kind::CLASS) {
            classes_in_scope.insert(id);
            let s = score_by_id.get(&id).copied().unwrap_or(0.5);
            let e = class_eff.entry(id).or_insert(s);
            if s > *e { *e = s; }
            for a in ancestors_of(id, &idx) {
                classes_in_scope.insert(a);
                let e = class_eff.entry(a).or_insert(s);
                if s > *e { *e = s; }
            }
        }
    }

    // Source endpoints: every activated METHOD whose enclosing parent is a
    // CLASS. `self` in such a method is typed as that class.
    let mut sources: Vec<(NodeId, NodeId)> = Vec::new();
    for &id in activated {
        if graph.nav.kind_by_id.get(&id).copied() != Some(node_kind::METHOD) {
            continue;
        }
        let Some(&parent) = graph.nav.parent_of.get(&id) else { continue };
        if graph.nav.kind_by_id.get(&parent).copied() == Some(node_kind::CLASS) {
            sources.push((id, parent));
        }
    }

    // Target attributes: any ATTRIBUTE whose parent CLASS is activated, OR the
    // attribute itself is activated. PPR structurally under-activates data
    // attributes in code graphs (typically ~1% of ATTRIBUTE nodes make the
    // top-K); gating on "parent class activated" keeps the enumeration inside
    // the activated subgraph while still producing attribute targets.
    let mut seen_attrs: HashSet<NodeId> = HashSet::new();
    let mut targets: Vec<(NodeId, NodeId, String)> = Vec::new();
    for (&id, &kind) in &graph.nav.kind_by_id {
        if kind != node_kind::ATTRIBUTE {
            continue;
        }
        let Some(&parent) = graph.nav.parent_of.get(&id) else { continue };
        if graph.nav.kind_by_id.get(&parent).copied() != Some(node_kind::CLASS) {
            continue;
        }
        if !activated_set.contains(&id) && !classes_in_scope.contains(&parent) {
            continue;
        }
        let Some(name) = graph.nav.name_by_id.get(&id).cloned() else { continue };
        if seen_attrs.insert(id) {
            targets.push((id, parent, name));
        }
    }
    // Determinism: targets were pushed in graph.nav.kind_by_id (HashMap) iter
    // order. Path enumeration walks sources × targets and that order propagates
    // into the AccessPath Vec → CompositionCells → summaries-aplus.json. Sort
    // by attribute qname (with NodeId tiebreak) so the synthesized cells land
    // in stable order across runs.
    targets.sort_by(|a, b| {
        let qa = graph.nav.qname_by_id.get(&a.0).map(String::as_str).unwrap_or("");
        let qb = graph.nav.qname_by_id.get(&b.0).map(String::as_str).unwrap_or("");
        qa.cmp(qb).then_with(|| a.0.0.cmp(&b.0.0))
    });

    if sources.is_empty() || targets.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<AccessPath> = Vec::new();
    for &(_src_method, src_class) in &sources {
        for (tgt_attr, tgt_class, tgt_name) in &targets {
            // BFS on classes from src_class looking for tgt_class (or any
            // ancestor of tgt_class — attribute lookup walks inheritance on
            // the target side too).
            let paths = bfs_class_paths(
                src_class,
                *tgt_class,
                &idx,
                graph,
                max_hops.saturating_sub(1),
                &activated_set,
            );
            for mut hops in paths {
                // Final data-attribute hop.
                hops.push(Hop {
                    owning_class: *tgt_class,
                    via: *tgt_attr,
                    name: tgt_name.clone(),
                    kind: HopKind::DataAttribute,
                    returns: None,
                });
                let expression = render_expression(&hops);
                let derivation = render_derivation(&hops, graph);
                let src_score = score_by_id.get(&src_class).copied().unwrap_or(0.5);
                let tgt_score = score_by_id
                    .get(tgt_attr)
                    .copied()
                    .or_else(|| class_eff.get(tgt_class).copied())
                    .unwrap_or(0.5);
                let score = (src_score + tgt_score) / 2.0;
                out.push(AccessPath {
                    start_class: src_class,
                    target_attr: *tgt_attr,
                    hops,
                    expression,
                    derivation,
                    score,
                });
            }
        }
    }
    dedupe_paths(&mut out);
    out
}

/// Render a list of paths as `CompositionCell`s. Score comes from the path
/// (MRO-propagated by `synth_paths`); callers should not need to re-blend.
pub fn render_cells(paths: &[AccessPath]) -> Vec<CompositionCell> {
    paths
        .iter()
        .map(|p| {
            let summary = format!(
                "AccessPath `{}` — {}",
                p.expression, p.derivation,
            );
            let qname = format!("synth::AccessPath::{}", p.expression);
            let id = synth_id(&qname);
            CompositionCell { qname, summary, score: p.score, id }
        })
        .collect()
}

// ============================================================================
// Edge index
// ============================================================================

/// Precomputed adjacency by edge category. Rebuilding from the graph each
/// BFS would be O(|E|) per search.
struct EdgeIndex {
    /// method → return class (from RETURNS_TYPE edges or docstring hint).
    returns: HashMap<NodeId, Vec<(NodeId, HopKind)>>,
    /// class → methods defined on it (DEFINES edges with METHOD target).
    methods_of: HashMap<NodeId, Vec<NodeId>>,
    /// class → superclasses (INHERITS_FROM edges).
    parents_of: HashMap<NodeId, Vec<NodeId>>,
}

impl EdgeIndex {
    fn build(graph: &RepoGraph) -> Self {
        let mut returns: HashMap<NodeId, Vec<(NodeId, HopKind)>> = HashMap::new();
        let mut methods_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut parents_of: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

        // Record which methods already have an explicit RETURNS_TYPE so we
        // don't duplicate with docstring-inferred ones.
        let mut has_explicit_return: HashSet<NodeId> = HashSet::new();

        for e in &graph.edges {
            match e.category {
                c if c == edge_category::RETURNS_TYPE => {
                    returns.entry(e.from).or_default().push((e.to, HopKind::TypedReturn));
                    has_explicit_return.insert(e.from);
                }
                c if c == edge_category::INHERITS_FROM => {
                    parents_of.entry(e.from).or_default().push(e.to);
                }
                c if c == edge_category::DEFINES => {
                    if graph.nav.kind_by_id.get(&e.to).copied() == Some(node_kind::METHOD) {
                        methods_of.entry(e.from).or_default().push(e.to);
                    }
                }
                _ => {}
            }
        }

        // Docstring fallback — general Python docstring convention.
        let class_by_simple_name = build_class_name_index(graph);
        for (_class, methods) in methods_of.iter() {
            for &m in methods {
                if has_explicit_return.contains(&m) {
                    continue;
                }
                for candidate_name in extract_docstring_class_hints(m, graph) {
                    if let Some(class_ids) = class_by_simple_name.get(&candidate_name) {
                        for &cid in class_ids {
                            returns
                                .entry(m)
                                .or_default()
                                .push((cid, HopKind::DocstringReturn));
                        }
                    }
                }
            }
        }

        // Determinism: per-key Vecs collected entries in graph.edges iteration
        // order (deterministic, Vec) for the first pass, but the docstring
        // fallback walks `methods_of.iter()` (HashMap order) and pushes into
        // `returns` per-method Vecs in non-deterministic order. Sort all Vec
        // values now so bfs_class_paths explores in stable order, dedupe_paths
        // keeps a stable representative, and synth_paths emits stable scores.
        for v in returns.values_mut() {
            v.sort_by(|a, b| a.0.0.cmp(&b.0.0).then((a.1 as u8).cmp(&(b.1 as u8))));
        }
        for v in methods_of.values_mut() {
            v.sort_by_key(|id| id.0);
        }
        for v in parents_of.values_mut() {
            v.sort_by_key(|id| id.0);
        }

        Self { returns, methods_of, parents_of }
    }
}

/// Collect all CLASS nodes indexed by their simple name. Multiple classes may
/// share a simple name across modules — keep all, let downstream type-check
/// filter.
fn build_class_name_index(graph: &RepoGraph) -> HashMap<String, Vec<NodeId>> {
    let mut out: HashMap<String, Vec<NodeId>> = HashMap::new();
    for (id, &kind) in &graph.nav.kind_by_id {
        if kind != node_kind::CLASS {
            continue;
        }
        if let Some(name) = graph.nav.name_by_id.get(id) {
            out.entry(name.clone()).or_default().push(*id);
        }
    }
    // Determinism: per-name Vecs got NodeIds in graph.nav.kind_by_id (HashMap)
    // iter order. Sort each so EdgeIndex::build's docstring-return resolution
    // pushes deterministically into `returns`.
    for v in out.values_mut() {
        v.sort_by_key(|id| id.0);
    }
    out
}

/// General docstring convention: the first sentence of a docstring may
/// contain backtick-quoted type names. Extract them.
/// "Reference to the `Foo` that this bar belongs to" → ["Foo"].
fn extract_docstring_class_hints(method_id: NodeId, graph: &RepoGraph) -> Vec<String> {
    let Some(node) = graph.nodes.iter().find(|n| n.id == method_id) else { return Vec::new() };
    let Some(doc_cell) = node.cells.iter().find(|c| c.kind == cell_type::DOC) else {
        return Vec::new();
    };
    let CellPayload::Text(ref doc) = doc_cell.payload else { return Vec::new() };
    // First sentence — stop at `.`, `\n\n`, or exhaust the string.
    let end = doc
        .find("\n\n")
        .or_else(|| doc.find(". "))
        .unwrap_or(doc.len());
    let first = &doc[..end];
    parse_backtick_identifiers(first)
}

/// Extract identifiers enclosed in backticks. Accepts `Foo`, `pkg.Foo` — keeps
/// only the trailing name for matching, same convention as type-annotation
/// extraction in the Python parser.
fn parse_backtick_identifiers(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '`' { continue; }
        let mut buf = String::new();
        while let Some(&nc) = chars.peek() {
            if nc == '`' { chars.next(); break; }
            buf.push(nc);
            chars.next();
        }
        let trimmed = buf.rsplit('.').next().unwrap_or(&buf).trim();
        if !trimmed.is_empty()
            && trimmed.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
            && trimmed.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.push(trimmed.to_string());
        }
    }
    out
}

// ============================================================================
// BFS
// ============================================================================

/// BFS from `start_class` looking for any path to `target_class` via method-
/// return-type hops. Inheritance on the *target* side is handled by the caller
/// (we'll also match when `target_class` is an ancestor of the reached class).
/// Returns one hop-vector per distinct expression discovered (deduped on
/// expression text).
fn bfs_class_paths(
    start_class: NodeId,
    target_class: NodeId,
    idx: &EdgeIndex,
    graph: &RepoGraph,
    max_traversal_hops: usize,
    activated: &HashSet<NodeId>,
) -> Vec<Vec<Hop>> {
    if max_traversal_hops == 0 {
        return Vec::new();
    }

    let mut results: Vec<Vec<Hop>> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut queue: VecDeque<(NodeId, Vec<Hop>, HashSet<NodeId>)> = VecDeque::new();
    let mut initial_visited = HashSet::new();
    initial_visited.insert(start_class);
    queue.push_back((start_class, Vec::new(), initial_visited));

    while let Some((current_class, hops, visited)) = queue.pop_front() {
        // Termination: attribute on target_class is visible on current_class
        // iff target_class is current_class or an ancestor of current_class
        // (standard MRO attribute lookup). Require ≥1 traversal hop so we
        // don't emit zero-length paths.
        let current_mro = ancestors_including(current_class, idx);
        if !hops.is_empty() && current_mro.contains(&target_class) {
            let expr_key = render_expression_partial(&hops);
            if seen.insert(expr_key) {
                results.push(hops.clone());
            }
            // Continue exploring — alternative paths may also reach target.
        }
        if hops.len() >= max_traversal_hops {
            continue;
        }

        // Expand: every method of current_class (or its MRO ancestors) that
        // has a known return class is a candidate hop. Sage-filter: only
        // property-style methods count as valid attribute-read hops. A
        // `self.method_name` chain reads the callable, not its return value —
        // so non-property methods can't legitimately extend an access path.
        for cls in std::iter::once(current_class).chain(ancestors_of(current_class, idx)) {
            let Some(methods) = idx.methods_of.get(&cls) else { continue };
            for &method_id in methods {
                if !graph.properties.contains(&method_id) {
                    continue;
                }
                let Some(returns_list) = idx.returns.get(&method_id) else { continue };
                for &(return_class, kind) in returns_list {
                    if visited.contains(&return_class) {
                        continue; // avoid cycles
                    }
                    // Prefer methods that are themselves activated — this
                    // keeps the path "grounded" in the PPR result. But don't
                    // *require* it (removes fallback coverage otherwise).
                    let _ = activated.contains(&method_id);
                    let Some(name) = graph.nav.name_by_id.get(&method_id).cloned() else { continue };
                    let mut new_hops = hops.clone();
                    new_hops.push(Hop {
                        owning_class: cls,
                        via: method_id,
                        name,
                        kind,
                        returns: Some(return_class),
                    });
                    let mut new_visited = visited.clone();
                    new_visited.insert(return_class);
                    queue.push_back((return_class, new_hops, new_visited));
                }
            }
        }
    }
    results
}

fn ancestors_including(class_id: NodeId, idx: &EdgeIndex) -> HashSet<NodeId> {
    let mut set: HashSet<NodeId> = HashSet::new();
    set.insert(class_id);
    for a in ancestors_of(class_id, idx) {
        set.insert(a);
    }
    set
}

fn ancestors_of(class_id: NodeId, idx: &EdgeIndex) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![class_id];
    let mut seen: HashSet<NodeId> = HashSet::new();
    while let Some(c) = stack.pop() {
        let Some(parents) = idx.parents_of.get(&c) else { continue };
        for &p in parents {
            if seen.insert(p) {
                out.push(p);
                stack.push(p);
            }
        }
    }
    out
}

// ============================================================================
// Rendering
// ============================================================================

fn render_expression(hops: &[Hop]) -> String {
    let mut s = String::from("self");
    for h in hops {
        s.push('.');
        s.push_str(&h.name);
    }
    s
}

fn render_expression_partial(hops: &[Hop]) -> String {
    // Used as a dedupe key inside BFS before the terminal hop is appended.
    let mut s = String::new();
    for h in hops {
        s.push('.');
        s.push_str(&h.name);
    }
    s
}

fn render_derivation(hops: &[Hop], graph: &RepoGraph) -> String {
    let mut parts: Vec<String> = Vec::new();
    for h in hops {
        let owner_qname = graph
            .nav
            .qname_by_id
            .get(&h.owning_class)
            .cloned()
            .unwrap_or_default();
        let short_owner = owner_qname.rsplit("::").next().unwrap_or(&owner_qname).to_string();
        let frag = match h.kind {
            HopKind::TypedReturn => {
                let ret = h
                    .returns
                    .and_then(|r| graph.nav.name_by_id.get(&r).cloned())
                    .unwrap_or_default();
                format!("`{short_owner}.{}` → `{ret}` (typed return)", h.name)
            }
            HopKind::DocstringReturn => {
                let ret = h
                    .returns
                    .and_then(|r| graph.nav.name_by_id.get(&r).cloned())
                    .unwrap_or_default();
                format!("`{short_owner}.{}` → `{ret}` (docstring hint)", h.name)
            }
            HopKind::DataAttribute => {
                format!("`{short_owner}.{}` (attribute)", h.name)
            }
            HopKind::InheritedLookup => {
                format!("`{short_owner}.{}` (inherited)", h.name)
            }
        };
        parts.push(frag);
    }
    parts.join(" → ")
}

fn dedupe_paths(paths: &mut Vec<AccessPath>) {
    // Determinism: same expression can be produced by multiple (src_class,
    // tgt_attr) pairs with different blended scores. retain() keeps first
    // occurrence — but push order into `paths` depends on BFS / EdgeIndex
    // iteration which has been a recurring HashMap-iter jitter source. Sort
    // first so retain keeps a stable representative regardless of upstream
    // order: highest-score-wins (strongest PPR evidence), tiebreak by NodeId.
    paths.sort_by(|a, b| {
        a.expression.cmp(&b.expression)
            .then_with(|| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.start_class.0.cmp(&b.start_class.0))
            .then_with(|| a.target_attr.0.cmp(&b.target_attr.0))
    });
    let mut seen: HashSet<String> = HashSet::new();
    paths.retain(|p| seen.insert(p.expression.clone()));
}

fn synth_id(qname: &str) -> u64 {
    // Cheap deterministic hash — same FxHash-ish approach used for NodeId.
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    qname.hash(&mut h);
    h.finish()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_core::RepoId;
    use repo_graph_parser_python::parse_file;
    use repo_graph_graph::build_python;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://composition")
    }

    /// Minimal happy-path: a method with `-> OtherClass` reaches an attribute
    /// via one hop. Exercises explicit RETURNS_TYPE + HAS_ATTRIBUTE traversal.
    #[test]
    fn one_hop_typed_return_path() {
        let src = "
class Inner:
    def __init__(self):
        self.value = 1

class Outer:
    def __init__(self):
        self.inner = Inner()
    @property
    def get_inner(self) -> Inner:
        return self.inner
    def use(self):
        return self.get_inner.value
";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let g = build_python(repo(), vec![parse]).unwrap();

        let outer_use = find_by_qname(&g, "m::Outer::use").expect("Outer::use");
        let inner_value = find_by_qname(&g, "m::Inner::value").expect("Inner::value attr");

        let activated = vec![outer_use, inner_value];
        let paths = synth_paths(&activated, &[], &g, 3);
        assert!(
            paths.iter().any(|p| p.expression == "self.get_inner.value"),
            "expected self.get_inner.value path, got: {:?}",
            paths.iter().map(|p| &p.expression).collect::<Vec<_>>()
        );
    }

    /// Docstring-fallback: method has no `-> T:` annotation but docstring says
    /// "`Target`" in first sentence. Must still synth a path.
    #[test]
    fn docstring_backtick_fallback_enables_path() {
        let src = "
class Target:
    def __init__(self):
        self.opts = None

class Source:
    def __init__(self):
        self._t = Target()
    @property
    def resolve(self):
        \"\"\"Reference to the `Target` this belongs to.\"\"\"
        return self._t
    def use(self):
        return self.resolve.opts
";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let g = build_python(repo(), vec![parse]).unwrap();

        let source_use = find_by_qname(&g, "m::Source::use").expect("Source::use");
        let target_opts = find_by_qname(&g, "m::Target::opts").expect("Target::opts");

        let activated = vec![source_use, target_opts];
        let paths = synth_paths(&activated, &[], &g, 3);
        assert!(
            paths.iter().any(|p| p.expression == "self.resolve.opts"),
            "expected self.resolve.opts via docstring hint, got: {:?}",
            paths.iter().map(|p| &p.expression).collect::<Vec<_>>()
        );
    }

    /// Inheritance: target attribute lives on a superclass of the class the
    /// method's return chain reached. MRO lookup must succeed.
    #[test]
    fn inherited_attribute_resolves_via_mro() {
        let src = "
class Base:
    def __init__(self):
        self.shared = 1

class Derived(Base):
    pass

class Source:
    @property
    def get_it(self) -> Derived:
        return Derived()
    def use(self):
        return self.get_it.shared
";
        let parse = parse_file(src, "m.py", "m", repo()).unwrap();
        let g = build_python(repo(), vec![parse]).unwrap();

        let source_use = find_by_qname(&g, "m::Source::use").expect("Source::use");
        let base_shared = find_by_qname(&g, "m::Base::shared").expect("Base::shared");

        let activated = vec![source_use, base_shared];
        let paths = synth_paths(&activated, &[], &g, 3);
        assert!(
            paths.iter().any(|p| p.expression == "self.get_it.shared"),
            "expected self.get_it.shared via Base inheritance, got: {:?}",
            paths.iter().map(|p| &p.expression).collect::<Vec<_>>()
        );
    }

    #[test]
    fn backtick_identifier_parser() {
        let got = parse_backtick_identifiers("Reference to the `Schema` that belongs to `pkg.Klass`.");
        assert_eq!(got, vec!["Schema", "Klass"]);
        // Skips lowercase (not a class convention).
        let got = parse_backtick_identifiers("uses `foo` and `bar`.");
        assert!(got.is_empty());
    }

    #[test]
    fn render_cells_builds_qname_and_score() {
        let path = AccessPath {
            start_class: NodeId::from_parts("code", repo(), node_kind::CLASS, "m::C"),
            target_attr: NodeId::from_parts("code", repo(), node_kind::ATTRIBUTE, "m::D::opts"),
            hops: vec![],
            expression: "self.x.opts".into(),
            derivation: "test".into(),
            score: 0.5,
        };
        let cells = render_cells(&[path]);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].qname, "synth::AccessPath::self.x.opts");
        assert!(cells[0].summary.contains("self.x.opts"));
        assert_eq!(cells[0].score, 0.5);
    }

    // ----- helpers -----

    fn find_by_qname(g: &RepoGraph, qname: &str) -> Option<NodeId> {
        g.nav
            .qname_by_id
            .iter()
            .find_map(|(id, q)| if q == qname { Some(*id) } else { None })
    }
}
