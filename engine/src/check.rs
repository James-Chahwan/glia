//! **check** (LE.8): the declared architecture rules, evaluated. A rule a
//! human wrote down ("web must not reach into the api's internals", "the api
//! has no import cycles") that the graph breaks is a VIOLATION, with the
//! located edges that break it. This is the CI gate over the rules; `glia
//! cycles` (LE.6b) is the report.
//!
//! The rules are LF.4a's `[[constraint]]` stanzas (and cell-API rules), read
//! only through [`declared_constraints`]: its scopes are already
//! repo-relative paths (`.` = the root), so this module never re-resolves a
//! label.
//!
//! - `forbid_edge {from, to}`: every DIRECT edge (the observed fact;
//!   transitive reach is `effects` / `diff-impact` business) whose category is
//!   in the rule's set, `from` end in scope `from` and `to` end in scope `to`.
//!   The set is the rule's `categories` (edge category NAMES), else
//!   [`default_forbid_categories`]: IMPORTS plus the code profile's carry
//!   edges, minus TESTS and DOCUMENTS (a test or a doc legitimately crosses
//!   a boundary). One [`Violation`] per rule; evidence sorted by `(tier,
//!   file, line, category)`, capped at [`MAX_EVIDENCE`] with `count` the
//!   full number.
//! - `no_cycle {scope}`: `categories` empty or exactly `[IMPORTS]` checks the
//!   module import graph (`cycles::module_import_graph`, LE.6b) restricted to
//!   the modules in scope; any other set checks the node-level graph of those
//!   categories restricted to the nodes in scope. One [`Violation`] per
//!   strongly-connected component, its evidence a shortest witness cycle and
//!   its `count` the component's size. An unscoped rule checks the whole
//!   graph.
//! - `invariant {text}` (and any kind this crate cannot evaluate): listed in
//!   [`CheckReport::unchecked`], never dropped.
//!
//! # Tiers (CC.3)
//!
//! Every evidence row carries the tier `why` gives the same edge
//! (`why::tier_of`, one rule for both surfaces): a parser / graph binding the
//! source spells out is [`FACT`]; a resolver- or pass-paired edge, a graph
//! edge inferred below Strong confidence (Go implicit IMPLEMENTS) or one with
//! no location is [`DERIVED`]; an overlay declaration, a git co-change or a
//! name-only guess is [`HEURISTIC`]. The row's `note` says why when the tier
//! is not the stage's plain one, or where an overlay edge was declared.
//!
//! - forbid_edge: the Violation takes its STRONGEST row's tier. One observed
//!   edge proves the forbidden dependency exists; a rule broken only by
//!   resolver-paired edges is derived, only by declared or name-only edges
//!   heuristic. Rows sort tier-first, so the truncation keeps the facts.
//! - no_cycle: the Violation is [`DERIVED`] (the cycle is computed) unless a
//!   hop is [`HEURISTIC`], then heuristic: every hop is needed for the cycle,
//!   so its weakest hop bounds it. A node-level hop is tiered over the real
//!   graph edge; an import hop over the lifted module edge, which carries the
//!   first import's evidence and confidence but no ORIGIN cell (an overlay
//!   import keeps its heuristic tier; its note names the stanza, not who
//!   declared it).
//!
//! Exit codes do not read the tier: a heuristic-only violation still fails
//! CI; the tier tells the reader how sure the graph is.
//!
//! SCOPE MEMBERSHIP IS STRICT. A node is in scope `X` only when the file one
//! [`Locator`] places it in sits under `X` on a path-segment boundary (`web`
//! holds `web/app.py`, not `webhooks/app.py`), or it is a PROJECT whose path
//! does. A node no file places (a SQL table, a queue topic) is in no scope:
//! the keep-unlocatable leniency of `node_in_scope` (right for filtering a
//! blast radius) would accuse every rule that names a data entity's
//! neighbour. A scope no node sits in is a rule error, not a silent pass.
//!
//! Multi-repo: scopes are repo-relative, so a rule is evaluated against every
//! merged repo's nodes at that relative path; a cross-repo edge matches when
//! both ends are in scope in their own repos.
//!
//! Architecture rules only: nothing here carries auth or security semantics.
//!
//! Fired-on marker, one line per call (two when a rule is violated):
//! `[check] rules=<R> checked=<C> violations=<V> (forbid_edge=<F> no_cycle=<N>) unchecked=<U> errors=<E>`,
//! where `V` counts [`Violation`] records (a forbid_edge rule makes at most
//! one, a no_cycle rule one per cycle) and `F` / `N` split them by kind; when
//! `V` > 0 a second line
//! `[check] tiers fact=<F> derived=<D> heuristic=<H>` splits them by tier
//! (CC.3) — grep `^\[check\] tiers`.

use std::collections::{BTreeMap, HashMap, HashSet};

use glia_activation::algo::cycles::{strongly_connected, witness_cycle};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Edge, EdgeCategoryId, NodeId};
use glia_graph::MergedGraph;

use crate::answers::{Locator, in_scope, project_roots};
use crate::cycles::module_import_graph;
use crate::external::declared::declared_constraints;
use crate::profile::CODE_PROFILE;
use crate::why::tier_of;

/// [`Violation::rule_kind`] of a forbidden-edge rule.
pub const FORBID_EDGE: &str = "forbid_edge";
/// [`Violation::rule_kind`] of a no-cycle rule.
pub const NO_CYCLE: &str = "no_cycle";
/// [`Violation::severity`]: an explicit human rule is broken (an observed
/// convention that is broken is a DIVERGENCE, LE.7's business).
pub const VIOLATION: &str = "VIOLATION";
/// A tier (module doc "Tiers"): read at a site the source spells out.
pub const FACT: &str = "fact";
/// A tier: paired or inferred by the build (a resolver, a pass, a graph
/// binding below Strong confidence), or a computed cycle.
pub const DERIVED: &str = "derived";
/// A tier: declared by a person or a model, co-change in git, or a name-only
/// guess.
pub const HEURISTIC: &str = "heuristic";
/// Evidence rows kept per [`Violation`]; `count` keeps the full number.
pub const MAX_EVIDENCE: usize = 100;

/// One edge that breaks a rule: a forbidden edge, or a hop of a witness cycle.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ViolationEdge {
    pub from_qname: String,
    pub to_qname: String,
    /// The edge category name (`IMPORTS`, `CALLS`, ...).
    pub category: &'static str,
    /// Where the edge is asserted: its EVIDENCE site (LC.3a), else the `from`
    /// node's location.
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// The `<stage>:<name>` that put the edge in the graph (`graph:calls`),
    /// when the edge carries EVIDENCE.
    pub emitter: Option<String>,
    /// [`FACT`] | [`DERIVED`] | [`HEURISTIC`]: the tier `why` gives this
    /// edge (module doc "Tiers").
    pub tier: &'static str,
    /// Why the tier is not the emitter stage's plain one, or where an overlay
    /// edge was declared; `why`'s row note.
    pub note: Option<String>,
}

/// One broken rule: all of a forbid_edge rule's edges, or one cycle of a
/// no_cycle rule.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct Violation {
    pub rule_id: String,
    /// [`FORBID_EDGE`] or [`NO_CYCLE`].
    pub rule_kind: &'static str,
    /// `.glia/overlay.toml:<line>` of the stanza; `None` for a cell-API rule.
    pub decl: Option<String>,
    /// Always [`VIOLATION`].
    pub severity: &'static str,
    /// forbid_edge: its strongest evidence row's tier. no_cycle: [`DERIVED`],
    /// or [`HEURISTIC`] when a hop of the witness is (module doc "Tiers").
    pub tier: &'static str,
    /// forbid_edge: the forbidden edges, all of them (the evidence lists at
    /// most [`MAX_EVIDENCE`]). no_cycle: the nodes (modules, for an import
    /// rule) in the cycle's strongly-connected component; the evidence is
    /// one shortest cycle through it.
    pub count: usize,
    pub evidence: Vec<ViolationEdge>,
}

/// Every declared rule, evaluated.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct CheckReport {
    /// CONSTRAINT rules read.
    pub rules: usize,
    /// forbid_edge + no_cycle rules evaluated (a rule with an error is not).
    pub checked: usize,
    /// Ids of the rules no graph query evaluates (`invariant`), in rule order.
    pub unchecked: Vec<String>,
    /// `(rule id, message)`: a rule that could not be evaluated (an unknown
    /// edge category, a scope no node sits in). The rule is skipped.
    pub errors: Vec<(String, String)>,
    /// Sorted by rule id; a rule's cycles in the order of their first member.
    pub violations: Vec<Violation>,
}

/// The categories a forbid_edge rule without `categories` checks: IMPORTS
/// plus `CODE_PROFILE`'s carry edges, minus TESTS and DOCUMENTS.
pub fn default_forbid_categories() -> Vec<EdgeCategoryId> {
    let mut out = vec![edge_category::IMPORTS];
    for c in CODE_PROFILE.tables.carry_edges {
        if *c != edge_category::TESTS && *c != edge_category::DOCUMENTS && !out.contains(c) {
            out.push(*c);
        }
    }
    out
}

/// Evaluate every rule [`declared_constraints`] reads from `merged`.
pub fn check(merged: &MergedGraph) -> CheckReport {
    check_rules(merged, &declared_constraints(merged))
}

/// Evaluate `rules` (as [`declared_constraints`] returns them: anchor node,
/// rule) against `merged`, whichever graph they were read from: a review
/// checks the working tree's rules against the base graph. Emits the marker.
pub(crate) fn check_rules(merged: &MergedGraph, rules: &[(NodeId, ConstraintRule)]) -> CheckReport {
    let mut report = CheckReport {
        rules: rules.len(),
        checked: 0,
        unchecked: Vec::new(),
        errors: Vec::new(),
        violations: Vec::new(),
    };
    if !rules.is_empty() {
        let mut ctx = Ctx::new(merged);
        for (_, rule) in rules {
            match evaluate(&mut ctx, rule) {
                Outcome::Unchecked => report.unchecked.push(rule.id.clone()),
                Outcome::Error(msg) => report.errors.push((rule.id.clone(), msg)),
                Outcome::Checked(found) => {
                    report.checked += 1;
                    report.violations.extend(found);
                }
            }
        }
    }
    // Stable: a rule's cycles keep their order.
    report.violations.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    let of = |k: &str| {
        report
            .violations
            .iter()
            .filter(|v| v.rule_kind == k)
            .count()
    };
    eprintln!(
        "[check] rules={} checked={} violations={} (forbid_edge={} no_cycle={}) unchecked={} errors={}",
        report.rules,
        report.checked,
        report.violations.len(),
        of(FORBID_EDGE),
        of(NO_CYCLE),
        report.unchecked.len(),
        report.errors.len(),
    );
    if !report.violations.is_empty() {
        let tier = |t: &str| report.violations.iter().filter(|v| v.tier == t).count();
        eprintln!(
            "[check] tiers fact={} derived={} heuristic={}",
            tier(FACT),
            tier(DERIVED),
            tier(HEURISTIC),
        );
    }
    report
}

/// Sort rank of a tier, strongest first; an unknown spelling sorts last.
fn tier_rank(t: &str) -> u8 {
    match t {
        FACT => 0,
        DERIVED => 1,
        HEURISTIC => 2,
        _ => 3,
    }
}

enum Outcome {
    Unchecked,
    Error(String),
    Checked(Vec<Violation>),
}

fn evaluate(ctx: &mut Ctx<'_>, rule: &ConstraintRule) -> Outcome {
    // Only the kinds this module evaluates reach the category check: an
    // invariant's categories are never read.
    match &rule.kind {
        ConstraintKind::ForbidEdge { .. } | ConstraintKind::NoCycle { .. } => {}
        _ => return Outcome::Unchecked,
    }
    let categories = match categories_of(&rule.categories) {
        Ok(c) => c,
        Err(e) => return Outcome::Error(e),
    };
    let result = match &rule.kind {
        ConstraintKind::ForbidEdge { from, to } => {
            let cats = if categories.is_empty() {
                default_forbid_categories()
            } else {
                categories
            };
            forbid_edge(ctx, rule, from, to, &cats).map(|v| v.into_iter().collect())
        }
        ConstraintKind::NoCycle { scope } => no_cycle(ctx, rule, scope.as_deref(), &categories),
        _ => return Outcome::Unchecked,
    };
    match result {
        Ok(found) => Outcome::Checked(found),
        Err(e) => Outcome::Error(e),
    }
}

/// The rule's category names as ids; an unregistered name is an error that
/// names every one of them.
fn categories_of(names: &[String]) -> Result<Vec<EdgeCategoryId>, String> {
    let mut ids = Vec::with_capacity(names.len());
    let mut unknown = Vec::new();
    for n in names {
        match edge_category::ALL
            .iter()
            .find(|(_, name)| *name == n.as_str())
        {
            Some((id, _)) if !ids.contains(id) => ids.push(*id),
            Some(_) => {}
            None => unknown.push(format!("`{n}`")),
        }
    }
    if unknown.is_empty() {
        Ok(ids)
    } else {
        Err(format!(
            "unknown edge categor{} {} (the rule is skipped; name registered edge categories)",
            if unknown.len() == 1 { "y" } else { "ies" },
            unknown.join(", ")
        ))
    }
}

/// Per-call node facts: every node's located file (once), the PROJECT
/// paths, and the members of each scope a rule names.
struct Ctx<'a> {
    merged: &'a MergedGraph,
    loc: Locator<'a>,
    /// Every node of every graph with the file the locator places it in.
    files: HashMap<NodeId, Option<String>>,
    /// PROJECT node -> its repo-relative path (`.` = the root).
    projects: HashMap<NodeId, String>,
    scopes: HashMap<String, HashSet<NodeId>>,
    qnames: HashMap<NodeId, String>,
}

impl<'a> Ctx<'a> {
    fn new(merged: &'a MergedGraph) -> Self {
        let loc = Locator::new(merged);
        let path_of: HashMap<String, String> = project_roots(merged)
            .into_iter()
            .map(|p| (p.qname, p.path))
            .collect();
        let mut files: HashMap<NodeId, Option<String>> = HashMap::new();
        let mut projects: HashMap<NodeId, String> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if files.contains_key(&n.id) {
                    continue;
                }
                files.insert(n.id, loc.file_of(n.id));
                if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::PROJECT)
                    && let Some(p) = g.nav.qname_by_id.get(&n.id).and_then(|q| path_of.get(q))
                {
                    projects.insert(n.id, p.clone());
                }
            }
        }
        Ctx {
            merged,
            loc,
            files,
            projects,
            scopes: HashMap::new(),
            qnames: HashMap::new(),
        }
    }

    /// The nodes strictly in `scope`, computed once per scope.
    fn members(&mut self, scope: &str) -> &HashSet<NodeId> {
        if !self.scopes.contains_key(scope) {
            let mut set = HashSet::new();
            for (id, file) in &self.files {
                let located = file.as_deref().is_some_and(|f| in_scope(f, scope));
                let project = self.projects.get(id).is_some_and(|p| in_scope(p, scope));
                if located || project {
                    set.insert(*id);
                }
            }
            self.scopes.insert(scope.to_string(), set);
        }
        &self.scopes[scope]
    }

    /// The members of `scope`, or the error that names the empty scope.
    fn scope(&mut self, scope: &str, role: &str) -> Result<HashSet<NodeId>, String> {
        let set = self.members(scope);
        if set.is_empty() {
            return Err(format!(
                "{role} scope `{scope}` matches no located node (no file sits under it and no PROJECT is at it)"
            ));
        }
        Ok(set.clone())
    }

    fn qname(&mut self, id: NodeId) -> String {
        if let Some(q) = self.qnames.get(&id) {
            return q.clone();
        }
        let q = self.loc.locate(id).qname;
        self.qnames.insert(id, q.clone());
        q
    }

    /// One located evidence row: at the edge's EVIDENCE site when it names a
    /// file, else at the `from` node. `(tier, note)` is `why::tier_of` over
    /// the edge, computed by the caller that holds it.
    fn row(
        &mut self,
        from: NodeId,
        category: EdgeCategoryId,
        to: NodeId,
        ev: Option<&Evidence>,
        (tier, note): (&'static str, Option<String>),
    ) -> ViolationEdge {
        let f = self.loc.locate(from);
        let site = ev.and_then(|e| {
            e.file
                .clone()
                .map(|file| (Some(file), e.line.map(|l| i64::from(l) + 1)))
        });
        let (file, line) = site.unwrap_or((f.file, f.line));
        ViolationEdge {
            from_qname: f.qname,
            to_qname: self.qname(to),
            category: edge_category::name(category),
            file,
            line,
            emitter: ev.map(|e| e.emitter.clone()),
            tier,
            note,
        }
    }
}

fn violation(
    rule: &ConstraintRule,
    kind: &'static str,
    tier: &'static str,
    count: usize,
    mut evidence: Vec<ViolationEdge>,
) -> Violation {
    evidence.truncate(MAX_EVIDENCE);
    Violation {
        rule_id: rule.id.clone(),
        rule_kind: kind,
        decl: rule.decl.clone(),
        severity: VIOLATION,
        tier,
        count,
        evidence,
    }
}

// ============================================================================
// forbid_edge
// ============================================================================

/// One forbidden edge as reported: `(from, category, to, file, 0-based line)`.
/// An edge two graphs both hold is one row.
type EdgeSite = (NodeId, u32, NodeId, Option<String>, Option<u32>);

fn forbid_edge(
    ctx: &mut Ctx<'_>,
    rule: &ConstraintRule,
    from: &str,
    to: &str,
    cats: &[EdgeCategoryId],
) -> Result<Option<Violation>, String> {
    let from_set = ctx.scope(from, "from")?;
    let to_set = ctx.scope(to, "to")?;
    let merged = ctx.merged;
    let mut seen: HashSet<EdgeSite> = HashSet::new();
    let mut rows: Vec<ViolationEdge> = Vec::new();
    for e in merged.all_edges() {
        if !cats.contains(&e.category) || !from_set.contains(&e.from) || !to_set.contains(&e.to) {
            continue;
        }
        let ev = Evidence::of(e);
        let key = (
            e.from,
            e.category.0,
            e.to,
            ev.as_ref().and_then(|x| x.file.clone()),
            ev.as_ref().and_then(|x| x.line),
        );
        if !seen.insert(key) {
            continue;
        }
        let tier = tier_of(ev.as_ref(), e);
        rows.push(ctx.row(e.from, e.category, e.to, ev.as_ref(), tier));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    // Strongest tier first (so the MAX_EVIDENCE cut keeps the facts), then
    // located rows by (file, line, category); qnames break the rest.
    rows.sort_by(|a, b| {
        (
            tier_rank(a.tier),
            a.file.is_none(),
            &a.file,
            a.line.is_none(),
            a.line,
            a.category,
        )
            .cmp(&(
                tier_rank(b.tier),
                b.file.is_none(),
                &b.file,
                b.line.is_none(),
                b.line,
                b.category,
            ))
            .then_with(|| a.from_qname.cmp(&b.from_qname))
            .then_with(|| a.to_qname.cmp(&b.to_qname))
    });
    // Sorted tier-first: the first row holds the strongest tier.
    let tier = rows.first().map_or(FACT, |r| r.tier);
    let count = rows.len();
    Ok(Some(violation(rule, FORBID_EDGE, tier, count, rows)))
}

// ============================================================================
// no_cycle
// ============================================================================

/// A graph restricted to one scope: the kept nodes and the edges between them.
struct Sub {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl GraphSource for Sub {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// Where a hop of a sub-graph edge was asserted, and its `why` tier and note,
/// computed from the edge the hop stands for when it is recorded.
type Sites = BTreeMap<(u64, u32, u64), (Option<Evidence>, &'static str, Option<String>)>;

fn no_cycle(
    ctx: &mut Ctx<'_>,
    rule: &ConstraintRule,
    scope: Option<&str>,
    cats: &[EdgeCategoryId],
) -> Result<Vec<Violation>, String> {
    let keep: Option<HashSet<NodeId>> = match scope {
        Some(s) => Some(ctx.scope(s, "no_cycle")?),
        None => None,
    };
    let inside = |id: &NodeId| keep.as_ref().is_none_or(|k| k.contains(id));
    let imports_only = cats.is_empty() || cats == [edge_category::IMPORTS];

    let mut sub = Sub {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut sites: Sites = BTreeMap::new();
    let mut node_seen: HashSet<NodeId> = HashSet::new();
    let mut add_node = |sub: &mut Sub, id: NodeId| {
        if node_seen.insert(id) {
            sub.nodes.push(id);
        }
    };
    if imports_only {
        let imports = module_import_graph(ctx.merged);
        for e in imports.edges() {
            if !(inside(&e.from) && inside(&e.to)) {
                continue;
            }
            add_node(&mut sub, e.from);
            add_node(&mut sub, e.to);
            sub.edges
                .push(Edge::new(e.from, e.to, e.category, e.confidence));
            // The lifted edge carries the first import's evidence and
            // confidence, not its ORIGIN cell (module doc "Tiers").
            sites
                .entry((e.from.0, e.category.0, e.to.0))
                .or_insert_with(|| {
                    let ev = imports.evidence(e.from, e.to);
                    let (tier, note) = tier_of(ev, e);
                    (ev.cloned(), tier, note)
                });
        }
    } else {
        for e in ctx.merged.all_edges() {
            if !(cats.contains(&e.category) && inside(&e.from) && inside(&e.to)) {
                continue;
            }
            add_node(&mut sub, e.from);
            add_node(&mut sub, e.to);
            sub.edges
                .push(Edge::new(e.from, e.to, e.category, e.confidence));
            sites
                .entry((e.from.0, e.category.0, e.to.0))
                .or_insert_with(|| {
                    let ev = Evidence::of(e);
                    let (tier, note) = tier_of(ev.as_ref(), e);
                    (ev, tier, note)
                });
        }
    }

    let adj = Adjacency::build(&sub, &CategorySet::all());
    let mut cycles: Vec<(String, Violation)> = Vec::new();
    for comp in strongly_connected(&adj) {
        // The member first in qname order starts the witness, so the same
        // code checked out elsewhere (other node ids) gives the same cycle.
        let mut by_qname: Vec<(String, NodeId)> =
            comp.iter().map(|id| (ctx.qname(*id), *id)).collect();
        by_qname.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
        let Some((first, start)) = by_qname.first().cloned() else {
            continue;
        };
        let evidence: Vec<ViolationEdge> = witness_cycle(&adj, &comp, start)
            .into_iter()
            .map(|(f, c, t)| {
                // Every witness hop is a sub edge, so it has a site; the
                // fallback only keeps a missing one honest.
                let (ev, tier) = match sites.get(&(f.0, c.0, t.0)) {
                    Some((ev, tier, note)) => (ev.as_ref(), (*tier, note.clone())),
                    None => (None, (DERIVED, Some("no evidence recorded".to_string()))),
                };
                ctx.row(f, c, t, ev, tier)
            })
            .collect();
        // Every hop is needed for the cycle: its weakest hop bounds it.
        let tier = if evidence.iter().any(|e| e.tier == HEURISTIC) {
            HEURISTIC
        } else {
            DERIVED
        };
        cycles.push((first, violation(rule, NO_CYCLE, tier, comp.len(), evidence)));
    }
    cycles.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(cycles.into_iter().map(|(_, v)| v).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_forbid_set_is_imports_plus_carry_minus_tests_and_docs() {
        let d = default_forbid_categories();
        assert_eq!(d.first(), Some(&edge_category::IMPORTS));
        assert!(d.contains(&edge_category::CALLS));
        assert!(d.contains(&edge_category::HTTP_CALLS));
        assert!(!d.contains(&edge_category::TESTS));
        assert!(!d.contains(&edge_category::DOCUMENTS));
        assert!(
            !d.contains(&edge_category::CONTAINS),
            "structure is no dependency"
        );
        let mut sorted = d.clone();
        sorted.sort_by_key(|c| c.0);
        sorted.dedup();
        assert_eq!(sorted.len(), d.len(), "no category twice");
    }

    #[test]
    fn categories_of_maps_names_and_reports_every_unknown() {
        assert_eq!(
            categories_of(&["CALLS".into(), "IMPORTS".into(), "CALLS".into()]),
            Ok(vec![edge_category::CALLS, edge_category::IMPORTS])
        );
        assert_eq!(categories_of(&[]), Ok(Vec::new()));
        let err = categories_of(&["CALLS".into(), "NOPE".into(), "GONE".into()])
            .expect_err("unknown names");
        assert!(
            err.contains("`NOPE`") && err.contains("`GONE`") && err.contains("categories"),
            "{err}"
        );
        let one = categories_of(&["NOPE".into()]).expect_err("unknown name");
        assert!(one.contains("category `NOPE`"), "{one}");
    }

    #[test]
    fn tier_rank_orders_strongest_first() {
        let mut t = vec![HEURISTIC, "other", FACT, DERIVED];
        t.sort_by_key(|x| tier_rank(x));
        assert_eq!(t, [FACT, DERIVED, HEURISTIC, "other"]);
    }

    #[test]
    fn empty_graph_has_no_rules() {
        let r = check(&MergedGraph::new(Vec::new()));
        assert_eq!((r.rules, r.checked), (0, 0));
        assert!(r.violations.is_empty() && r.unchecked.is_empty() && r.errors.is_empty());
    }
}
