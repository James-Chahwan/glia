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
//!   a boundary). One [`Violation`] per rule, tier [`FACT`]; evidence sorted
//!   by `(file, line, category)`, capped at [`MAX_EVIDENCE`] with `count` the
//!   full number.
//! - `no_cycle {scope}`: `categories` empty or exactly `[IMPORTS]` checks the
//!   module import graph (`cycles::module_import_graph`, LE.6b) restricted to
//!   the modules in scope; any other set checks the node-level graph of those
//!   categories restricted to the nodes in scope. One [`Violation`] per
//!   strongly-connected component, tier [`DERIVED`] (every hop is an observed
//!   edge, only the cycle is computed), its evidence a shortest witness cycle
//!   and its `count` the component's size. An unscoped rule checks the whole
//!   graph.
//! - `invariant {text}` (and any kind this crate cannot evaluate): listed in
//!   [`CheckReport::unchecked`], never dropped.
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
//! Fired-on marker, one line per call:
//! `[check] rules=<R> checked=<C> violations=<V> (forbid_edge=<F> no_cycle=<N>) unchecked=<U> errors=<E>`,
//! where `V` counts [`Violation`] records (a forbid_edge rule makes at most
//! one, a no_cycle rule one per cycle) and `F` / `N` split them by kind.

use std::collections::{BTreeMap, HashMap, HashSet};

use repo_graph_activation::algo::cycles::{strongly_connected, witness_cycle};
use repo_graph_activation::algo::{Adjacency, CategorySet, GraphSource};
use repo_graph_code_domain::evidence::Evidence;
use repo_graph_code_domain::external_inputs::{ConstraintKind, ConstraintRule};
use repo_graph_code_domain::{edge_category, node_kind};
use repo_graph_core::{Edge, EdgeCategoryId, NodeId};
use repo_graph_graph::MergedGraph;

use crate::answers::{Locator, in_scope, project_roots};
use crate::cycles::module_import_graph;
use crate::external::declared::declared_constraints;
use crate::profile::CODE_PROFILE;

/// [`Violation::rule_kind`] of a forbidden-edge rule.
pub const FORBID_EDGE: &str = "forbid_edge";
/// [`Violation::rule_kind`] of a no-cycle rule.
pub const NO_CYCLE: &str = "no_cycle";
/// [`Violation::severity`]: an explicit human rule is broken (an observed
/// convention that is broken is a DIVERGENCE, LE.7's business).
pub const VIOLATION: &str = "VIOLATION";
/// [`Violation::tier`]: the evidence is the observed edges themselves.
pub const FACT: &str = "fact";
/// [`Violation::tier`]: every hop is an observed edge; the cycle is computed.
pub const DERIVED: &str = "derived";
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
    /// [`FACT`] (forbid_edge) or [`DERIVED`] (no_cycle).
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
    let rules = declared_constraints(merged);
    let mut report = CheckReport {
        rules: rules.len(),
        checked: 0,
        unchecked: Vec::new(),
        errors: Vec::new(),
        violations: Vec::new(),
    };
    if !rules.is_empty() {
        let mut ctx = Ctx::new(merged);
        for (_, rule) in &rules {
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
    report
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
    /// file, else at the `from` node.
    fn row(
        &mut self,
        from: NodeId,
        category: EdgeCategoryId,
        to: NodeId,
        ev: Option<&Evidence>,
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
        rows.push(ctx.row(e.from, e.category, e.to, ev.as_ref()));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    // Located rows first, by (file, line, category); qnames break the rest.
    rows.sort_by(|a, b| {
        (
            a.file.is_none(),
            &a.file,
            a.line.is_none(),
            a.line,
            a.category,
        )
            .cmp(&(
                b.file.is_none(),
                &b.file,
                b.line.is_none(),
                b.line,
                b.category,
            ))
            .then_with(|| a.from_qname.cmp(&b.from_qname))
            .then_with(|| a.to_qname.cmp(&b.to_qname))
    });
    let count = rows.len();
    Ok(Some(violation(rule, FORBID_EDGE, FACT, count, rows)))
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

/// Where a hop of a sub-graph edge was asserted.
type Sites = BTreeMap<(u64, u32, u64), Option<Evidence>>;

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
            sites
                .entry((e.from.0, e.category.0, e.to.0))
                .or_insert_with(|| imports.evidence(e.from, e.to).cloned());
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
                .or_insert_with(|| Evidence::of(e));
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
                let ev = sites.get(&(f.0, c.0, t.0)).and_then(Option::as_ref);
                ctx.row(f, c, t, ev)
            })
            .collect();
        cycles.push((
            first,
            violation(rule, NO_CYCLE, DERIVED, comp.len(), evidence),
        ));
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
    fn empty_graph_has_no_rules() {
        let r = check(&MergedGraph::new(Vec::new()));
        assert_eq!((r.rules, r.checked), (0, 0));
        assert!(r.violations.is_empty() && r.unchecked.is_empty() && r.errors.is_empty());
    }
}
