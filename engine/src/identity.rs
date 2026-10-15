//! The identity view every answer collapses through: SAME_AS classes by rule
//! tier, walk classes, display members and `also_in` rows (Q17), and the
//! `glia identity` report. Public module slot, reached by module path
//! (`glia_engine::identity::<item>`). Filled by DD.8.
//!
//! # Why one module
//!
//! Six answers (find, blast radius, trace, tests-for, serves, arch) collapse
//! an identity class to one row the SAME way, so reading SAME_AS edges into
//! classes, choosing the row that stands for a class and listing the other
//! members live here; CLI and pyo3 stay transport. An answer builds one
//! [`IdentityView`] with [`IdentityView::new`] from its [`Collapse`] option and
//! then calls [`IdentityView::fold`] on its ranked rows BEFORE its top-k cut
//! (a class never spends two slots), [`IdentityView::quotient`] /
//! [`IdentityView::walk_rep`] for a walk and [`IdentityView::display`] /
//! [`IdentityView::row_identity`] for each shown row.
//!
//! # Classes
//!
//! One scan of `merged.cross_edges` for SAME_AS (the identity resolver, DD.5,
//! only ever writes cross edges). Each edge's tier: emitter
//! `resolver:identity` with a rule the code domain's table declares
//! (`CODE_PROFILE.tables.identity_rules`, DD.3, the base rule read through
//! `same_as::split_evidence_rule`) takes that rule's [`IdentityTier`]; any
//! other edge (an overlay `[[edge]]` SAME_AS, a rule the table does not
//! declare) takes `why::tier_of`'s tier, heuristic for overlay and history
//! and names no rule. Three [`Classes`] (DD.1):
//!
//! - **collapse**: the FACT / DERIVED edges. Its classes fold to one row in
//!   every answer. Tier = the weakest of its edges (`Fact < Derived`), rules =
//!   the distinct base rules in table order, diverged = any edge with the
//!   `/diverged` qualifier. A heuristic edge between two members of one
//!   collapse class does not weaken it.
//! - **walk**: the collapse edges whose rule also walks (`walk == true`:
//!   messages, contract ops, SCIP symbols, a package and its project; not
//!   `package_name`, whose class of `react` spans every dependant). A walk
//!   class is one node in a walk ([`IdentityView::quotient`]). Every walk
//!   class lies inside one collapse class.
//! - **candidates**: the HEURISTIC edges between nodes not already in one
//!   collapse class (vendored copies, overlay guesses). Never collapsed and
//!   never walked (Q18); only the report reads them (an answer's view skips
//!   them, so a graph of only heuristic edges costs it one scan).
//!
//! The direction of a SAME_AS edge means nothing here and one pair may carry
//! two edges (`contract_op` and `contract_route` on the same two ops): the
//! classes are union-find sets, every edge only adds its tier, rule and
//! qualifier.
//!
//! # Display member
//!
//! The row that stands for a class is chosen by CONTENT, never by NodeId (the
//! class minimum moves when a RepoId does, LB.1): the least member by (index
//! of the first graph naming it, located file, line, qname), unlocated files
//! and lines after located ones. `also_in` is every other member, located and
//! labelled, sorted by (repo label, file, line, qname). No field carries a
//! class id; a report class's `key` is its display member's qname.
//!
//! A repo label is `Collapse::repo_labels[repo id]` (the build's
//! `GenerateResult::repo_labels`), else `repo#<id % 1000>` (`union_labels`'
//! collision form): the motivating PACKAGE_DEP rows are unlocated, so the
//! repo is what tells two copies apart.
//!
//! # Cost
//!
//! Free when unused: `!enabled`, or a scan that meets no SAME_AS edge, is an
//! empty view and every answer is then its HEAD self, byte for byte. Only a
//! view with classes builds a [`Locator`] (O(V)) and one pass over the nodes
//! for the members' contents.
//!
//! # fired_on
//!
//! - `[identity] collapse surface=<s> classes=<c> walk_classes=<w> folded=<f> rows=<n>`
//!   once per answer through [`IdentityView::collapse_marker`], only when the
//!   view has a collapse class. grep `^\[identity\] collapse surface=`.
//! - `[identity] report classes=<c> fact=<f> derived=<d> heuristic=<h> diverged=<x>`
//!   once per [`identity_classes`] call that answers (an invalid filter
//!   prints nothing). grep `^\[identity\] report classes=`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use glia_activation::algo::GraphSource;
use glia_activation::algo::classes::{Classes, Quotient};
use glia_activation::profile::IdentityTier;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, same_as};
use glia_core::{Edge, NodeId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence, plural};
use crate::answers::{Located, Locator};
use crate::profile::CODE_PROFILE;
use crate::why;

/// The identity resolver's EVIDENCE emitter (DD.5): the `resolver!` stamp of
/// its `CODE_PASSES` row.
const IDENTITY_EMITTER: &str = "resolver:identity";

/// The report's absence primitive and marker surface.
const PRIMITIVE: &str = "identity";

// ============================================================================
// Public types
// ============================================================================

/// How an answer treats identity classes (Q17): collapsed by default, labels
/// for the `also_in` rows from the build. `Collapse::off()` is the raw rows
/// (`--no-collapse`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Collapse {
    /// Fold FACT / DERIVED classes to one row and walk walk classes once.
    pub enabled: bool,
    /// Repo id -> label (`GenerateResult::repo_labels`). A repo without one
    /// is `repo#<id % 1000>`.
    pub repo_labels: BTreeMap<u64, String>,
}

impl Default for Collapse {
    /// Enabled, no labels.
    fn default() -> Self {
        Self {
            enabled: true,
            repo_labels: BTreeMap::new(),
        }
    }
}

impl Collapse {
    /// No collapse: every answer is its raw, per-member self.
    pub fn off() -> Self {
        Self {
            enabled: false,
            repo_labels: BTreeMap::new(),
        }
    }

    /// Enabled, with the build's repo labels.
    pub fn with_labels(labels: &BTreeMap<u64, String>) -> Self {
        Self {
            enabled: true,
            repo_labels: labels.clone(),
        }
    }
}

/// One other member of a collapsed row's class: where else the thing is.
/// `line` is 1-based, as every answer record's.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AlsoIn {
    pub repo: String,
    pub qname: String,
    pub file: Option<String>,
    pub line: Option<i64>,
}

/// What a row that stands for a collapse class says about it.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RowIdentity {
    /// `fact` or `derived`: the weakest tier of the class's edges.
    pub tier: &'static str,
    /// The identity rules that joined the class, in table order.
    pub rules: Vec<&'static str>,
    /// Two members' contents differ (an edge carries `<rule>/diverged`).
    pub diverged: bool,
    /// Every member but the shown one, by (repo, file, line, qname).
    pub also_in: Vec<AlsoIn>,
}

/// The `glia identity` filters. Build with `Default` and set fields.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct IdentityQuery {
    /// `fact`, `derived` or `heuristic`; `None` is every tier.
    pub tier: Option<String>,
    /// One identity rule name; `None` is every rule.
    pub rule: Option<String>,
    /// Only classes whose members' contents differ.
    pub diverged_only: bool,
    /// Classes to show after filtering; `None` or `Some(0)` is every class.
    pub limit: Option<usize>,
    /// Repo id -> label, as [`Collapse::repo_labels`].
    pub repo_labels: BTreeMap<u64, String>,
}

/// One located member of a report class.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClassMember {
    pub repo: String,
    pub qname: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// Index of this member's content among the class's distinct contents
    /// (`same_as::content_key`, DD.2), in member order; `None` without one.
    pub variant: Option<u32>,
}

/// One identity class of the report.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityClass {
    /// The display member's qname: what the class is shown as.
    pub key: String,
    /// `fact`, `derived` or `heuristic`.
    pub tier: &'static str,
    /// The identity rules that joined it, in table order (empty for a class
    /// only overlay edges declared).
    pub rules: Vec<&'static str>,
    /// Folded to one row in answers (FACT / DERIVED).
    pub collapses: bool,
    /// At least two of its members are one node in walks.
    pub walks: bool,
    pub diverged: bool,
    /// Distinct known contents among the members.
    pub variants: u32,
    /// The display member first, then the others by (repo, file, line, qname).
    pub members: Vec<ClassMember>,
}

/// The report's totals, over the classes that passed the filters (before
/// `limit`).
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdentityCounts {
    pub classes: usize,
    pub members: usize,
    pub fact: usize,
    pub derived: usize,
    pub heuristic: usize,
    pub diverged: usize,
    /// Classes per identity rule, every table rule in table order.
    pub by_rule: Vec<(&'static str, usize)>,
}

/// `glia identity`: the identity classes, collapse classes (fact / derived)
/// first, then the heuristic same-as candidates.
#[derive(serde::Serialize, Debug, Clone)]
#[non_exhaustive]
pub struct IdentityReport {
    pub counts: IdentityCounts,
    pub classes: Vec<IdentityClass>,
    /// Classes the `limit` cut.
    pub omitted: usize,
    /// `Some` exactly when `classes` is empty.
    pub absence: Option<Absence>,
}

// ============================================================================
// The view
// ============================================================================

/// One SAME_AS edge as the view reads it.
struct Reading {
    from: NodeId,
    to: NodeId,
    tier: IdentityTier,
    /// Index into the table's `identity_rules`, for an identity-resolver edge.
    rule: Option<usize>,
    walk: bool,
    diverged: bool,
}

/// One class member, located once.
struct Member {
    id: NodeId,
    /// Index of the first graph naming it; `usize::MAX` when none does.
    graph: usize,
    repo: String,
    at: Located,
    content: Option<u64>,
}

/// One class's reading.
struct ClassInfo {
    tier: IdentityTier,
    /// Table indices, ascending, each once.
    rules: Vec<usize>,
    diverged: bool,
    /// Member indices (into `IdentityView::members`) by the also_in order.
    sorted: Vec<usize>,
    /// The display member (a member index).
    display: usize,
}

/// The identity classes of one graph, read once per answer (module doc).
/// Built with [`IdentityView::new`]; carries no class id. Public, with
/// private fields, so a surface outside the engine (and
/// `engine/tests/identity_view.rs`) can fold its own rows the same way.
pub struct IdentityView {
    collapse: Classes,
    walk: Classes,
    candidates: Classes,
    /// Every member of a collapse or candidate class, sorted by id.
    members: Vec<Member>,
    /// Parallel to `collapse.classes()` (by representative ascending).
    collapse_info: Vec<ClassInfo>,
    collapse_reps: Vec<NodeId>,
    /// Parallel to `candidates.classes()`.
    candidate_info: Vec<ClassInfo>,
    candidate_reps: Vec<NodeId>,
}

impl IdentityView {
    /// The view of `merged` for an answer asked with `collapse`: empty when
    /// collapse is off or no SAME_AS edge exists (one scan of the cross
    /// edges), so the answer is then exactly its uncollapsed self.
    pub fn new(merged: &MergedGraph, collapse: &Collapse) -> Self {
        if !collapse.enabled {
            return Self::empty();
        }
        Self::read(merged, &collapse.repo_labels, false)
    }

    fn empty() -> Self {
        Self {
            collapse: Classes::default(),
            walk: Classes::default(),
            candidates: Classes::default(),
            members: Vec::new(),
            collapse_info: Vec::new(),
            collapse_reps: Vec::new(),
            candidate_info: Vec::new(),
            candidate_reps: Vec::new(),
        }
    }

    /// The classes of `merged`. `with_candidates` reads the heuristic
    /// candidates too (the report); an answer never shows them, so it skips
    /// them, and a graph whose only SAME_AS edges are heuristic costs it one
    /// scan and no [`Locator`].
    fn read(merged: &MergedGraph, labels: &BTreeMap<u64, String>, with_candidates: bool) -> Self {
        let readings: Vec<Reading> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .map(read_edge)
            .collect();
        if readings.is_empty() {
            return Self::empty();
        }
        let pair = |r: &Reading| (r.from, r.to);
        let collapse =
            Classes::from_pairs(readings.iter().filter(|r| r.tier.collapses()).map(pair));
        let walk = Classes::from_pairs(readings.iter().filter(|r| r.walk).map(pair));
        let candidates = Classes::from_pairs(
            readings
                .iter()
                .filter(|r| with_candidates && !r.tier.collapses() && !collapse.same(r.from, r.to))
                .map(pair),
        );
        if collapse.is_empty() && candidates.is_empty() {
            return Self::empty();
        }

        let mut ids: Vec<NodeId> = collapse
            .classes()
            .chain(candidates.classes())
            .flat_map(|(_, m)| m.iter().copied())
            .collect();
        ids.sort_unstable_by_key(|id| id.0);
        ids.dedup();
        let members = locate_members(merged, &ids, labels);

        let mut view = Self {
            collapse_reps: collapse.classes().map(|(rep, _)| rep).collect(),
            collapse_info: Vec::new(),
            candidate_reps: candidates.classes().map(|(rep, _)| rep).collect(),
            candidate_info: Vec::new(),
            collapse,
            walk,
            candidates,
            members,
        };
        view.collapse_info = view.infos(&view.collapse);
        view.candidate_info = view.infos(&view.candidates);
        for r in readings.iter().filter(|r| r.from != r.to) {
            let (classes, reps, info) = if r.tier.collapses() {
                (&view.collapse, &view.collapse_reps, &mut view.collapse_info)
            } else if with_candidates && !view.collapse.same(r.from, r.to) {
                (
                    &view.candidates,
                    &view.candidate_reps,
                    &mut view.candidate_info,
                )
            } else {
                continue;
            };
            let Ok(c) = reps.binary_search_by_key(&classes.rep(r.from).0, |x| x.0) else {
                continue;
            };
            let c = &mut info[c];
            c.tier = c.tier.max(r.tier);
            c.diverged |= r.diverged;
            if let Some(rule) = r.rule
                && let Err(at) = c.rules.binary_search(&rule)
            {
                c.rules.insert(at, rule);
            }
        }
        view
    }

    /// One [`ClassInfo`] per class of `classes`, tier at its strongest until
    /// the edges are read.
    fn infos(&self, classes: &Classes) -> Vec<ClassInfo> {
        classes
            .classes()
            .map(|(_, ids)| {
                let idx: Vec<usize> = ids.iter().filter_map(|id| self.member_index(*id)).collect();
                let mut sorted = idx.clone();
                sorted.sort_by(|&a, &b| also_in_order(&self.members[a], &self.members[b]));
                let display = idx
                    .iter()
                    .copied()
                    .min_by(|&a, &b| display_order(&self.members[a], &self.members[b]))
                    .unwrap_or(0);
                ClassInfo {
                    tier: IdentityTier::Fact,
                    rules: Vec::new(),
                    diverged: false,
                    sorted,
                    display,
                }
            })
            .collect()
    }

    fn member_index(&self, id: NodeId) -> Option<usize> {
        self.members.binary_search_by_key(&id.0, |m| m.id.0).ok()
    }

    /// The collapse class `id` is in, as an index into `collapse_info`.
    fn collapse_class(&self, id: NodeId) -> Option<usize> {
        self.collapse.class_of(id)?;
        self.collapse_reps
            .binary_search_by_key(&self.collapse.rep(id).0, |r| r.0)
            .ok()
    }

    /// No collapse class: every answer is its uncollapsed self. (Heuristic
    /// candidates never change an answer, so a view of only those is empty.)
    pub fn is_empty(&self) -> bool {
        self.collapse.is_empty()
    }

    /// Some class is contracted in walks.
    pub fn has_walk_classes(&self) -> bool {
        !self.walk.is_empty()
    }

    /// `g` with every walk class contracted to one node (DD.1), or `None`
    /// without walk classes (walk `g` itself). A walk over it can reach a
    /// representative `g` does not list; map every reached id through
    /// [`Self::display`] before showing it.
    pub fn quotient<'v, G: GraphSource + ?Sized>(&'v self, g: &'v G) -> Option<Quotient<'v, G>> {
        self.has_walk_classes()
            .then(|| Quotient::new(g, &self.walk))
    }

    /// The node `id` is in a walk: its walk class's representative, or `id`.
    pub fn walk_rep(&self, id: NodeId) -> NodeId {
        self.walk.rep(id)
    }

    /// The node a row for `id` shows: its collapse class's display member, or
    /// `id` outside one. A walk representative is a member of its walk class,
    /// so it maps to that class's collapse display member.
    pub fn display(&self, id: NodeId) -> NodeId {
        match self.collapse_class(id) {
            Some(c) => self.members[self.collapse_info[c].display].id,
            None => id,
        }
    }

    /// The identity of a row that stands for `id`'s collapse class and shows
    /// `shown`: `None` outside a collapse class; `also_in` is every member
    /// but `shown`.
    pub fn row_identity(&self, id: NodeId, shown: NodeId) -> Option<RowIdentity> {
        let c = &self.collapse_info[self.collapse_class(id)?];
        Some(RowIdentity {
            tier: c.tier.as_str(),
            rules: rule_names(&c.rules),
            diverged: c.diverged,
            also_in: c
                .sorted
                .iter()
                .map(|&m| &self.members[m])
                .filter(|m| m.id != shown)
                .map(|m| AlsoIn {
                    repo: m.repo.clone(),
                    qname: m.at.qname.clone(),
                    file: m.at.file.clone(),
                    line: m.at.line,
                })
                .collect(),
        })
    }

    /// The row collapse every answer runs BEFORE its top-k cut: the first row
    /// of each collapse class (in the answer's order) is kept and gets
    /// `set_identity(row, row_identity(id, id))`, its later rows are dropped;
    /// a row outside every class passes untouched. `id_of` is the node the
    /// row shows.
    pub fn fold<T>(
        &self,
        rows: Vec<T>,
        id_of: impl Fn(&T) -> NodeId,
        mut set_identity: impl FnMut(&mut T, RowIdentity),
    ) -> Vec<T> {
        if self.is_empty() {
            return rows;
        }
        let mut seen = vec![false; self.collapse_info.len()];
        let mut out = Vec::with_capacity(rows.len());
        for mut row in rows {
            let id = id_of(&row);
            if let Some(c) = self.collapse_class(id) {
                if seen[c] {
                    continue;
                }
                seen[c] = true;
                if let Some(identity) = self.row_identity(id, id) {
                    set_identity(&mut row, identity);
                }
            }
            out.push(row);
        }
        out
    }

    /// The `[identity] collapse` line, once per answer, only when the view
    /// has a collapse class: `rows_before` the fold, `rows_after` it.
    pub fn collapse_marker(&self, surface: &str, rows_before: usize, rows_after: usize) {
        if self.is_empty() {
            return;
        }
        eprintln!(
            "[identity] collapse surface={surface} classes={} walk_classes={} folded={} rows={rows_after}",
            self.collapse.len(),
            self.walk.len(),
            rows_before.saturating_sub(rows_after),
        );
    }

    /// Every class as a report row with its sort key: the collapse classes,
    /// then the candidates.
    fn report_classes(&self) -> Vec<(ClassKey, IdentityClass)> {
        let collapse = self.collapse_info.iter().map(|c| (c, true));
        let candidates = self.candidate_info.iter().map(|c| (c, false));
        collapse
            .chain(candidates)
            .map(|(c, collapses)| {
                let order: Vec<&Member> = std::iter::once(c.display)
                    .chain(c.sorted.iter().copied().filter(|&m| m != c.display))
                    .map(|m| &self.members[m])
                    .collect();
                let mut contents: Vec<u64> = Vec::new();
                let members: Vec<ClassMember> = order
                    .iter()
                    .map(|m| ClassMember {
                        repo: m.repo.clone(),
                        qname: m.at.qname.clone(),
                        kind: m.at.kind,
                        file: m.at.file.clone(),
                        line: m.at.line,
                        variant: m
                            .content
                            .map(|k| match contents.iter().position(|&x| x == k) {
                                Some(i) => i as u32,
                                None => {
                                    contents.push(k);
                                    (contents.len() - 1) as u32
                                }
                            }),
                    })
                    .collect();
                let walks = collapses && order.iter().any(|m| self.walk.class_of(m.id).is_some());
                let class = IdentityClass {
                    key: order
                        .first()
                        .map(|m| m.at.qname.clone())
                        .unwrap_or_default(),
                    tier: c.tier.as_str(),
                    rules: rule_names(&c.rules),
                    collapses,
                    walks,
                    diverged: c.diverged,
                    variants: contents.len() as u32,
                    members,
                };
                let key = ClassKey {
                    tier: c.tier,
                    rules: c.rules.clone(),
                    key: class.key.clone(),
                    members: class
                        .members
                        .iter()
                        .map(|m| (m.repo.clone(), m.qname.clone()))
                        .collect(),
                };
                (key, class)
            })
            .collect()
    }
}

/// The report order: (tier, rules in table order, key), then the members.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ClassKey {
    tier: IdentityTier,
    rules: Vec<usize>,
    key: String,
    members: Vec<(String, String)>,
}

/// Tier, rule and qualifier of one SAME_AS edge (module doc).
fn read_edge(e: &Edge) -> Reading {
    let ev = Evidence::of(e);
    let (base, diverged) = ev
        .as_ref()
        .and_then(|ev| ev.rule.as_deref())
        .map(same_as::split_evidence_rule)
        .unwrap_or(("", false));
    let rules = CODE_PROFILE.tables.identity_rules;
    let declared = ev
        .as_ref()
        .filter(|ev| ev.emitter == IDENTITY_EMITTER)
        .and_then(|_| rules.iter().position(|r| r.name == base));
    let (tier, rule, walk) = match declared {
        Some(i) => (
            rules[i].tier,
            Some(i),
            rules[i].walk && rules[i].tier.collapses(),
        ),
        None => (tier_named(why::tier_of(ev.as_ref(), e).0), None, false),
    };
    Reading {
        from: e.from,
        to: e.to,
        tier,
        rule,
        walk,
        diverged,
    }
}

/// `why`'s tier string as an [`IdentityTier`].
fn tier_named(tier: &str) -> IdentityTier {
    match tier {
        why::FACT => IdentityTier::Fact,
        why::DERIVED => IdentityTier::Derived,
        _ => IdentityTier::Heuristic,
    }
}

fn rule_names(rules: &[usize]) -> Vec<&'static str> {
    let table = CODE_PROFILE.tables.identity_rules;
    rules
        .iter()
        .filter_map(|&i| table.get(i).map(|r| r.name))
        .collect()
}

/// `ids` (sorted) located and labelled, with their contents: one Locator and
/// one pass over every graph's nodes.
fn locate_members(
    merged: &MergedGraph,
    ids: &[NodeId],
    labels: &BTreeMap<u64, String>,
) -> Vec<Member> {
    let locator = Locator::new(merged);
    let find = |id: NodeId| ids.binary_search_by_key(&id.0, |x| x.0).ok();
    let mut content: Vec<Option<Option<u64>>> = vec![None; ids.len()];
    for g in &merged.graphs {
        for n in &g.nodes {
            if let Some(i) = find(n.id)
                && content[i].is_none()
                && let Some(kind) = g.nav.kind_by_id.get(&n.id)
            {
                content[i] = Some(same_as::content_key(*kind, &n.cells));
            }
        }
    }
    ids.iter()
        .zip(content)
        .map(|(&id, content)| {
            let graph = merged
                .graphs
                .iter()
                .position(|g| g.nav.qname_by_id.contains_key(&id));
            let repo = match graph {
                Some(gi) => {
                    let repo = merged.graphs[gi].repo.0;
                    labels
                        .get(&repo)
                        .cloned()
                        .unwrap_or_else(|| format!("repo#{}", repo % 1000))
                }
                None => "repo#?".to_string(),
            };
            Member {
                id,
                graph: graph.unwrap_or(usize::MAX),
                repo,
                at: locator.locate(id),
                content: content.flatten(),
            }
        })
        .collect()
}

/// `(file, line)` with unlocated values after located ones.
fn place_order(a: &Located, b: &Located) -> Ordering {
    (a.file.is_none(), &a.file, a.line.is_none(), a.line).cmp(&(
        b.file.is_none(),
        &b.file,
        b.line.is_none(),
        b.line,
    ))
}

/// The display member is the least by (first graph, file, line, qname): a
/// function of content and argument order, never of a NodeId (the id only
/// breaks a tie between two nodes alike in all of that).
fn display_order(a: &Member, b: &Member) -> Ordering {
    a.graph
        .cmp(&b.graph)
        .then_with(|| place_order(&a.at, &b.at))
        .then_with(|| a.at.qname.cmp(&b.at.qname))
        .then_with(|| a.id.0.cmp(&b.id.0))
}

/// The `also_in` order: (repo label, file, line, qname).
fn also_in_order(a: &Member, b: &Member) -> Ordering {
    a.repo
        .cmp(&b.repo)
        .then_with(|| place_order(&a.at, &b.at))
        .then_with(|| a.at.qname.cmp(&b.at.qname))
        .then_with(|| a.graph.cmp(&b.graph))
        .then_with(|| a.id.0.cmp(&b.id.0))
}

// ============================================================================
// The report
// ============================================================================

/// `glia identity` (DD.15 renders it): every identity class of `merged` with
/// its located members, filtered by `q`. `Err` names the valid values of an
/// unknown tier or rule. An empty answer carries the absence: no SAME_AS
/// edge, or the filters removed every class.
pub fn identity_classes(merged: &MergedGraph, q: &IdentityQuery) -> Result<IdentityReport, String> {
    let tier = q.tier.as_deref().map(parse_tier).transpose()?;
    let rule = q.rule.as_deref().map(parse_rule).transpose()?;
    let view = IdentityView::read(merged, &q.repo_labels, true);
    let mut all = view.report_classes();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let total = all.len();
    let kept: Vec<(ClassKey, IdentityClass)> = all
        .into_iter()
        .filter(|(k, c)| {
            tier.is_none_or(|t| k.tier == t)
                && rule.is_none_or(|r| k.rules.contains(&r))
                && (!q.diverged_only || c.diverged)
        })
        .collect();

    let table = CODE_PROFILE.tables.identity_rules;
    let count = |t: IdentityTier| kept.iter().filter(|(k, _)| k.tier == t).count();
    let counts = IdentityCounts {
        classes: kept.len(),
        members: kept.iter().map(|(_, c)| c.members.len()).sum(),
        fact: count(IdentityTier::Fact),
        derived: count(IdentityTier::Derived),
        heuristic: count(IdentityTier::Heuristic),
        diverged: kept.iter().filter(|(_, c)| c.diverged).count(),
        by_rule: table
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    r.name,
                    kept.iter().filter(|(k, _)| k.rules.contains(&i)).count(),
                )
            })
            .collect(),
    };
    let mut classes: Vec<IdentityClass> = kept.into_iter().map(|(_, c)| c).collect();
    let shown = match q.limit {
        Some(n) if n > 0 => n.min(classes.len()),
        _ => classes.len(),
    };
    let omitted = classes.len() - shown;
    classes.truncate(shown);

    let absence = classes.is_empty().then(|| {
        let edges = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .count();
        let note = if edges == 0 {
            "no SAME_AS edge: one repo with no shared package, message or contract op".to_string()
        } else if total == 0 {
            format!(
                "{edges} SAME_AS {} {} no class (each joins a node to itself)",
                plural(edges, "edge", "edges"),
                plural(edges, "forms", "form")
            )
        } else {
            format!(
                "filters removed {total} {}",
                plural(total, "class", "classes")
            )
        };
        let mechanisms = [edge_category::name(edge_category::SAME_AS)];
        absence::empty(
            merged,
            PRIMITIVE,
            &query_text(q),
            "no_match",
            note,
            &mechanisms,
            None,
        )
    });
    eprintln!(
        "[identity] report classes={} fact={} derived={} heuristic={} diverged={}",
        counts.classes, counts.fact, counts.derived, counts.heuristic, counts.diverged
    );
    Ok(IdentityReport {
        counts,
        classes,
        omitted,
        absence,
    })
}

fn parse_tier(s: &str) -> Result<IdentityTier, String> {
    [
        IdentityTier::Fact,
        IdentityTier::Derived,
        IdentityTier::Heuristic,
    ]
    .into_iter()
    .find(|t| t.as_str() == s.trim().to_ascii_lowercase())
    .ok_or_else(|| format!("unknown identity tier `{s}`: expected fact, derived or heuristic"))
}

/// The table index of rule `s`.
fn parse_rule(s: &str) -> Result<usize, String> {
    let table = CODE_PROFILE.tables.identity_rules;
    table
        .iter()
        .position(|r| r.name == s.trim())
        .ok_or_else(|| {
            let names: Vec<&str> = table.iter().map(|r| r.name).collect();
            format!(
                "unknown identity rule `{s}`: expected one of {}",
                names.join(", ")
            )
        })
}

/// The query as the absence records it: `identity` plus the filters given.
fn query_text(q: &IdentityQuery) -> String {
    let mut parts = vec![PRIMITIVE.to_string()];
    if let Some(t) = &q.tier {
        parts.push(format!("tier={t}"));
    }
    if let Some(r) = &q.rule {
        parts.push(format!("rule={r}"));
    }
    if q.diverged_only {
        parts.push("diverged_only".to_string());
    }
    parts.join(" ")
}
